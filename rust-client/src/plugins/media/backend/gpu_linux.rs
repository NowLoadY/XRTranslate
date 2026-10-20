//! libmpv renders into a wgpu-owned GL texture on the UI's EGL context.
use eframe::{egui, egui_wgpu::RenderState, glow, wgpu};
use glow::HasContext;
use libmpv_sys as mpv;
use std::{
    ffi::{CStr, c_char, c_void},
    num::NonZeroU32,
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
            CStr::from_ptr(mpv::mpv_error_string(code)).to_string_lossy()
        }))
    }
}

unsafe extern "C" fn get_proc_address(data: *mut c_void, name: *const c_char) -> *mut c_void {
    let context = unsafe { &*(data as *const wgpu::hal::gles::AdapterContext) };
    let name = unsafe { CStr::from_ptr(name) }.to_str().unwrap_or("");
    context
        .egl_instance()
        .and_then(|egl| egl.get_proc_address(name))
        .map_or(std::ptr::null_mut(), |proc| proc as *const () as *mut _)
}

// The same context serializes video and UI work, so no readback, GPU copies or
// cross-context fences are needed. Never call wgpu while this lock is held.
fn with_gl<T>(
    state: &RenderState,
    action: impl FnOnce(&glow::Context, &wgpu::hal::gles::AdapterContext) -> T,
) -> T {
    let hal = unsafe { state.device.as_hal::<wgpu::hal::api::Gles>() }.unwrap();
    let context = hal.context();
    let gl = context.lock();
    unsafe {
        // wgpu keeps its VAO bound between submissions. mpv owns a different VAO
        // and expects ordinary GL defaults; preserve wgpu's VAO and upload alignment.
        let vao = NonZeroU32::new(gl.get_parameter_i32(glow::VERTEX_ARRAY_BINDING) as u32)
            .map(glow::NativeVertexArray);
        let unpack = gl.get_parameter_i32(glow::UNPACK_ALIGNMENT);
        let pack = gl.get_parameter_i32(glow::PACK_ALIGNMENT);
        gl.bind_vertex_array(None);
        gl.use_program(None);
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, None);
        gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
        for cap in [
            glow::DEPTH_TEST,
            glow::STENCIL_TEST,
            glow::SCISSOR_TEST,
            glow::BLEND,
            glow::CULL_FACE,
        ] {
            gl.disable(cap);
        }
        let result = action(&gl, context);
        gl.bind_vertex_array(vao);
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, unpack);
        gl.pixel_store_i32(glow::PACK_ALIGNMENT, pack);
        gl.flush();
        result
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
    wake.dirty.store(true, Ordering::Release);
    if wake.visible.load(Ordering::Acquire) {
        wake.ctx.request_repaint();
    } else {
        let _ = wake.commands.send(Command::Update);
    }
}

enum Command {
    Draw(
        glow::NativeFramebuffer,
        [u32; 2],
        bool,
        mpsc::SyncSender<Result<(), String>>,
    ),
    Update,
    Stop,
}

struct Surface {
    framebuffer: glow::NativeFramebuffer,
    _texture: wgpu::Texture,
    id: egui::TextureId,
    size: [u32; 2],
}

pub(super) struct VideoRenderer {
    worker: Option<thread::JoinHandle<()>>,
    wake: Arc<Wake>,
    state: RenderState,
    surface: Option<Surface>,
}

impl VideoRenderer {
    pub(super) fn new(
        handle: *mut mpv::mpv_handle,
        state: &RenderState,
        ctx: &egui::Context,
        display: Option<super::VideoDisplay>,
    ) -> Result<Self, String> {
        let hal = unsafe { state.device.as_hal::<wgpu::hal::api::Gles>() }
            .ok_or("Video GPU rendering requires the OpenGL UI backend")?;
        if hal.context().egl_instance().is_none() {
            return Err("Video GPU rendering requires an EGL context".into());
        }
        drop(hal);
        let (commands, receiver) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let wake = Arc::new(Wake {
            ctx: ctx.clone(),
            visible: AtomicBool::new(false),
            dirty: AtomicBool::new(true),
            commands,
        });
        let worker_wake = wake.clone();
        let worker_state = state.clone();
        // Only the render thread calls mpv_render_*. It never calls blocking core
        // APIs, and is joined before the mpv core or UI device can be destroyed.
        let handle = handle as usize;
        let worker = thread::Builder::new()
            .name("media-gpu".into())
            .spawn(move || unsafe {
                let mut context = std::ptr::null_mut();
                let result = with_gl(&worker_state, |_, gl_context| {
                    let mut init = mpv::mpv_opengl_init_params {
                        get_proc_address: Some(get_proc_address),
                        get_proc_address_ctx: gl_context as *const _ as *mut _,
                        extra_exts: std::ptr::null(),
                    };
                    let mut params = vec![
                        param(1, c"opengl".as_ptr() as *mut _),
                        param(2, &mut init as *mut _ as *mut _),
                    ];
                    // The native display enables direct VA-API / EGL decoder interop.
                    match display {
                        Some(super::VideoDisplay::X11(ptr)) => params.push(param(8, ptr as *mut _)),
                        Some(super::VideoDisplay::Wayland(ptr)) => {
                            params.push(param(9, ptr as *mut _))
                        }
                        None => {}
                    }
                    params.push(end());
                    check(mpv::mpv_render_context_create(
                        &mut context,
                        handle as *mut _,
                        params.as_mut_ptr(),
                    ))
                });
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
                while let Ok(command) = receiver.recv() {
                    match command {
                        Command::Draw(framebuffer, size, force, reply) => {
                            let result = with_gl(&worker_state, |_, _| {
                                worker_wake.dirty.store(false, Ordering::Release);
                                let changed = mpv::mpv_render_context_update(context) & 1 != 0;
                                if !changed && !force {
                                    return Ok(());
                                }
                                let mut target = mpv::mpv_opengl_fbo {
                                    fbo: framebuffer.0.get() as i32,
                                    w: size[0] as i32,
                                    h: size[1] as i32,
                                    internal_format: glow::RGBA8 as i32,
                                };
                                // Texture coordinates start at the image's top left;
                                // only a GL window framebuffer needs FLIP_Y.
                                let mut block = 0i32;
                                let mut params = [
                                    param(3, &mut target as *mut _ as *mut _),
                                    param(12, &mut block as *mut _ as *mut _),
                                    end(),
                                ];
                                check(mpv::mpv_render_context_render(context, params.as_mut_ptr()))
                            });
                            let _ = reply.send(result);
                        }
                        Command::Update => {
                            if !worker_wake.visible.load(Ordering::Acquire)
                                && worker_wake.dirty.swap(false, Ordering::AcqRel)
                            {
                                with_gl(&worker_state, |_, _| {
                                    if mpv::mpv_render_context_update(context) & 1 != 0 {
                                        let mut skip = 1i32;
                                        let mut params =
                                            [param(13, &mut skip as *mut _ as *mut _), end()];
                                        mpv::mpv_render_context_render(
                                            context,
                                            params.as_mut_ptr(),
                                        );
                                    }
                                });
                            }
                        }
                        Command::Stop => break,
                    }
                }
                with_gl(&worker_state, |_, _| {
                    mpv::mpv_render_context_set_update_callback(
                        context,
                        None,
                        std::ptr::null_mut(),
                    );
                    mpv::mpv_render_context_free(context);
                });
            })
            .map_err(|e| e.to_string())?;
        match ready_rx.recv().map_err(|e| e.to_string())? {
            Ok(()) => {}
            Err(error) => {
                let _ = worker.join();
                return Err(error);
            }
        }
        log::info!("Media GPU renderer: OpenGL texture on the UI's EGL context");
        Ok(Self {
            worker: Some(worker),
            wake,
            state: state.clone(),
            surface: None,
        })
    }

    pub(super) fn set_visible(&self, visible: bool) {
        if self.wake.visible.swap(visible, Ordering::AcqRel) != visible {
            self.wake.dirty.store(true, Ordering::Release);
            if !visible {
                let _ = self.wake.commands.send(Command::Update);
            }
        }
    }

    fn create_surface(&self, size: [u32; 2]) -> Result<Surface, String> {
        let texture = self.state.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("media-video"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        // Initialize through wgpu first, so its lazy clear cannot overwrite the
        // frame after libmpv has filled the texture for the first time.
        let mut encoder = self
            .state
            .device
            .create_command_encoder(&Default::default());
        {
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
        self.state.queue.submit([encoder.finish()]);
        let raw = unsafe {
            let hal = texture.as_hal::<wgpu::hal::api::Gles>().unwrap();
            match hal.inner {
                wgpu::hal::gles::TextureInner::Texture { raw, .. } => raw,
                _ => return Err("Video target is not an OpenGL texture".into()),
            }
        };
        let framebuffer = with_gl(&self.state, |gl, _| unsafe {
            let framebuffer = gl.create_framebuffer()?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(raw),
                0,
            );
            let complete =
                gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE;
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            if !complete {
                gl.delete_framebuffer(framebuffer);
                return Err("Video framebuffer is incomplete".into());
            }
            Ok::<_, String>(framebuffer)
        })?;
        let id = self.state.renderer.write().register_native_texture(
            &self.state.device,
            &view,
            wgpu::FilterMode::Linear,
        );
        Ok(Surface {
            framebuffer,
            _texture: texture,
            id,
            size,
        })
    }

    fn free_surface(&self, surface: Surface) {
        self.state.renderer.write().free_texture(&surface.id);
        with_gl(&self.state, |gl, _| unsafe {
            gl.delete_framebuffer(surface.framebuffer)
        });
        // Drop the wgpu texture after releasing the GL context lock.
    }

    pub(super) fn frame(&mut self, size: [u32; 2]) -> Result<egui::TextureId, String> {
        let limit = self
            .state
            .device
            .limits()
            .max_texture_dimension_2d
            .min(8192);
        let size = size.map(|n| n.clamp(1, limit));
        let resized = self.surface.as_ref().is_none_or(|s| s.size != size);
        if resized {
            let surface = self.create_surface(size)?;
            if let Some(old) = self.surface.replace(surface) {
                self.free_surface(old);
            }
        }
        let was_visible = self.wake.visible.swap(true, Ordering::AcqRel);
        let force = resized || !was_visible;
        let surface = self.surface.as_ref().unwrap();
        if force || self.wake.dirty.load(Ordering::Acquire) {
            let (tx, rx) = mpsc::sync_channel(1);
            self.wake
                .commands
                .send(Command::Draw(surface.framebuffer, size, force, tx))
                .map_err(|e| e.to_string())?;
            rx.recv().map_err(|e| e.to_string())??;
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
            self.free_surface(surface);
        }
    }
}
