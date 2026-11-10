//! D3D11 video producer with a D3D12 shared image and GPU timeline handoff.
use super::{RenderContext, end, param};
use eframe::{egui_wgpu::RenderState, wgpu};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use windows_gpu::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, HMODULE},
        Graphics::{
            Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0},
            Direct3D11::*,
            Direct3D12::*,
            Dxgi::{Common::*, *},
        },
    },
    core::Interface,
};

// Stable libmpv ABI entries newer than libmpv-sys' bundled headers.
const D3D11_INIT: u32 = 21;
const D3D11_TARGET: u32 = 22;

struct SharedHandle(HANDLE);
impl Drop for SharedHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

#[derive(Clone)]
pub struct Device {
    device11: ID3D11Device,
    device12: ID3D12Device,
    queue12: ID3D12CommandQueue,
}
impl Device {
    pub fn new(state: &RenderState) -> Result<Self, String> {
        let hal = unsafe { state.device.as_hal::<wgpu::hal::api::Dx12>() }
            .ok_or("Video GPU rendering requires the Direct3D 12 UI backend")?;
        let device12 = hal.raw_device().clone();
        let queue12 = hal.raw_queue().clone();
        drop(hal);
        // The producer must use the UI adapter; sharing never crosses GPUs.
        let device11 = unsafe {
            (|| -> windows_gpu::core::Result<_> {
                let factory: IDXGIFactory4 = CreateDXGIFactory1()?;
                let adapter: IDXGIAdapter = factory.EnumAdapterByLuid(device12.GetAdapterLuid())?;
                let mut device = None;
                let mut context = None;
                D3D11CreateDevice(
                    &adapter,
                    D3D_DRIVER_TYPE_UNKNOWN,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                    Some(&[D3D_FEATURE_LEVEL_11_0]),
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    Some(&mut context),
                )?;
                let multithread: ID3D11Multithread = context.unwrap().cast()?;
                let _ = multithread.SetMultithreadProtected(true);
                Ok(device.unwrap())
            })()
        }
        .map_err(|e| e.to_string())?;
        Ok(Self {
            device11,
            device12,
            queue12,
        })
    }

    pub fn create_image(
        &self,
        state: &RenderState,
        size: [u32; 2],
    ) -> Result<Arc<SharedImage>, String> {
        let (texture11, resource, fence12, fence11) = unsafe {
            (|| -> windows_gpu::core::Result<_> {
                let mut texture = None;
                self.device11.CreateTexture2D(
                    &D3D11_TEXTURE2D_DESC {
                        Width: size[0],
                        Height: size[1],
                        MipLevels: 1,
                        ArraySize: 1,
                        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                        SampleDesc: DXGI_SAMPLE_DESC {
                            Count: 1,
                            Quality: 0,
                        },
                        Usage: D3D11_USAGE_DEFAULT,
                        BindFlags: (D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE).0 as u32,
                        MiscFlags: (D3D11_RESOURCE_MISC_SHARED
                            | D3D11_RESOURCE_MISC_SHARED_NTHANDLE)
                            .0 as u32,
                        ..Default::default()
                    },
                    None,
                    Some(&mut texture),
                )?;
                let texture = texture.unwrap();
                let dxgi: IDXGIResource1 = texture.cast()?;
                let handle = SharedHandle(dxgi.CreateSharedHandle(
                    None,
                    DXGI_SHARED_RESOURCE_READ.0 | DXGI_SHARED_RESOURCE_WRITE.0,
                    None,
                )?);
                let mut resource: Option<ID3D12Resource> = None;
                self.device12.OpenSharedHandle(handle.0, &mut resource)?;
                let fence12: ID3D12Fence = self.device12.CreateFence(0, D3D12_FENCE_FLAG_SHARED)?;
                let handle = SharedHandle(
                    self.device12
                        .CreateSharedHandle(&fence12, None, 0x10000000, None)?,
                );
                let device5: ID3D11Device5 = self.device11.cast()?;
                let mut fence11: Option<ID3D11Fence> = None;
                device5.OpenSharedFence(handle.0, &mut fence11)?;
                Ok((texture, resource.unwrap(), fence12, fence11.unwrap()))
            })()
        }
        .map_err(|e| e.to_string())?;
        let extent = wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        };
        let texture = unsafe {
            let hal = wgpu::hal::dx12::Device::texture_from_raw(
                resource,
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureDimension::D2,
                extent,
                1,
                1,
            );
            state
                .device
                .create_texture_from_hal::<wgpu::hal::api::Dx12>(
                    hal,
                    &wgpu::TextureDescriptor {
                        label: Some("media-shared-frame"),
                        size: extent,
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        usage: wgpu::TextureUsages::COPY_SRC
                            | wgpu::TextureUsages::RENDER_ATTACHMENT,
                        view_formats: &[],
                    },
                    wgpu::TextureUses::UNINITIALIZED,
                )
        };
        let shared = Arc::new(SharedImage {
            texture,
            size,
            texture11,
            fence11,
            fence12,
            queue12: self.queue12.clone(),
            available: AtomicU64::new(2),
        });
        // Initialize on the UI queue, then publish COMMON ownership to D3D11.
        let mut encoder = state.device.create_command_encoder(&Default::default());
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
        shared.release(&mut encoder);
        state.queue.submit([encoder.finish()]);
        unsafe { shared.queue12.Signal(&shared.fence12, 2) }.map_err(|e| e.to_string())?;
        Ok(shared)
    }
}

pub struct SharedImage {
    pub texture: wgpu::Texture,
    pub size: [u32; 2],
    texture11: ID3D11Texture2D,
    fence11: ID3D11Fence,
    fence12: ID3D12Fence,
    queue12: ID3D12CommandQueue,
    available: AtomicU64,
}
impl SharedImage {
    fn release(&self, encoder: &mut wgpu::CommandEncoder) {
        encoder.transition_resources(
            std::iter::empty(),
            std::iter::once(wgpu::TextureTransition {
                texture: &self.texture,
                selector: None,
                state: wgpu::TextureUses::PRESENT,
            }),
        );
    }
    pub fn copy_to(&self, state: &RenderState, output: &wgpu::Texture) -> Result<(), String> {
        let serial = self.available.load(Ordering::Acquire);
        // The worker has already submitted this signal. No CPU wait or readback.
        unsafe { self.queue12.Wait(&self.fence12, serial + 1) }.map_err(|e| e.to_string())?;
        let mut encoder = state.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_texture(
            self.texture.as_image_copy(),
            output.as_image_copy(),
            self.texture.size(),
        );
        self.release(&mut encoder);
        state.queue.submit([encoder.finish()]);
        unsafe { self.queue12.Signal(&self.fence12, serial + 2) }.map_err(|e| e.to_string())?;
        self.available.store(serial + 2, Ordering::Release);
        Ok(())
    }
}

pub struct Renderer {
    pub context: RenderContext,
    context11: ID3D11DeviceContext4,
    last: Option<(Arc<SharedImage>, u64)>,
}
impl Renderer {
    pub fn new(
        handle: usize,
        device: Device,
        _: Option<super::super::VideoDisplay>,
    ) -> Result<Self, String> {
        let context11 = unsafe { device.device11.GetImmediateContext() }
            .and_then(|context| context.cast())
            .map_err(|e| e.to_string())?;
        let mut device_ptr = device.device11.as_raw();
        let mut params = [
            param(1, c"d3d11".as_ptr() as *mut _),
            param(D3D11_INIT, &mut device_ptr as *mut _ as *mut _),
            end(),
        ];
        let context = RenderContext::new(handle, &mut params)?;
        log::info!("Media GPU renderer: independent D3D11 producer, D3D12 shared-memory transfer");
        Ok(Self {
            context,
            context11,
            last: None,
        })
    }
    pub fn draw(&mut self, shared: Arc<SharedImage>, changed: bool) -> Result<(), String> {
        let serial = shared.available.load(Ordering::Acquire);
        unsafe { self.context11.Wait(&shared.fence11, serial) }.map_err(|e| e.to_string())?;
        let result = if changed {
            let mut texture = shared.texture11.as_raw();
            self.context
                .render(param(D3D11_TARGET, &mut texture as *mut _ as *mut _))
        } else {
            Ok(())
        };
        // Complete the handoff even if mpv reports a render error.
        unsafe { self.context11.Signal(&shared.fence11, serial + 1) }.map_err(|e| e.to_string())?;
        unsafe {
            self.context11.Flush();
        }
        self.last = Some((shared, serial + 1));
        result
    }
}
impl Drop for Renderer {
    fn drop(&mut self) {
        self.context.close();
        // Only teardown waits for GPU completion, on the producer's own thread.
        if let Some((shared, ready)) = &self.last {
            let _ = unsafe {
                shared
                    .fence12
                    .SetEventOnCompletion(*ready, HANDLE::default())
            };
        }
    }
}
