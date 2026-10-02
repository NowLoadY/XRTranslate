use super::{CaptureError, CapturedFrame, OverlayRegion, RgbImage};
use ashpd::desktop::{
    PersistMode,
    screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType},
};
use futures::StreamExt;
use pipewire::{self as pw, properties::properties, spa};
use std::{
    cell::{Cell, RefCell},
    future::Future,
    os::fd::OwnedFd,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use x11rb::rust_connection::RustConnection;

// A token is single-use and scoped to this application, never written to disk.
static RESTORE_TOKEN: Mutex<Option<String>> = Mutex::new(None);
const SAMPLE_INTERVAL: Duration = Duration::from_millis(200);

struct Frame {
    image: Arc<RgbImage>,
    logical_bounds: Option<OverlayRegion>,
}

type SharedFrame = Arc<Mutex<Result<Option<Frame>, String>>>;

pub struct Capture {
    latest: SharedFrame,
    stopped: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    connection: RustConnection,
    screen: usize,
}

impl Capture {
    pub fn new(cancelled: &AtomicBool) -> Result<Self, String> {
        let (connection, screen) = x11rb::connect(None).map_err(|e| e.to_string())?;
        let latest = Arc::new(Mutex::new(Ok(None)));
        let stopped = Arc::new(AtomicBool::new(false));
        let output = latest.clone();
        let stop = stopped.clone();
        let worker = std::thread::Builder::new()
            .name("screen-share".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run(output.clone(), &stop)
                }))
                .unwrap_or_else(|_| Err("Screen sharing stopped unexpectedly.".into()));
                if let Err(error) = result {
                    *output.lock().unwrap_or_else(|e| e.into_inner()) = Err(error);
                }
            })
            .map_err(|e| e.to_string())?;
        let capture = Self {
            latest,
            stopped,
            worker: Some(worker),
            connection,
            screen,
        };
        loop {
            if cancelled.load(Ordering::Acquire) {
                return Err("Screen capture stopped.".into());
            }
            let ready = match &*capture.latest.lock().unwrap_or_else(|e| e.into_inner()) {
                Ok(frame) => frame.is_some(),
                Err(error) => return Err(error.clone()),
            };
            if ready {
                return Ok(capture);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    pub fn region(
        &mut self,
        area: OverlayRegion,
        cancelled: &AtomicBool,
    ) -> Result<CapturedFrame, CaptureError> {
        if cancelled.load(Ordering::Acquire) {
            return Err(CaptureError::fatal("Screen capture stopped."));
        }
        let (image, logical_bounds) = {
            let latest = self.latest.lock().unwrap_or_else(|e| e.into_inner());
            let frame = latest
                .as_ref()
                .map_err(CaptureError::fatal)?
                .as_ref()
                .ok_or_else(|| CaptureError::retry("Waiting for screen sharing."))?;
            (frame.image.clone(), frame.logical_bounds)
        };
        let monitors =
            super::linux::monitors(&self.connection, self.screen).map_err(CaptureError::fatal)?;
        let bounds = monitor_bounds(logical_bounds, &monitors)?;
        CapturedFrame::cropped(&image, bounds, area)
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        // Capture is owned by the OCR worker, never the UI thread.
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// XWayland coordinates may be uniformly scaled relative to the compositor.
/// Accept only a unique match; guessing on an ambiguous layout captures the
/// wrong screen. Video pixels are then mapped relative to this monitor.
fn monitor_bounds(
    logical: Option<OverlayRegion>,
    monitors: &[OverlayRegion],
) -> Result<OverlayRegion, CaptureError> {
    let candidates: Vec<_> = monitors
        .iter()
        .copied()
        .filter(|monitor| {
            let Some(logical) = logical else {
                return monitors.len() == 1;
            };
            let sx = f64::from(monitor.width) / f64::from(logical.width);
            let sy = f64::from(monitor.height) / f64::from(logical.height);
            (sx - sy).abs() < 0.01
                && (f64::from(logical.x) * sx - f64::from(monitor.x)).abs() <= 1.0
                && (f64::from(logical.y) * sy - f64::from(monitor.y)).abs() <= 1.0
        })
        .collect();
    match candidates.as_slice() {
        [bounds] => Ok(*bounds),
        _ => Err(CaptureError::fatal(
            "Unable to map the shared screen to the OCR frame. Use matching display scales and select the screen containing the frame.",
        )),
    }
}

async fn cancellable<T>(
    work: impl Future<Output = Result<T, ashpd::Error>>,
    stopped: &AtomicBool,
) -> Result<T, String> {
    tokio::pin!(work);
    loop {
        if stopped.load(Ordering::Acquire) {
            return Err("Screen capture stopped.".into());
        }
        tokio::select! {
            result = &mut work => return result.map_err(|e| format!("Screen sharing could not start: {e}")),
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
    }
}

fn run(latest: SharedFrame, stopped: &AtomicBool) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let (portal, session, monitor, fd) = runtime.block_on(async {
        let portal = cancellable(Screencast::new(), stopped).await?;
        let session =
            Arc::new(cancellable(portal.create_session(Default::default()), stopped).await?);
        let token = RESTORE_TOKEN
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let setup = async {
            portal
                .select_sources(
                    &session,
                    SelectSourcesOptions::default()
                        .set_sources(Some(SourceType::Monitor.into()))
                        .set_cursor_mode(CursorMode::Hidden)
                        .set_multiple(false)
                        .set_persist_mode(PersistMode::Application)
                        .set_restore_token(token.as_deref()),
                )
                .await?
                .response()?;
            let selection = portal
                .start(&session, None, Default::default())
                .await?
                .response()?;
            *RESTORE_TOKEN.lock().unwrap_or_else(|e| e.into_inner()) =
                selection.restore_token().map(str::to_owned);
            let monitor = selection
                .streams()
                .first()
                .cloned()
                .ok_or(ashpd::Error::NoResponse)?;
            let fd = portal
                .open_pipe_wire_remote(&session, Default::default())
                .await?;
            Ok((monitor, fd))
        };
        match cancellable(setup, stopped).await {
            Ok((monitor, fd)) => Ok((portal, session, monitor, fd)),
            Err(error) => {
                // Closing the session also dismisses a pending source chooser.
                let _ = tokio::time::timeout(Duration::from_secs(1), session.close()).await;
                Err(error)
            }
        }
    })?;
    let closed_session = session.clone();
    let closed_output = latest.clone();
    let closed = runtime.spawn(async move {
        match closed_session.receive_closed().await {
            Ok(signal) => {
                futures::pin_mut!(signal);
                let _ = signal.next().await;
                *closed_output.lock().unwrap_or_else(|e| e.into_inner()) =
                    Err("Screen sharing ended. Switch back to OCR to reconnect.".into());
            }
            Err(error) => {
                *closed_output.lock().unwrap_or_else(|e| e.into_inner()) =
                    Err(format!("Screen sharing disconnected: {error}"));
            }
        }
    });
    let result = stream_frames(&latest, stopped, monitor, fd);
    closed.abort();
    runtime.block_on(async {
        let _ = tokio::time::timeout(Duration::from_secs(1), session.close()).await;
    });
    drop(portal);
    result
}

fn stream_frames(
    latest: &SharedFrame,
    stopped: &AtomicBool,
    monitor: ashpd::desktop::screencast::Stream,
    fd: OwnedFd,
) -> Result<(), String> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopBox::new(None).map_err(|e| e.to_string())?;
    let context =
        pw::context::ContextBox::new(mainloop.loop_(), None).map_err(|e| e.to_string())?;
    let core = context.connect_fd(fd, None).map_err(|e| e.to_string())?;
    let errors = latest.clone();
    let _core_listener = core
        .add_listener_local()
        .error(move |_, _, _, error| {
            *errors.lock().unwrap_or_else(|e| e.into_inner()) =
                Err(format!("Screen sharing disconnected: {error}"));
        })
        .register();
    let stream = pw::stream::StreamBox::new(&core, "XRTranslate screen text", properties! {
        *pw::keys::MEDIA_TYPE => "Video", *pw::keys::MEDIA_CATEGORY => "Capture", *pw::keys::MEDIA_ROLE => "Screen",
    }).map_err(|e| e.to_string())?;
    let logical_bounds =
        monitor
            .position()
            .zip(monitor.size())
            .and_then(|((x, y), (width, height))| {
                (width > 0 && height > 0).then_some(OverlayRegion {
                    x,
                    y,
                    width: width as u32,
                    height: height as u32,
                })
            });
    let format = Rc::new(RefCell::new(spa::param::video::VideoInfoRaw::default()));
    let parsed = format.clone();
    let format_errors = latest.clone();
    let pending = Rc::new(Cell::new(false));
    let ready = pending.clone();
    let errors = latest.clone();
    let _listener = stream
        .add_local_listener_with_user_data(())
        .state_changed(move |_, _, old, state| {
            let message = match state {
                pw::stream::StreamState::Error(error) => {
                    Some(format!("Screen sharing stopped: {error}"))
                }
                pw::stream::StreamState::Unconnected
                    if !matches!(old, pw::stream::StreamState::Unconnected) =>
                {
                    Some("Screen sharing ended.".into())
                }
                _ => None,
            };
            if let Some(message) = message {
                *errors.lock().unwrap_or_else(|e| e.into_inner()) = Err(message);
            }
        })
        .param_changed(move |_, _, id, param| {
            if id == spa::param::ParamType::Format.as_raw()
                && let Some(param) = param
            {
                if let Err(error) = parsed.borrow_mut().parse(param) {
                    *format_errors.lock().unwrap_or_else(|e| e.into_inner()) =
                        Err(format!("Unsupported shared screen format: {error}"));
                }
            }
        })
        .process(move |_, _| ready.set(true))
        .register()
        .map_err(|e| e.to_string())?;
    let format_pod = spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaType,
            Id,
            spa::param::format::MediaType::Video
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaSubtype,
            Id,
            spa::param::format::MediaSubtype::Raw
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            spa::param::video::VideoFormat::BGRx,
            spa::param::video::VideoFormat::BGRx,
            spa::param::video::VideoFormat::RGBx,
            spa::param::video::VideoFormat::BGRA,
            spa::param::video::VideoFormat::RGBA,
            spa::param::video::VideoFormat::RGB,
            spa::param::video::VideoFormat::BGR
        )
    );
    let bytes = spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(format_pod),
    )
    .map_err(|e| format!("Screen format: {e:?}"))?
    .0
    .into_inner();
    let mut params = [spa::pod::Pod::from_bytes(&bytes).ok_or("Invalid screen format")?];
    stream
        .connect(
            spa::utils::Direction::Input,
            Some(monitor.pipe_wire_node_id()),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(|e| e.to_string())?;
    let started = Instant::now();
    let mut sampled = started - SAMPLE_INTERVAL;
    let mut has_frame = false;
    while !stopped.load(Ordering::Acquire) {
        let status = mainloop
            .loop_()
            .iterate(pw::loop_::Timeout::Finite(Duration::from_millis(50)));
        if status < 0 {
            return Err("Screen sharing disconnected.".into());
        }
        if latest.lock().unwrap_or_else(|e| e.into_inner()).is_err() {
            break;
        }
        if !has_frame && started.elapsed() > Duration::from_secs(15) {
            return Err("The shared screen did not provide an image.".into());
        }
        if !pending.get() || sampled.elapsed() < SAMPLE_INTERVAL {
            continue;
        }
        pending.set(false);
        // Leave buffers queued between samples, then take the newest one. This
        // bounds copying to 5 Hz without losing the last frame of a static page.
        let mut newest = stream.dequeue_buffer();
        for _ in 0..16 {
            let Some(next) = stream.dequeue_buffer() else {
                break;
            };
            newest = Some(next);
        }
        let Some(mut buffer) = newest else {
            continue;
        };
        let Some(data) = buffer.datas_mut().first_mut() else {
            continue;
        };
        if let Some(image) = read_frame(data, &format.borrow())? {
            let mut output = latest.lock().unwrap_or_else(|e| e.into_inner());
            if output.is_ok() {
                *output = Ok(Some(Frame {
                    image: Arc::new(image),
                    logical_bounds,
                }));
            }
            has_frame = true;
            sampled = Instant::now();
        }
    }
    Ok(())
}

fn read_frame(
    data: &mut spa::buffer::Data,
    format: &spa::param::video::VideoInfoRaw,
) -> Result<Option<RgbImage>, String> {
    let width = format.size().width;
    let height = format.size().height;
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 67_108_864 {
        return Err("The shared screen image is too large or empty.".into());
    }
    let pixel_format = format.format();
    let (bytes, bgr) = match pixel_format {
        spa::param::video::VideoFormat::BGR => (3, true),
        spa::param::video::VideoFormat::RGB => (3, false),
        spa::param::video::VideoFormat::BGRx | spa::param::video::VideoFormat::BGRA => (4, true),
        spa::param::video::VideoFormat::RGBx | spa::param::video::VideoFormat::RGBA => (4, false),
        _ => return Err("Unsupported shared screen pixel format.".into()),
    };
    if data.as_raw().chunk.is_null() {
        return Err("Missing shared screen image metadata.".into());
    }
    let chunk = data.chunk();
    if chunk.flags().contains(spa::buffer::ChunkFlags::CORRUPTED) || chunk.size() == 0 {
        return Ok(None);
    }
    let stride = chunk.stride();
    let offset = chunk.offset() as usize;
    let size = chunk.size() as usize;
    if stride < width as i32 * bytes {
        return Err("Invalid shared screen row size.".into());
    }
    let needed = (height as usize - 1) * stride as usize + width as usize * bytes as usize;
    let Some(pixels) = data.data() else {
        return Err("The shared screen pixels are unavailable.".into());
    };
    if needed > size
        || offset
            .checked_add(needed)
            .is_none_or(|end| end > pixels.len())
    {
        return Err("Incomplete shared screen image.".into());
    }
    Ok(Some(RgbImage::from_fn(width, height, |x, y| {
        let index = offset + y as usize * stride as usize + x as usize * bytes as usize;
        if bgr {
            image::Rgb([pixels[index + 2], pixels[index + 1], pixels[index]])
        } else {
            image::Rgb([pixels[index], pixels[index + 1], pixels[index + 2]])
        }
    })))
}
