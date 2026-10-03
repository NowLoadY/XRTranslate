//! Owns capture and recognition lifetimes; plugins receive only recognized text.
use crate::{ocr_runtime::OcrRuntime, overlay_ipc::OverlayRegion, screen_capture::ScreenCapture};
use crossbeam_channel::{Receiver, Sender, bounded};
use std::{
    hash::{Hash, Hasher},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
struct Request {
    revision: u64,
    region: Option<OverlayRegion>,
    excluded: [Option<OverlayRegion>; 2],
}

pub enum CaptureEvent {
    Busy(u64),
    Text(u64, String),
    Idle(u64),
    Failed {
        revision: u64,
        error: String,
        fatal: bool,
    },
}

impl CaptureEvent {
    fn revision(&self) -> u64 {
        match self {
            Self::Busy(revision)
            | Self::Text(revision, _)
            | Self::Idle(revision)
            | Self::Failed { revision, .. } => *revision,
        }
    }
}

/// Preserve the newest text while subsequent recognition is busy. Both slots
/// are bounded and publication never waits for the UI to consume an event.
#[derive(Default)]
struct Mailbox {
    text: Option<CaptureEvent>,
    status: Option<CaptureEvent>,
}

impl Mailbox {
    fn publish(&mut self, event: CaptureEvent) {
        if matches!(event, CaptureEvent::Text(..)) {
            self.status = Some(CaptureEvent::Idle(event.revision()));
            self.text = Some(event);
        } else {
            self.status = Some(event);
        }
    }
}

pub struct OcrCapture {
    request: Arc<Mutex<Request>>,
    cancelled: Arc<AtomicBool>,
    events: Arc<Mutex<Mailbox>>,
    wake: Sender<()>,
    worker: std::thread::JoinHandle<()>,
}

impl OcrCapture {
    pub fn start(
        root: PathBuf,
        region: OverlayRegion,
        revision: u64,
        excluded: [Option<OverlayRegion>; 2],
    ) -> Result<Self, String> {
        let request = Arc::new(Mutex::new(Request {
            revision,
            region: Some(region),
            excluded,
        }));
        let cancelled = Arc::new(AtomicBool::new(false));
        let events = Arc::new(Mutex::new(Mailbox::default()));
        let (wake, changes) = bounded(1);
        let pending = request.clone();
        let stop = cancelled.clone();
        let output = events.clone();
        let worker = std::thread::Builder::new()
            .name("screen-text".into())
            .spawn(move || {
                publish(&output, CaptureEvent::Busy(revision));
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run(root, &pending, &stop, &output, &changes)
                }))
                .unwrap_or_else(|_| {
                    Err("Screen recognition stopped unexpectedly. Reopen OCR to try again.".into())
                });
                if let Err(error) = result
                    && !stop.load(Ordering::Acquire)
                {
                    let revision = pending.lock().unwrap_or_else(|e| e.into_inner()).revision;
                    publish(
                        &output,
                        CaptureEvent::Failed {
                            revision,
                            error,
                            fatal: true,
                        },
                    );
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            request,
            cancelled,
            events,
            wake,
            worker,
        })
    }

    pub fn update(
        &self,
        region: Option<OverlayRegion>,
        revision: u64,
        excluded: [Option<OverlayRegion>; 2],
    ) {
        *self.request.lock().unwrap_or_else(|e| e.into_inner()) = Request {
            revision,
            region,
            excluded,
        };
        let _ = self.wake.try_send(());
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        let _ = self.wake.try_send(());
    }

    pub fn is_finished(&self) -> bool {
        self.worker.is_finished()
    }

    pub fn poll(&self) -> impl Iterator<Item = CaptureEvent> {
        let revision = self
            .request
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .revision;
        let mut events = self.events.lock().unwrap_or_else(|e| e.into_inner());
        [events.text.take(), events.status.take()]
            .into_iter()
            .flatten()
            .filter(move |event| event.revision() == revision)
    }
}

impl Drop for OcrCapture {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn publish(events: &Mutex<Mailbox>, event: CaptureEvent) {
    events
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .publish(event);
}

fn run(
    root: PathBuf,
    request: &Mutex<Request>,
    cancelled: &AtomicBool,
    events: &Mutex<Mailbox>,
    changes: &Receiver<()>,
) -> Result<(), String> {
    let config =
        xrtranslate_config::AppConfig::from_path_with_user_config(&root.join("config.json"), &root)
            .map_err(|e| e.to_string())?;
    let mut capture = ScreenCapture::new(cancelled)?;
    let mut runtime = OcrRuntime::load(&root, &config, cancelled)?;
    let mut previous_image: Option<(((u32, u32), u64), String)> = None;
    let mut previous_text: Option<(u64, String)> = None;
    let mut empty_since: Option<(u64, Instant)> = None;
    let mut recognition_failures = 0;
    while !cancelled.load(Ordering::Acquire) {
        let current = *request.lock().unwrap_or_else(|e| e.into_inner());
        if empty_since.is_some_and(|(revision, _)| revision != current.revision) {
            empty_since = None;
        }
        let Some(region) = current.region else {
            // Pauses retain model/image results, but cannot confirm an empty
            // observation across a period when no pixels were sampled.
            empty_since = None;
            let _ = changes.recv();
            continue;
        };
        let started = Instant::now();
        let mut retry = false;
        match capture.region(region, cancelled) {
            Err(error) => {
                empty_since = None;
                if error.fatal {
                    return Err(error.message);
                }
                retry = true;
                publish(
                    events,
                    CaptureEvent::Failed {
                        revision: current.revision,
                        error: error.message,
                        fatal: false,
                    },
                );
            }
            Ok(mut frame) => {
                if request.lock().unwrap_or_else(|e| e.into_inner()).revision != current.revision {
                    continue;
                }
                let mut occluded = false;
                for area in current.excluded.into_iter().flatten() {
                    occluded |= frame.mask(area);
                }
                let image = frame.image;
                let mut hash = std::collections::hash_map::DefaultHasher::new();
                image.as_raw().hash(&mut hash);
                let fingerprint = (image.dimensions(), hash.finish());
                let result = match &previous_image {
                    Some((previous, text)) if *previous == fingerprint => Ok(text.clone()),
                    _ if image
                        .pixels()
                        .next()
                        .is_none_or(|first| image.pixels().all(|pixel| pixel == first)) =>
                    {
                        Ok(String::new())
                    }
                    _ => {
                        publish(events, CaptureEvent::Busy(current.revision));
                        runtime.recognize(image, cancelled)
                    }
                };
                if cancelled.load(Ordering::Acquire) {
                    break;
                }
                // Cache successful pixels independently of UI revisions. A
                // pause/resume or moved window must not rerun identical input.
                if let Ok(text) = &result {
                    previous_image = Some((fingerprint, text.clone()));
                }
                if request.lock().unwrap_or_else(|e| e.into_inner()).revision != current.revision {
                    continue;
                }
                match result {
                    Ok(text) => {
                        recognition_failures = 0;
                        let text = text.trim();
                        let ready = if text.is_empty() {
                            if occluded {
                                empty_since = None;
                                false
                            } else {
                                let (_, since) = empty_since
                                    .get_or_insert_with(|| (current.revision, Instant::now()));
                                since.elapsed() >= Duration::from_millis(500)
                            }
                        } else {
                            empty_since = None;
                            true
                        };
                        let unchanged =
                            previous_text.as_ref().is_some_and(|(revision, previous)| {
                                *revision == current.revision
                                    && previous.split_whitespace().eq(text.split_whitespace())
                            });
                        if ready && !unchanged {
                            previous_text = Some((current.revision, text.to_owned()));
                            publish(
                                events,
                                CaptureEvent::Text(current.revision, text.to_owned()),
                            );
                        } else {
                            publish(events, CaptureEvent::Idle(current.revision));
                        }
                    }
                    Err(error) => {
                        empty_since = None;
                        recognition_failures += 1;
                        if recognition_failures >= 3 {
                            return Err(error);
                        }
                        retry = true;
                        publish(
                            events,
                            CaptureEvent::Failed {
                                revision: current.revision,
                                error,
                                fatal: false,
                            },
                        );
                    }
                }
            }
        }
        let delay = if retry {
            Duration::from_secs(2)
        } else {
            Duration::from_millis(700)
                .saturating_sub(started.elapsed())
                .max(Duration::from_millis(200))
        };
        let _ = changes.recv_timeout(delay);
    }
    Ok(())
}
