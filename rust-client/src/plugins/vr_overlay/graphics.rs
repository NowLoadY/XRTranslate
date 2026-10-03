//! SteamVR's GPU and extension requirements applied before Vulkan creation.
use super::openvr::{AvatarTracking, OpenVrOverlay, OverlayError, VulkanTexture, raw_rgba_len};
use ash::{vk, vk::Handle};
use eframe::wgpu::{
    self,
    hal::{api::Vulkan, vulkan},
};
use std::{
    ffi::{CStr, CString},
    rc::Rc,
    sync::{LazyLock, Mutex},
};

// HAL retains static names. Intern only new runtime names, once per process,
// with a fixed 16 KiB bound; reconnects do not leak another copy of each name.
fn intern(names: Vec<CString>) -> Result<Vec<&'static CStr>, String> {
    static NAMES: LazyLock<Mutex<Vec<&'static CStr>>> = LazyLock::new(|| Mutex::new(Vec::new()));
    let mut saved = NAMES
        .lock()
        .map_err(|_| "Vulkan extension cache unavailable")?;
    let mut result = Vec::new();
    for name in names {
        if name.as_bytes().len() > 255 || !name.as_bytes().starts_with(b"VK_") {
            return Err("Invalid SteamVR Vulkan extension name".into());
        }
        let value = if let Some(&value) = saved.iter().find(|&&value| value == name.as_c_str()) {
            value
        } else {
            if saved.len() >= 64 {
                return Err("Too many SteamVR Vulkan extensions".into());
            }
            let value: &'static CStr = Box::leak(name.into_boxed_c_str());
            saved.push(value);
            value
        };
        if !result.contains(&value) {
            result.push(value);
        }
    }
    Ok(result)
}

struct InstanceOwner(ash::Instance);
impl Drop for InstanceOwner {
    fn drop(&mut self) {
        unsafe { self.0.destroy_instance(None) };
    }
}
struct DeviceOwner(ash::Device);
impl Drop for DeviceOwner {
    fn drop(&mut self) {
        unsafe {
            let _ = self.0.device_wait_idle();
            self.0.destroy_device(None)
        };
    }
}

pub(super) struct Graphics {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

pub(super) fn create(tracking: &AvatarTracking) -> Result<Rc<Graphics>, String> {
    // This dedicated queue is touched only by the overlay worker, including
    // Valve's submission. Desktop rendering never concurrently uses it.
    let entry = unsafe { ash::Entry::load() }.map_err(|e| e.to_string())?;
    let version = unsafe { entry.try_enumerate_instance_version() }
        .map_err(|e| e.to_string())?
        .unwrap_or(vk::API_VERSION_1_0)
        .min(vk::API_VERSION_1_3);
    if version < vk::API_VERSION_1_1 {
        return Err("Vulkan 1.1 is required for VR overlays".into());
    }
    let flags = wgpu::InstanceFlags::empty();
    let mut extensions =
        vulkan::Instance::desired_extensions(&entry, version, flags).map_err(|e| e.to_string())?;
    for name in intern(tracking.extensions(None)?)? {
        if !extensions.contains(&name) {
            extensions.push(name);
        }
    }
    let pointers: Vec<_> = extensions.iter().map(|s| s.as_ptr()).collect();
    let app = vk::ApplicationInfo::default()
        .application_name(c"XRTranslate Overlays")
        .api_version(version);
    let raw = unsafe {
        entry.create_instance(
            &vk::InstanceCreateInfo::default()
                .application_info(&app)
                .enabled_extension_names(&pointers),
            None,
        )
    }
    .map_err(|e| e.to_string())?;
    let owner = InstanceOwner(raw.clone());
    let hal = unsafe {
        vulkan::Instance::from_raw(
            entry,
            raw,
            version,
            0,
            None,
            extensions,
            flags,
            Default::default(),
            false,
            Some(Box::new(move || drop(owner))),
        )
    }
    .map_err(|e| e.to_string())?;
    let instance = unsafe { wgpu::Instance::from_hal::<Vulkan>(hal) };
    let raw_instance = unsafe { instance.as_hal::<Vulkan>() }
        .ok_or("Vulkan instance unavailable")?
        .shared_instance()
        .raw_instance();
    let required = tracking.output_device(raw_instance.handle().as_raw() as *mut _);
    if required == 0 {
        return Err("SteamVR did not provide its Vulkan GPU".into());
    }
    let adapter = futures::executor::block_on(instance.enumerate_adapters(wgpu::Backends::VULKAN))
        .into_iter()
        .find(|adapter| {
            unsafe { adapter.as_hal::<Vulkan>() }
                .is_some_and(|hal| hal.raw_physical_device().as_raw() == required)
        })
        .ok_or("SteamVR GPU is unavailable for overlay rendering")?;
    let descriptor = wgpu::DeviceDescriptor::default();
    let hal = unsafe { adapter.as_hal::<Vulkan>() }.ok_or("Vulkan adapter unavailable")?;
    let mut extensions = hal.required_device_extensions(descriptor.required_features);
    for name in intern(tracking.extensions(Some(required as *mut _))?)? {
        if !extensions.contains(&name) {
            extensions.push(name);
        }
    }
    let raw_instance = hal.shared_instance().raw_instance();
    let physical = hal.raw_physical_device();
    let family = unsafe { raw_instance.get_physical_device_queue_family_properties(physical) }
        .iter()
        .position(|q| q.queue_count > 0 && q.queue_flags.contains(vk::QueueFlags::GRAPHICS))
        .ok_or("SteamVR GPU has no graphics queue")? as u32;
    let priorities = [1.0];
    let queues = [vk::DeviceQueueCreateInfo::default()
        .queue_family_index(family)
        .queue_priorities(&priorities)];
    let pointers: Vec<_> = extensions.iter().map(|s| s.as_ptr()).collect();
    let mut features = hal.physical_device_features(&extensions, descriptor.required_features);
    let info = features.add_to_device_create(
        vk::DeviceCreateInfo::default()
            .queue_create_infos(&queues)
            .enabled_extension_names(&pointers),
    );
    let raw =
        unsafe { raw_instance.create_device(physical, &info, None) }.map_err(|e| e.to_string())?;
    let owner = DeviceOwner(raw.clone());
    let opened = unsafe {
        hal.device_from_raw(
            raw,
            Some(Box::new(move || drop(owner))),
            &extensions,
            descriptor.required_features,
            &descriptor.required_limits,
            &descriptor.memory_hints,
            family,
            0,
        )
    }
    .map_err(|e| e.to_string())?;
    drop(hal);
    let (device, queue) = unsafe { adapter.create_device_from_hal::<Vulkan>(opened, &descriptor) }
        .map_err(|e| e.to_string())?;
    device.on_uncaptured_error(std::sync::Arc::new(|error| {
        log::error!("Overlay GPU error: {error}");
    }));
    let graphics = Rc::new(Graphics { device, queue });
    tracking.retain_graphics(Box::new(Rc::clone(&graphics)));
    Ok(graphics)
}

pub(super) fn texture(
    device: &wgpu::Device,
    texture: &wgpu::Texture,
) -> Result<VulkanTexture, String> {
    if texture.format() != wgpu::TextureFormat::Rgba8Unorm
        || texture.dimension() != wgpu::TextureDimension::D2
        || texture.depth_or_array_layers() != 1
        || texture.sample_count() != 1
    {
        return Err("Overlay texture must be a single RGBA8 image".into());
    }
    let width = texture.width();
    let height = texture.height();
    let device = unsafe { device.as_hal::<Vulkan>() }.ok_or("Overlay GPU is unavailable")?;
    let texture = unsafe { texture.as_hal::<Vulkan>() }.ok_or("Overlay texture is unavailable")?;
    Ok(VulkanTexture {
        image: unsafe { texture.raw_handle() }.as_raw(),
        device: device.raw_device().handle().as_raw() as *mut _,
        physical_device: device.raw_physical_device().as_raw() as *mut _,
        instance: device.shared_instance().raw_instance().handle().as_raw() as *mut _,
        queue: device.raw_queue().as_raw() as *mut _,
        queue_family: device.queue_family_index(),
        width,
        height,
        format: vk::Format::R8G8B8A8_UNORM.as_raw() as u32,
        samples: 1,
    })
}

/// Reusable GPU image for straight-alpha, sRGB caption and speech pixels.
pub(super) struct RgbaTexture {
    graphics: Rc<Graphics>,
    texture: wgpu::Texture,
    native: VulkanTexture,
    len: usize,
}

impl RgbaTexture {
    pub fn new(
        tracking: &AvatarTracking,
        graphics: Rc<Graphics>,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        let len = raw_rgba_len(width, height)?;
        let validation = graphics
            .device
            .push_error_scope(wgpu::ErrorFilter::Validation);
        let image = graphics.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("RGBA overlay"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        if let Some(error) = futures::executor::block_on(validation.pop()) {
            return Err(error.to_string());
        }
        let native = texture(&graphics.device, &image)?;
        tracking.retain_graphics(Box::new(image.clone()));
        Ok(Self {
            graphics,
            texture: image,
            native,
            len,
        })
    }

    pub fn upload(&mut self, overlay: &OpenVrOverlay, pixels: &[u8]) -> Result<(), OverlayError> {
        if pixels.len() != self.len {
            return Err(OverlayError::InvalidFrame(
                "Invalid buffer length for RGBA overlay".into(),
            ));
        }
        let validation = self
            .graphics
            .device
            .push_error_scope(wgpu::ErrorFilter::Validation);
        self.graphics.queue.write_texture(
            self.texture.as_image_copy(),
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.native.width * 4),
                rows_per_image: Some(self.native.height),
            },
            self.texture.size(),
        );
        let mut encoder = self
            .graphics
            .device
            .create_command_encoder(&Default::default());
        // SteamVR reads on this same queue and leaves TRANSFER_SRC_OPTIMAL intact.
        encoder.transition_resources(
            std::iter::empty(),
            std::iter::once(wgpu::TextureTransition {
                texture: &self.texture,
                selector: None,
                state: wgpu::TextureUses::COPY_SRC,
            }),
        );
        self.graphics.queue.submit([encoder.finish()]);
        if let Some(error) = futures::executor::block_on(validation.pop()) {
            return Err(OverlayError::InvalidFrame(error.to_string()));
        }
        overlay.set_vulkan_texture(&mut self.native)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extension_names_are_stable_deduplicated_and_bounded() {
        let names = || vec![CString::new("VK_KHR_external_memory").unwrap(); 2];
        let a = intern(names()).unwrap();
        let b = intern(names()).unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].as_ptr(), b[0].as_ptr());
        assert!(intern(vec![CString::new("invalid").unwrap()]).is_err());
        assert!(
            intern(vec![
                CString::new(format!("VK_{}", "x".repeat(256))).unwrap()
            ])
            .is_err()
        );
    }
}
