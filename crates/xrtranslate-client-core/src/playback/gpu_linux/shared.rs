//! Bounded GPU handoff: GL writes a shared image, Vulkan copies it to the UI
//! texture and immediately returns it. No CPU pixel transfers or UI-thread waits.
use super::context::{GlContext, OPAQUE_FD, TRANSFER_SRC};
use ash::{khr, vk};
use eframe::{
    egui_wgpu::RenderState,
    glow::{self, HasContext},
    wgpu,
};
use std::{
    os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd},
    sync::Arc,
};

struct Allocation {
    device: ash::Device,
    _owner: wgpu::Device,
    image: vk::Image,
    memory: vk::DeviceMemory,
    semaphores: [vk::Semaphore; 2], // available for GL, ready for Vulkan
}

impl Drop for Allocation {
    fn drop(&mut self) {
        unsafe {
            for semaphore in self.semaphores {
                self.device.destroy_semaphore(semaphore, None);
            }
            self.device.destroy_image(self.image, None);
            self.device.free_memory(self.memory, None);
        }
    }
}

pub struct SharedImage {
    allocation: Arc<Allocation>,
    pub texture: wgpu::Texture,
    pub size: [u32; 2],
    memory_size: u64,
    family: u32,
    memory_fd: khr::external_memory_fd::Device,
    semaphore_fd: khr::external_semaphore_fd::Device,
}

impl SharedImage {
    pub fn new(state: &RenderState, size: [u32; 2]) -> Result<Arc<Self>, String> {
        let hal = unsafe { state.device.as_hal::<wgpu::hal::api::Vulkan>() }
            .ok_or("GPU video requires the Vulkan UI backend")?;
        let device = hal.raw_device();
        let instance = hal.shared_instance().raw_instance();
        let mut external_info = vk::PhysicalDeviceExternalImageFormatInfo::default()
            .handle_type(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
        let format_info = vk::PhysicalDeviceImageFormatInfo2::default()
            .format(vk::Format::R8G8B8A8_UNORM)
            .ty(vk::ImageType::TYPE_2D)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC)
            .push_next(&mut external_info);
        let mut external_properties = vk::ExternalImageFormatProperties::default();
        let mut properties =
            vk::ImageFormatProperties2::default().push_next(&mut external_properties);
        unsafe {
            instance.get_physical_device_image_format_properties2(
                hal.raw_physical_device(),
                &format_info,
                &mut properties,
            )
        }
        .map_err(|e| format!("Shared video image format is unavailable: {e}"))?;
        if !external_properties
            .external_memory_properties
            .external_memory_features
            .contains(vk::ExternalMemoryFeatureFlags::EXPORTABLE)
        {
            return Err("GPU does not support exporting video images".into());
        }
        let mut allocation = Allocation {
            device: device.clone(),
            _owner: state.device.clone(),
            image: vk::Image::null(),
            memory: vk::DeviceMemory::null(),
            semaphores: [vk::Semaphore::null(); 2],
        };
        let mut external = vk::ExternalMemoryImageCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
        let extent = vk::Extent3D {
            width: size[0],
            height: size[1],
            depth: 1,
        };
        let info = vk::ImageCreateInfo::default()
            .push_next(&mut external)
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk::Format::R8G8B8A8_UNORM)
            .extent(extent)
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        allocation.image =
            unsafe { device.create_image(&info, None) }.map_err(|e| e.to_string())?;
        let requirements = unsafe { device.get_image_memory_requirements(allocation.image) };
        let properties =
            unsafe { instance.get_physical_device_memory_properties(hal.raw_physical_device()) };
        let memory_type = properties
            .memory_types_as_slice()
            .iter()
            .enumerate()
            .find(|(index, ty)| {
                requirements.memory_type_bits & (1 << index) != 0
                    && ty
                        .property_flags
                        .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
            })
            .map(|(index, _)| index as u32)
            .ok_or("No device-local video memory")?;
        let mut export = vk::ExportMemoryAllocateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(allocation.image);
        let info = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_type)
            .push_next(&mut export)
            .push_next(&mut dedicated);
        allocation.memory =
            unsafe { device.allocate_memory(&info, None) }.map_err(|e| e.to_string())?;
        unsafe { device.bind_image_memory(allocation.image, allocation.memory, 0) }
            .map_err(|e| e.to_string())?;
        for semaphore in &mut allocation.semaphores {
            let mut export = vk::ExportSemaphoreCreateInfo::default()
                .handle_types(vk::ExternalSemaphoreHandleTypeFlags::OPAQUE_FD);
            *semaphore = unsafe {
                device.create_semaphore(
                    &vk::SemaphoreCreateInfo::default().push_next(&mut export),
                    None,
                )
            }
            .map_err(|e| e.to_string())?;
        }
        let allocation = Arc::new(allocation);
        let owner = allocation.clone();
        let desc = wgpu::TextureDescriptor {
            label: Some("media-shared-frame"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        };
        let texture = unsafe {
            let raw = hal.texture_from_raw(
                allocation.image,
                &wgpu::hal::TextureDescriptor {
                    label: desc.label,
                    size: desc.size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: desc.dimension,
                    format: desc.format,
                    usage: wgpu::TextureUses::COPY_SRC | wgpu::TextureUses::COLOR_TARGET,
                    memory_flags: wgpu::hal::MemoryFlags::empty(),
                    view_formats: vec![],
                },
                Some(Box::new(move || drop(owner))),
                wgpu::hal::vulkan::TextureMemory::External,
            );
            state
                .device
                .create_texture_from_hal::<wgpu::hal::api::Vulkan>(
                    raw,
                    &desc,
                    wgpu::TextureUses::UNINITIALIZED,
                )
        };
        let shared = Arc::new(Self {
            allocation,
            texture,
            size,
            memory_size: requirements.size,
            family: hal.queue_family_index(),
            memory_fd: khr::external_memory_fd::Device::new(instance, device),
            semaphore_fd: khr::external_semaphore_fd::Device::new(instance, device),
        });
        drop(hal);
        let mut encoder = state.device.create_command_encoder(&Default::default());
        // Establish both initialized contents and the tracked Vulkan layout
        // before the first external write. MPV may not have a decoded frame yet.
        {
            let view = shared.texture.create_view(&Default::default());
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
        }
        encoder.transition_resources(
            std::iter::empty(),
            std::iter::once(wgpu::TextureTransition {
                texture: &shared.texture,
                selector: None,
                state: wgpu::TextureUses::COPY_SRC,
            }),
        );
        let release = shared.ownership_command(state, false);
        unsafe { state.queue.as_hal::<wgpu::hal::api::Vulkan>() }
            .unwrap()
            .add_signal_semaphore(shared.allocation.semaphores[0], None);
        state.queue.submit([encoder.finish(), release]);
        Ok(shared)
    }

    fn ownership_command(&self, state: &RenderState, acquire: bool) -> wgpu::CommandBuffer {
        // wgpu deliberately separates raw encoding from its tracked encoding.
        // Ownership commands and the tracked copy are submitted in one batch.
        let mut encoder = state.device.create_command_encoder(&Default::default());
        let (source, destination, src_access, dst_access) = if acquire {
            (
                vk::QUEUE_FAMILY_EXTERNAL,
                self.family,
                vk::AccessFlags::empty(),
                vk::AccessFlags::TRANSFER_READ,
            )
        } else {
            (
                self.family,
                vk::QUEUE_FAMILY_EXTERNAL,
                vk::AccessFlags::TRANSFER_READ,
                vk::AccessFlags::empty(),
            )
        };
        let barrier = vk::ImageMemoryBarrier::default()
            .image(self.allocation.image)
            .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .src_queue_family_index(source)
            .dst_queue_family_index(destination)
            .src_access_mask(src_access)
            .dst_access_mask(dst_access)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .level_count(1)
                    .layer_count(1),
            );
        unsafe {
            encoder.as_hal_mut::<wgpu::hal::api::Vulkan, _, _>(|encoder| {
                self.allocation.device.cmd_pipeline_barrier(
                    encoder.unwrap().raw_handle(),
                    vk::PipelineStageFlags::ALL_COMMANDS,
                    vk::PipelineStageFlags::ALL_COMMANDS,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[barrier],
                );
            });
        }
        encoder.finish()
    }

    pub fn copy_to(&self, state: &RenderState, output: &wgpu::Texture) -> Result<(), String> {
        let mut encoder = state.device.create_command_encoder(&Default::default());
        let acquire = self.ownership_command(state, true);
        encoder.copy_texture_to_texture(
            self.texture.as_image_copy(),
            output.as_image_copy(),
            self.texture.size(),
        );
        let release = self.ownership_command(state, false);
        {
            let queue = unsafe { state.queue.as_hal::<wgpu::hal::api::Vulkan>() }.unwrap();
            queue.add_wait_semaphore(
                self.allocation.semaphores[1],
                None,
                vk::PipelineStageFlags::ALL_COMMANDS,
            );
            queue.add_signal_semaphore(self.allocation.semaphores[0], None);
        }
        state.queue.submit([acquire, encoder.finish(), release]);
        Ok(())
    }
}

pub struct GlImage {
    pub shared: Arc<SharedImage>,
    pub framebuffer: glow::NativeFramebuffer,
    texture: glow::NativeTexture,
    memory: u32,
    semaphores: [u32; 2],
}

impl GlImage {
    pub fn new(context: &GlContext, shared: Arc<SharedImage>) -> Result<Self, String> {
        context.check()?;
        let gl = &context.gl;
        let external = &context.external;
        let framebuffer = unsafe { gl.create_framebuffer() }?;
        let texture = match unsafe { gl.create_texture() } {
            Ok(texture) => texture,
            Err(error) => {
                unsafe { gl.delete_framebuffer(framebuffer) };
                return Err(error);
            }
        };
        let mut image = Self {
            shared,
            framebuffer,
            texture,
            memory: 0,
            semaphores: [0; 2],
        };
        let result = (|| unsafe {
            (external.create_memory)(1, &mut image.memory);
            (external.memory_parameter)(image.memory, 0x9581, &1); // dedicated allocation
            context.check()?;
            let fd = OwnedFd::from_raw_fd(
                image
                    .shared
                    .memory_fd
                    .get_memory_fd(
                        &vk::MemoryGetFdInfoKHR::default()
                            .memory(image.shared.allocation.memory)
                            .handle_type(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD),
                    )
                    .map_err(|e| e.to_string())?,
            );
            (external.import_memory)(
                image.memory,
                image.shared.memory_size,
                OPAQUE_FD,
                fd.as_raw_fd(),
            );
            context.check()?;
            let _ = fd.into_raw_fd(); // successful imports transfer FD ownership to GL
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            (external.texture_storage)(
                glow::TEXTURE_2D,
                1,
                glow::RGBA8,
                image.shared.size[0] as i32,
                image.shared.size[1] as i32,
                image.memory,
                0,
            );
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            let complete =
                gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE;
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            context.check()?;
            if !complete {
                return Err("Shared video framebuffer is incomplete".into());
            }
            (external.create_semaphores)(2, image.semaphores.as_mut_ptr());
            context.check()?;
            for (semaphore, raw) in image
                .semaphores
                .iter()
                .zip(image.shared.allocation.semaphores)
            {
                let fd = OwnedFd::from_raw_fd(
                    image
                        .shared
                        .semaphore_fd
                        .get_semaphore_fd(
                            &vk::SemaphoreGetFdInfoKHR::default()
                                .semaphore(raw)
                                .handle_type(vk::ExternalSemaphoreHandleTypeFlags::OPAQUE_FD),
                        )
                        .map_err(|e| e.to_string())?,
                );
                (external.import_semaphore)(*semaphore, OPAQUE_FD, fd.as_raw_fd());
                context.check()?;
                let _ = fd.into_raw_fd();
            }
            Ok(())
        })();
        if let Err(error) = result {
            image.destroy(context);
            return Err(error);
        }
        Ok(image)
    }

    pub fn wait(&self, context: &GlContext) {
        unsafe {
            (context.external.wait)(
                self.semaphores[0],
                0,
                std::ptr::null(),
                1,
                &self.texture.0.get(),
                &TRANSFER_SRC,
            )
        };
    }

    pub fn signal(&self, context: &GlContext) {
        unsafe {
            (context.external.signal)(
                self.semaphores[1],
                0,
                std::ptr::null(),
                1,
                &self.texture.0.get(),
                &TRANSFER_SRC,
            );
            context.gl.flush();
        }
    }

    // Called only on the GL worker, while its context is current. Finish is
    // confined to resize/teardown; no per-frame CPU wait is needed.
    pub fn destroy(self, context: &GlContext) {
        unsafe {
            context.gl.finish();
            context.gl.delete_framebuffer(self.framebuffer);
            context.gl.delete_texture(self.texture);
            (context.external.delete_semaphores)(2, self.semaphores.as_ptr());
            (context.external.delete_memory)(1, &self.memory);
        }
    }
}
