//! SteamVR's GPU and extension requirements applied before Vulkan creation.
use super::openvr::{AvatarTracking, VulkanTexture};
use crate::ui::components::avatar::StereoRenderer;
use ash::{vk, vk::Handle};
use eframe::wgpu::{
    self,
    hal::{api::Vulkan, vulkan},
};
use std::{
    ffi::{CStr, CString},
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

pub(super) fn create(tracking: &AvatarTracking) -> Result<StereoRenderer, String> {
    // This dedicated queue is touched only by the overlay worker, including
    // Valve's submission. Desktop rendering never concurrently uses it.
    let entry = unsafe { ash::Entry::load() }.map_err(|e| e.to_string())?;
    let version = unsafe { entry.try_enumerate_instance_version() }
        .map_err(|e| e.to_string())?
        .unwrap_or(vk::API_VERSION_1_0)
        .min(vk::API_VERSION_1_3);
    if version < vk::API_VERSION_1_1 {
        return Err("Vulkan 1.1 is required for the VR companion".into());
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
        .application_name(c"XRTranslate Companion")
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
        .ok_or("SteamVR GPU is unavailable for companion rendering")?;
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
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let renderer = StereoRenderer::new(device.clone(), queue);
    if let Some(error) = futures::executor::block_on(validation.pop()) {
        return Err(error.to_string());
    }
    Ok(renderer)
}

pub(super) fn texture(renderer: &StereoRenderer) -> Result<VulkanTexture, String> {
    let device =
        unsafe { renderer.device.as_hal::<Vulkan>() }.ok_or("Companion GPU is unavailable")?;
    let texture =
        unsafe { renderer.texture.as_hal::<Vulkan>() }.ok_or("Companion texture is unavailable")?;
    Ok(VulkanTexture {
        image: unsafe { texture.raw_handle() }.as_raw(),
        device: device.raw_device().handle().as_raw() as *mut _,
        physical_device: device.raw_physical_device().as_raw() as *mut _,
        instance: device.shared_instance().raw_instance().handle().as_raw() as *mut _,
        queue: device.raw_queue().as_raw() as *mut _,
        queue_family: device.queue_family_index(),
        width: 1024,
        height: 512,
        format: vk::Format::R8G8B8A8_UNORM.as_raw() as u32,
        samples: 1,
    })
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
