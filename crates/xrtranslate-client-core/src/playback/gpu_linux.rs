//! Linux video producer: independent OpenGL with Vulkan external memory and sync.
#[path = "gpu_linux/context.rs"]
mod context;
#[path = "gpu_linux/shared.rs"]
mod shared;

use super::{RenderContext, end, param};
use ash::{khr, vk};
use context::GlContext;
use eframe::{
    egui_wgpu::{RenderState, WgpuSetup, WgpuSetupExisting},
    wgpu,
};
use libmpv_sys as mpv;
use shared::GlImage;
pub use shared::SharedImage;
use std::{ffi::CStr, sync::Arc};

/// Enable external semaphore support when present, without making video interop
/// a requirement for opening the UI. All rendering still uses unmodified wgpu.
pub fn graphics_setup(setup: &WgpuSetup) -> Result<WgpuSetup, String> {
    let WgpuSetup::CreateNew(config) = setup else {
        return Ok(setup.clone());
    };
    let instance = futures::executor::block_on(setup.new_instance());
    let adapter =
        futures::executor::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: config.power_preference,
            ..Default::default()
        }))
        .map_err(|e| e.to_string())?;
    let descriptor = (config.device_descriptor)(&adapter);
    let open = unsafe {
        let hal = adapter
            .as_hal::<wgpu::hal::api::Vulkan>()
            .ok_or("Linux UI requires Vulkan")?;
        let extensions = hal
            .shared_instance()
            .raw_instance()
            .enumerate_device_extension_properties(hal.raw_physical_device())
            .map_err(|e| e.to_string())?;
        let external_sync = extensions.iter().any(|extension| {
            CStr::from_ptr(extension.extension_name.as_ptr()) == khr::external_semaphore_fd::NAME
        });
        hal.open_with_callback(
            descriptor.required_features,
            &descriptor.required_limits,
            &descriptor.memory_hints,
            Some(Box::new(move |args| {
                if external_sync {
                    args.extensions.push(khr::external_semaphore_fd::NAME);
                }
            })),
        )
        .map_err(|e| e.to_string())?
    };
    let (device, queue) =
        unsafe { adapter.create_device_from_hal::<wgpu::hal::api::Vulkan>(open, &descriptor) }
            .map_err(|e| e.to_string())?;
    Ok(WgpuSetup::Existing(WgpuSetupExisting {
        instance,
        adapter,
        device,
        queue,
    }))
}

#[derive(Clone)]
pub struct Device {
    device_uuid: [u8; 16],
    driver_uuid: [u8; 16],
}
impl Device {
    pub fn new(state: &RenderState) -> Result<Self, String> {
        let (device_uuid, driver_uuid) = unsafe {
            let hal = state
                .device
                .as_hal::<wgpu::hal::api::Vulkan>()
                .ok_or("GPU video requires Vulkan")?;
            for extension in [
                khr::external_memory_fd::NAME,
                khr::external_semaphore_fd::NAME,
            ] {
                if !hal.enabled_device_extensions().contains(&extension) {
                    return Err(format!(
                        "GPU video sharing requires {}",
                        extension.to_string_lossy()
                    ));
                }
            }
            let mut sync = vk::ExternalSemaphoreProperties::default();
            hal.shared_instance()
                .raw_instance()
                .get_physical_device_external_semaphore_properties(
                    hal.raw_physical_device(),
                    &vk::PhysicalDeviceExternalSemaphoreInfo::default()
                        .handle_type(vk::ExternalSemaphoreHandleTypeFlags::OPAQUE_FD),
                    &mut sync,
                );
            if !sync
                .external_semaphore_features
                .contains(vk::ExternalSemaphoreFeatureFlags::EXPORTABLE)
            {
                return Err("GPU does not support exporting video synchronization handles".into());
            }
            let mut id = vk::PhysicalDeviceIDProperties::default();
            let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut id);
            hal.shared_instance()
                .raw_instance()
                .get_physical_device_properties2(hal.raw_physical_device(), &mut properties);
            (id.device_uuid, id.driver_uuid)
        };
        Ok(Self {
            device_uuid,
            driver_uuid,
        })
    }
    pub fn create_image(
        &self,
        state: &RenderState,
        size: [u32; 2],
    ) -> Result<Arc<SharedImage>, String> {
        SharedImage::new(state, size)
    }
}

pub struct Renderer {
    pub context: RenderContext,
    gl: Box<GlContext>,
    image: Option<GlImage>,
}
impl Renderer {
    pub fn new(
        handle: usize,
        device: Device,
        display: Option<super::super::VideoDisplay>,
    ) -> Result<Self, String> {
        let gl = GlContext::new(device.device_uuid, device.driver_uuid)?;
        // This context stays current only on the video worker for its entire life.
        let mut init = mpv::mpv_opengl_init_params {
            get_proc_address: Some(context::get_proc_address),
            get_proc_address_ctx: &gl.display as *const _ as *mut _,
            extra_exts: std::ptr::null(),
        };
        let mut params = vec![
            param(1, c"opengl".as_ptr() as *mut _),
            param(2, &mut init as *mut _ as *mut _),
        ];
        match display {
            Some(super::super::VideoDisplay::X11(ptr)) => params.push(param(8, ptr as *mut _)),
            Some(super::super::VideoDisplay::Wayland(ptr)) => params.push(param(9, ptr as *mut _)),
            None => {}
        }
        params.push(end());
        let context = RenderContext::new(handle, &mut params)?;
        log::info!(
            "Media GPU renderer: independent OpenGL producer, Vulkan shared-memory transfer"
        );
        Ok(Self {
            context,
            gl,
            image: None,
        })
    }

    pub fn draw(&mut self, shared: Arc<SharedImage>, changed: bool) -> Result<(), String> {
        if self
            .image
            .as_ref()
            .is_none_or(|image| !Arc::ptr_eq(&image.shared, &shared))
        {
            if let Some(old) = self.image.take() {
                old.destroy(&self.gl);
            }
            self.image = Some(GlImage::new(&self.gl, shared)?);
        }
        let image = self.image.as_ref().unwrap();
        image.wait(&self.gl);
        let result = if changed {
            let mut target = mpv::mpv_opengl_fbo {
                fbo: image.framebuffer.0.get() as i32,
                w: image.shared.size[0] as i32,
                h: image.shared.size[1] as i32,
                internal_format: eframe::glow::RGBA8 as i32,
            };
            self.context
                .render(param(3, &mut target as *mut _ as *mut _))
        } else {
            Ok(())
        };
        image.signal(&self.gl);
        result.and_then(|_| self.gl.check())
    }
}
impl Drop for Renderer {
    fn drop(&mut self) {
        self.context.close();
        if let Some(image) = self.image.take() {
            image.destroy(&self.gl);
        }
    }
}
