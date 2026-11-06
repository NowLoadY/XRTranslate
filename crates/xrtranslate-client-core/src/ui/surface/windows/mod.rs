//! D3D12 shared resources. Each process owns its renderer and command queue.
mod host;
mod page;
mod video;
pub use host::Host;
pub use page::Page;

use eframe::{egui_wgpu::RenderState, wgpu};
use windows_gpu::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, LUID},
        Graphics::{Direct3D12::*, Dxgi::Common::*},
    },
    core::{Interface, PCWSTR},
};
use xrtranslate_plugin::surface::SharedTarget;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const ACCESS: u32 = 0x10000000; // GENERIC_ALL, protected by the current user's default DACL.

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn wide(name: &str) -> Vec<u16> {
    name.encode_utf16().chain(Some(0)).collect()
}
fn luid(id: LUID) -> u64 {
    (id.HighPart as u32 as u64) << 32 | id.LowPart as u64
}

struct Device {
    raw: ID3D12Device,
    queue: ID3D12CommandQueue,
}
impl Device {
    fn new(state: &RenderState) -> Result<Self, String> {
        let hal = unsafe { state.device.as_hal::<wgpu::hal::api::Dx12>() }
            .ok_or("Shared plugin surfaces require Direct3D 12")?;
        Ok(Self {
            raw: hal.raw_device().clone(),
            queue: hal.raw_queue().clone(),
        })
    }
    fn open<T: Interface>(&self, name: &str) -> Result<T, String> {
        let name = wide(name);
        unsafe {
            let handle = Handle(
                self.raw
                    .OpenSharedHandleByName(PCWSTR(name.as_ptr()), ACCESS)
                    .map_err(|e| e.to_string())?,
            );
            let mut resource = None;
            self.raw
                .OpenSharedHandle(handle.0, &mut resource)
                .map_err(|e| e.to_string())?;
            resource.ok_or_else(|| "Shared GPU resource was not opened".into())
        }
    }
    fn share<T: Interface>(&self, object: &T, name: &str) -> Result<Handle, String> {
        let name = wide(name);
        let object = object
            .cast::<ID3D12DeviceChild>()
            .map_err(|e| e.to_string())?;
        unsafe {
            self.raw
                .CreateSharedHandle(&object, None, ACCESS, PCWSTR(name.as_ptr()))
        }
        .map(Handle)
        .map_err(|e| e.to_string())
    }
    fn signal(&self, fence: &ID3D12Fence, value: u64) -> Result<(), String> {
        unsafe { self.queue.Signal(fence, value) }.map_err(|e| e.to_string())
    }
}

fn import(state: &RenderState, resource: ID3D12Resource, size: [u32; 2]) -> wgpu::Texture {
    let size = wgpu::Extent3d {
        width: size[0],
        height: size[1],
        depth_or_array_layers: 1,
    };
    // Resources cross queues only in COMMON (wgpu PRESENT) state, after a fence.
    unsafe {
        let texture = wgpu::hal::dx12::Device::texture_from_raw(
            resource,
            FORMAT,
            wgpu::TextureDimension::D2,
            size,
            1,
            1,
        );
        state
            .device
            .create_texture_from_hal::<wgpu::hal::api::Dx12>(
                texture,
                &wgpu::TextureDescriptor {
                    label: Some("plugin shared surface"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: FORMAT,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                },
                wgpu::TextureUses::PRESENT,
            )
    }
}

fn release(encoder: &mut wgpu::CommandEncoder, texture: &wgpu::Texture) {
    encoder.transition_resources(
        std::iter::empty(),
        std::iter::once(wgpu::TextureTransition {
            texture,
            selector: None,
            state: wgpu::TextureUses::PRESENT,
        }),
    );
}

fn validate(target: &SharedTarget) -> Result<(), String> {
    if target.slot > 1
        || target.size.iter().any(|n| *n == 0 || *n > 8192)
        || !target.name.starts_with("Local\\XRTranslate-")
        || target.name.len() > 100
        || target.released == 0
        || target.released % 2 != 0
    {
        return Err("Invalid shared plugin surface".into());
    }
    Ok(())
}
