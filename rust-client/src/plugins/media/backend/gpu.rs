//! D3D11 video -> shared GPU texture -> egui/wgpu. No CPU pixel transfers.
use eframe::{egui, egui_wgpu::RenderState, wgpu};
use libmpv_sys as mpv;
use std::{
    ffi::c_void,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
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

// Added in recent libmpv; libmpv-sys' bundled headers predate these stable ABI entries.
const D3D11_INIT: u32 = 21;
const D3D11_TARGET: u32 = 22;

fn param(kind: u32, data: *mut c_void) -> mpv::mpv_render_param {
    mpv::mpv_render_param { type_: kind, data }
}
fn end() -> mpv::mpv_render_param {
    param(0, std::ptr::null_mut())
}
fn check(code: i32) -> Result<(), String> {
    if code >= 0 {
        Ok(())
    } else {
        Err(format!("Video GPU renderer: {}", unsafe {
            std::ffi::CStr::from_ptr(mpv::mpv_error_string(code)).to_string_lossy()
        }))
    }
}

struct SharedHandle(HANDLE);
impl Drop for SharedHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

struct Wake {
    ctx: egui::Context,
    visible: AtomicBool,
    dirty: AtomicBool,
}
unsafe extern "C" fn render_wakeup(data: *mut c_void) {
    let wake = unsafe { &*(data as *const Wake) };
    wake.dirty.store(true, Ordering::Release);
    if wake.visible.load(Ordering::Acquire) {
        wake.ctx.request_repaint();
    }
}

enum Command {
    Draw(
        ID3D11Texture2D,
        u64,
        bool,
        mpsc::SyncSender<Result<bool, String>>,
    ),
    Stop,
}

struct Surface {
    d3d11: ID3D11Texture2D,
    texture: wgpu::Texture,
    id: egui::TextureId,
    size: [u32; 2],
}

pub(super) struct VideoRenderer {
    worker: Option<thread::JoinHandle<()>>,
    commands: mpsc::Sender<Command>,
    wake: Arc<Wake>,
    state: RenderState,
    device11: ID3D11Device,
    device12: ID3D12Device,
    queue12: ID3D12CommandQueue,
    fence12: ID3D12Fence,
    serial: u64,
    surface: Option<Surface>,
}

impl VideoRenderer {
    pub(super) fn new(
        handle: *mut mpv::mpv_handle,
        state: &RenderState,
        ctx: &egui::Context,
        _display: Option<super::VideoDisplay>,
    ) -> Result<Self, String> {
        // Both APIs must use the same adapter. Never select a second GPU implicitly.
        let hal = unsafe { state.device.as_hal::<wgpu::hal::api::Dx12>() }
            .ok_or("Video GPU rendering requires the Direct3D 12 UI backend")?;
        let device12 = hal.raw_device().clone();
        let queue12 = hal.raw_queue().clone();
        drop(hal);
        let (device11, context11, fence12, fence11) = unsafe {
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
                let device = device.unwrap();
                let context = context.unwrap();
                let multithread: ID3D11Multithread = context.cast()?;
                let _ = multithread.SetMultithreadProtected(true);
                let fence: ID3D12Fence = device12.CreateFence(0, D3D12_FENCE_FLAG_SHARED)?;
                let shared =
                    SharedHandle(device12.CreateSharedHandle(&fence, None, 0x10000000, None)?);
                let device5: ID3D11Device5 = device.cast()?;
                let mut fence11: Option<ID3D11Fence> = None;
                device5.OpenSharedFence(shared.0, &mut fence11)?;
                Ok((
                    device,
                    context.cast::<ID3D11DeviceContext4>()?,
                    fence,
                    fence11.unwrap(),
                ))
            })()
        }
        .map_err(|e| e.to_string())?;
        let wake = Arc::new(Wake {
            ctx: ctx.clone(),
            visible: AtomicBool::new(false),
            dirty: AtomicBool::new(true),
        });
        let (commands, receiver) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let worker_device = device11.clone();
        let worker_wake = wake.clone();
        // The mpv core outlives this worker; Drop joins it before destroying the core.
        let handle = handle as usize;
        let worker = thread::Builder::new()
            .name("media-gpu".into())
            .spawn(move || {
                unsafe {
                    let mut context = std::ptr::null_mut();
                    let mut device_ptr = worker_device.as_raw();
                    let mut params = [
                        param(1, c"d3d11".as_ptr() as *mut _),
                        param(D3D11_INIT, &mut device_ptr as *mut _ as *mut _),
                        end(),
                    ];
                    let result = check(mpv::mpv_render_context_create(
                        &mut context,
                        handle as *mut _,
                        params.as_mut_ptr(),
                    ));
                    let ok = result.is_ok();
                    let _ = ready_tx.send(result);
                    if !ok {
                        return;
                    }
                    mpv::mpv_render_context_set_update_callback(
                        context,
                        Some(render_wakeup),
                        Arc::as_ptr(&worker_wake) as *mut _,
                    );
                    loop {
                        match receiver.recv_timeout(std::time::Duration::from_millis(50)) {
                            Ok(Command::Draw(texture, serial, force, reply)) => {
                                worker_wake.dirty.store(false, Ordering::Release);
                                let changed = mpv::mpv_render_context_update(context) & 1 != 0;
                                let result = if changed || force {
                                    (|| -> Result<bool, String> {
                                        context11
                                            .Wait(&fence11, serial)
                                            .map_err(|e| e.to_string())?;
                                        let mut texture_ptr = texture.as_raw();
                                        let mut block = 0i32;
                                        let mut params = [
                                            param(
                                                D3D11_TARGET,
                                                &mut texture_ptr as *mut _ as *mut _,
                                            ),
                                            param(12, &mut block as *mut _ as *mut _),
                                            end(),
                                        ];
                                        let rendered = check(mpv::mpv_render_context_render(
                                            context,
                                            params.as_mut_ptr(),
                                        ));
                                        // Signal even on render failure, so the UI queue cannot wait forever.
                                        context11
                                            .Signal(&fence11, serial + 1)
                                            .map_err(|e| e.to_string())?;
                                        context11.Flush();
                                        rendered.map(|_| true)
                                    })()
                                } else {
                                    Ok(false)
                                };
                                let _ = reply.send(result);
                            }
                            Ok(Command::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                            Err(mpsc::RecvTimeoutError::Timeout) => {
                                if !worker_wake.visible.load(Ordering::Acquire)
                                    && worker_wake.dirty.swap(false, Ordering::AcqRel)
                                    && mpv::mpv_render_context_update(context) & 1 != 0
                                {
                                    let mut skip = 1i32;
                                    let mut params =
                                        [param(13, &mut skip as *mut _ as *mut _), end()];
                                    mpv::mpv_render_context_render(context, params.as_mut_ptr());
                                }
                            }
                        }
                    }
                    mpv::mpv_render_context_set_update_callback(
                        context,
                        None,
                        std::ptr::null_mut(),
                    );
                    mpv::mpv_render_context_free(context);
                }
            })
            .map_err(|e| e.to_string())?;
        match ready_rx.recv().map_err(|e| e.to_string())? {
            Ok(()) => {}
            Err(error) => {
                let _ = worker.join();
                return Err(error);
            }
        }
        log::info!("Media GPU renderer: D3D11 shared texture on the UI's D3D12 adapter");
        Ok(Self {
            worker: Some(worker),
            commands,
            wake,
            state: state.clone(),
            device11,
            device12,
            queue12,
            fence12,
            serial: 0,
            surface: None,
        })
    }

    pub(super) fn set_visible(&self, visible: bool) {
        if self.wake.visible.swap(visible, Ordering::AcqRel) != visible {
            self.wake.dirty.store(true, Ordering::Release);
        }
    }

    fn create_surface(&self, size: [u32; 2]) -> Result<Surface, String> {
        let (d3d11, resource) = unsafe {
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
                let mut view = None;
                self.device11
                    .CreateRenderTargetView(&texture, None, Some(&mut view))?;
                let context = self.device11.GetImmediateContext()?;
                context.ClearRenderTargetView(view.as_ref().unwrap(), &[0.0, 0.0, 0.0, 1.0]);
                context.Flush();
                let dxgi: IDXGIResource1 = texture.cast()?;
                let shared = SharedHandle(dxgi.CreateSharedHandle(
                    None,
                    DXGI_SHARED_RESOURCE_READ.0 | DXGI_SHARED_RESOURCE_WRITE.0,
                    None,
                )?);
                let mut resource: Option<ID3D12Resource> = None;
                self.device12.OpenSharedHandle(shared.0, &mut resource)?;
                Ok((texture, resource.unwrap()))
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
            self.state
                .device
                .create_texture_from_hal::<wgpu::hal::api::Dx12>(
                    hal,
                    &wgpu::TextureDescriptor {
                        label: Some("media-video"),
                        size: extent,
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        usage: wgpu::TextureUsages::TEXTURE_BINDING
                            | wgpu::TextureUsages::RENDER_ATTACHMENT,
                        view_formats: &[],
                    },
                    wgpu::TextureUses::PRESENT,
                )
        };
        let id = self.state.renderer.write().register_native_texture(
            &self.state.device,
            &texture.create_view(&Default::default()),
            wgpu::FilterMode::Linear,
        );
        Ok(Surface {
            d3d11,
            texture,
            id,
            size,
        })
    }

    pub(super) fn frame(&mut self, size: [u32; 2]) -> Result<egui::TextureId, String> {
        let size = size.map(|n| n.clamp(1, 8192));
        let resized = self.surface.as_ref().is_none_or(|s| s.size != size);
        if resized {
            let surface = self.create_surface(size)?;
            if let Some(old) = self.surface.replace(surface) {
                self.state.renderer.write().free_texture(&old.id);
            }
        }
        let was_visible = self.wake.visible.swap(true, Ordering::AcqRel);
        let force = resized || !was_visible;
        let surface = self.surface.as_ref().unwrap();
        if !force && !self.wake.dirty.load(Ordering::Acquire) {
            return Ok(surface.id);
        }
        // Return ownership to D3D11 in COMMON state after all earlier UI reads.
        let mut encoder = self
            .state
            .device
            .create_command_encoder(&Default::default());
        encoder.transition_resources(
            std::iter::empty(),
            std::iter::once(wgpu::TextureTransition {
                texture: &surface.texture,
                selector: None,
                state: wgpu::TextureUses::PRESENT,
            }),
        );
        self.state.queue.submit([encoder.finish()]);
        self.serial += 2;
        unsafe {
            self.queue12
                .Signal(&self.fence12, self.serial)
                .map_err(|e| e.to_string())?;
        }
        let (tx, rx) = mpsc::sync_channel(1);
        self.commands
            .send(Command::Draw(surface.d3d11.clone(), self.serial, force, tx))
            .map_err(|e| e.to_string())?;
        let drawn = rx.recv().map_err(|e| e.to_string())??;
        if drawn {
            // GPU wait only; no readback and no CPU wait for GPU completion.
            unsafe {
                self.queue12
                    .Wait(&self.fence12, self.serial + 1)
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(surface.id)
    }
}

impl Drop for VideoRenderer {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if let Some(surface) = self.surface.take() {
            self.state.renderer.write().free_texture(&surface.id);
        }
    }
}
