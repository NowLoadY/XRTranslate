//! One event-driven video worker and bounded GPU handoff on every desktop.
//! The UI keeps displaying its own texture while the producer prepares a frame.
#[cfg(target_os = "linux")]
#[path = "gpu_linux.rs"]
mod platform;
#[cfg(windows)]
#[path = "gpu_windows.rs"]
mod platform;
#[cfg(target_os = "linux")]
pub use platform::graphics_setup;

use eframe::{egui, egui_wgpu::RenderState, wgpu};
use libmpv_sys as mpv;
use std::{
    cell::Cell,
    ffi::c_void,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

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

struct RenderContext(*mut mpv::mpv_render_context);
impl RenderContext {
    fn new(handle: usize, params: &mut [mpv::mpv_render_param]) -> Result<Self, String> {
        let mut context = std::ptr::null_mut();
        check(unsafe {
            mpv::mpv_render_context_create(&mut context, handle as *mut _, params.as_mut_ptr())
        })?;
        Ok(Self(context))
    }
    fn update(&self) -> bool {
        unsafe { mpv::mpv_render_context_update(self.0) & 1 != 0 }
    }
    fn render(&self, target: mpv::mpv_render_param) -> Result<(), String> {
        let mut block = 0i32;
        let mut params = [target, param(12, &mut block as *mut _ as *mut _), end()];
        check(unsafe { mpv::mpv_render_context_render(self.0, params.as_mut_ptr()) })
    }
    fn skip(&self) -> Result<(), String> {
        let mut skip = 1i32;
        self.render(param(13, &mut skip as *mut _ as *mut _))
    }
    fn close(&mut self) {
        if !self.0.is_null() {
            unsafe {
                mpv::mpv_render_context_set_update_callback(self.0, None, std::ptr::null_mut());
                mpv::mpv_render_context_free(self.0);
            }
            self.0 = std::ptr::null_mut();
        }
    }
}
impl Drop for RenderContext {
    fn drop(&mut self) {
        self.close();
    }
}

struct Wake {
    ctx: egui::Context,
    visible: AtomicBool,
    dirty: AtomicBool,
    commands: mpsc::Sender<Command>,
}
unsafe extern "C" fn render_wakeup(data: *mut c_void) {
    let wake = unsafe { &*(data as *const Wake) };
    let already_dirty = wake.dirty.swap(true, Ordering::AcqRel);
    if wake.visible.load(Ordering::Acquire) {
        wake.ctx.request_repaint();
    } else if !already_dirty {
        let _ = wake.commands.send(Command::Update);
    }
}

enum Command {
    Draw(
        Arc<platform::SharedImage>,
        bool,
        mpsc::SyncSender<Result<(), String>>,
    ),
    Update,
    Stop,
}

struct Surface {
    shared: Arc<platform::SharedImage>,
    output: wgpu::Texture,
    id: egui::TextureId,
}

pub struct VideoRenderer {
    worker: Option<thread::JoinHandle<()>>,
    wake: Arc<Wake>,
    state: RenderState,
    device: platform::Device,
    refresh: Cell<bool>,
    surface: Option<Surface>,
    pending: Option<mpsc::Receiver<Result<(), String>>>,
}

impl VideoRenderer {
    pub fn new(
        handle: *mut mpv::mpv_handle,
        state: &RenderState,
        ctx: &egui::Context,
        display: Option<super::VideoDisplay>,
    ) -> Result<Self, String> {
        let device = platform::Device::new(state)?;
        let producer = device.clone();
        let (commands, receiver) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let wake = Arc::new(Wake {
            ctx: ctx.clone(),
            visible: AtomicBool::new(false),
            dirty: AtomicBool::new(false),
            commands,
        });
        let worker_wake = wake.clone();
        let handle = handle as usize;
        // The mpv core outlives this worker. Drop joins before destroying the core.
        let worker = thread::Builder::new()
            .name("media-gpu".into())
            .spawn(move || {
                let mut renderer = match platform::Renderer::new(handle, producer, display) {
                    Ok(renderer) => renderer,
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                        return;
                    }
                };
                unsafe {
                    mpv::mpv_render_context_set_update_callback(
                        renderer.context.0,
                        Some(render_wakeup),
                        Arc::as_ptr(&worker_wake) as *mut _,
                    );
                }
                let _ = ready_tx.send(Ok(()));
                while let Ok(command) = receiver.recv() {
                    match command {
                        Command::Draw(shared, force, reply) => {
                            worker_wake.dirty.store(false, Ordering::Release);
                            let changed = renderer.context.update();
                            let result = renderer.draw(shared, changed || force);
                            let failed = result.is_err();
                            let _ = reply.send(result);
                            if worker_wake.visible.load(Ordering::Acquire) {
                                worker_wake.ctx.request_repaint();
                            }
                            // A failed handoff must never be reused with consumed sync objects.
                            if failed {
                                break;
                            }
                        }
                        Command::Update => {
                            if !worker_wake.visible.load(Ordering::Acquire)
                                && worker_wake.dirty.swap(false, Ordering::AcqRel)
                                && renderer.context.update()
                                && let Err(error) = renderer.context.skip()
                            {
                                log::warn!("Hidden video update: {error}");
                                break;
                            }
                        }
                        Command::Stop => break,
                    }
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
        Ok(Self {
            worker: Some(worker),
            wake,
            state: state.clone(),
            device,
            refresh: Cell::new(true),
            surface: None,
            pending: None,
        })
    }

    pub fn set_visible(&self, visible: bool) {
        if self.wake.visible.swap(visible, Ordering::AcqRel) != visible {
            self.refresh.set(true);
            self.wake.dirty.store(true, Ordering::Release);
            if !visible {
                let _ = self.wake.commands.send(Command::Update);
            }
        }
    }

    fn create_surface(&self, size: [u32; 2]) -> Result<Surface, String> {
        let shared = self.device.create_image(&self.state, size)?;
        let output = self.state.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("media-video"),
            size: shared.texture.size(),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let id = self.state.renderer.write().register_native_texture(
            &self.state.device,
            &output.create_view(&Default::default()),
            wgpu::FilterMode::Linear,
        );
        Ok(Surface { shared, output, id })
    }

    fn retire(&self, surface: Surface) {
        self.state.renderer.write().free_texture(&surface.id);
        // External fences/images must outlive the queued copy, including resize.
        self.state
            .queue
            .on_submitted_work_done(move || drop(surface));
    }

    pub fn frame(&mut self, size: [u32; 2]) -> Result<egui::TextureId, String> {
        let was_visible = self.wake.visible.swap(true, Ordering::AcqRel);
        if !was_visible {
            self.refresh.set(true);
            self.wake.dirty.store(true, Ordering::Release);
        }
        if let Some(pending) = &self.pending {
            match pending.try_recv() {
                Ok(result) => {
                    self.pending = None;
                    result?;
                    let surface = self.surface.as_ref().unwrap();
                    surface.shared.copy_to(&self.state, &surface.output)?;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    return Ok(self.surface.as_ref().expect("pending video frame").id);
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending = None;
                    return Err("Video rendering stopped unexpectedly".into());
                }
            }
        }
        let limit = self
            .state
            .device
            .limits()
            .max_texture_dimension_2d
            .min(8192);
        let size = size.map(|value| value.clamp(1, limit));
        let resized = self
            .surface
            .as_ref()
            .is_none_or(|surface| surface.shared.size != size);
        if resized {
            let surface = self.create_surface(size)?;
            if let Some(old) = self.surface.replace(surface) {
                self.retire(old);
            }
        }
        let force = self.refresh.replace(false) || resized;
        let surface = self.surface.as_ref().unwrap();
        if force || self.wake.dirty.load(Ordering::Acquire) {
            let (tx, rx) = mpsc::sync_channel(1);
            self.wake
                .commands
                .send(Command::Draw(surface.shared.clone(), force, tx))
                .map_err(|e| e.to_string())?;
            self.pending = Some(rx);
        }
        Ok(surface.id)
    }
}

impl Drop for VideoRenderer {
    fn drop(&mut self) {
        let _ = self.wake.commands.send(Command::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if let Some(surface) = self.surface.take() {
            self.retire(surface);
        }
    }
}
