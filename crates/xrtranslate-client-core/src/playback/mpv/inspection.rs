//! Bounded offline probes. Call from a worker, never the GUI thread.
use super::MpvHandle;
use std::{
    path::Path,
    time::{Duration, Instant},
};

#[derive(Debug, Clone)]
pub struct VideoProbe {
    pub duration_ms: i64,
    pub width: u32,
    pub height: u32,
}

/// Real, device-local detection. Empty output means no face was detected, not
/// proof that the entire image is safe for text. This does not estimate a body.
pub fn detect_frame_faces(path: &Path) -> Result<Vec<[f32; 4]>, String> {
    #[cfg(windows)]
    {
        use windows::{
            Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap},
            Media::FaceAnalysis::FaceDetector,
            Storage::Streams::DataWriter,
            Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
        };
        unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.map_err(|e| e.to_string())?;
        struct Apartment;
        impl Drop for Apartment {
            fn drop(&mut self) {
                unsafe { RoUninitialize() };
            }
        }
        let _apartment = Apartment;
        let image = image::open(path).map_err(|e| e.to_string())?.to_luma8();
        let result = (|| -> windows::core::Result<Vec<[f32; 4]>> {
            let bitmap = SoftwareBitmap::Create(
                BitmapPixelFormat::Gray8,
                image.width() as i32,
                image.height() as i32,
            )?;
            let writer = DataWriter::new()?;
            writer.WriteBytes(image.as_raw())?;
            bitmap.CopyFromBuffer(&writer.DetachBuffer()?)?;
            let detector = FaceDetector::CreateAsync()?.get()?;
            let faces = detector.DetectFacesAsync(&bitmap)?.get()?;
            let mut boxes = Vec::new();
            for index in 0..faces.Size()? {
                let rect = faces.GetAt(index)?.FaceBox()?;
                boxes.push([
                    rect.X as f32 / image.width() as f32,
                    rect.Y as f32 / image.height() as f32,
                    (rect.X + rect.Width) as f32 / image.width() as f32,
                    (rect.Y + rect.Height) as f32 / image.height() as f32,
                ]);
            }
            Ok(boxes)
        })();
        result.map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err("No face detector is configured for this platform".into())
    }
}

fn probe_handle() -> Result<MpvHandle, String> {
    if !super::is_mpv_dll_available() {
        return Err("Install the media playback runtime first".into());
    }
    let mpv = MpvHandle::create()?;
    for (key, value) in [
        ("config", "no"),
        ("load-scripts", "no"),
        ("terminal", "no"),
        ("audio", "no"),
        ("hwdec", "no"),
        ("idle", "yes"),
        ("keep-open", "no"),
    ] {
        mpv.set_option_string(key, value)?;
    }
    Ok(mpv)
}

pub fn inspect_media(source: &str) -> Result<VideoProbe, String> {
    let mpv = probe_handle()?;
    // A pure audio file must keep an audio stream selected for FILE_LOADED.
    // Null outputs allow both kinds of media to report metadata without playback.
    for (key, value) in [
        ("vo", "null"),
        ("audio", "auto"),
        ("ao", "null"),
        ("pause", "yes"),
    ] {
        mpv.set_option_string(key, value)?;
    }
    mpv.initialize()?;
    mpv.command(&["loadfile", source])?;
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(15) {
        let event = unsafe { &*libmpv_sys::mpv_wait_event(mpv.handle, 0.05) };
        if event.event_id == libmpv_sys::mpv_event_id_MPV_EVENT_FILE_LOADED {
            return Ok(VideoProbe {
                duration_ms: (mpv
                    .get_property_f64("duration")
                    .ok_or("Unknown media duration")?
                    * 1000.0)
                    .round() as i64,
                width: mpv.get_property_f64("video-params/w").unwrap_or(0.0) as u32,
                height: mpv.get_property_f64("video-params/h").unwrap_or(0.0) as u32,
            });
        }
        if event.event_id == libmpv_sys::mpv_event_id_MPV_EVENT_END_FILE {
            return Err("Media probe could not open the source".into());
        }
    }
    Err("Media probe timed out".into())
}

pub fn extract_video_frame(source: &str, source_ms: i64, output: &Path) -> Result<(), String> {
    extract_filtered_frame(source, source_ms, output, "scale=480:-2")
}

/// Save the displayed frame covering the playhead and return its presentation time.
/// An accurate forward seek can land one frame beyond a between-frame request.
pub fn capture_displayed_frame(source: &str, at_ms: i64, output: &Path) -> Result<i64, String> {
    let mpv = probe_handle()?;
    for (k, v) in [
        ("vo", "null"),
        ("pause", "yes"),
        ("keep-open", "yes"),
        ("sid", "no"),
        ("sub-auto", "no"),
        ("screenshot-format", "png"),
    ] {
        mpv.set_option_string(k, v)?;
    }
    mpv.set_option_string("start", &format!("{:.3}", at_ms as f64 / 1000.))?;
    mpv.initialize()?;
    let position = || {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let event = unsafe { &*libmpv_sys::mpv_wait_event(mpv.handle, 0.05) };
            if event.event_id == libmpv_sys::mpv_event_id_MPV_EVENT_PLAYBACK_RESTART {
                return mpv
                    .get_property_f64("time-pos")
                    .map(|s| (s * 1000.).round() as i64)
                    .ok_or("Unknown frame timestamp".to_string());
            }
            if event.event_id == libmpv_sys::mpv_event_id_MPV_EVENT_END_FILE {
                return Err("No frame at the requested time".into());
            }
        }
        Err("Timed out selecting the review frame".into())
    };
    mpv.command(&["loadfile", source])?;
    let mut actual = position()?;
    if actual > at_ms {
        mpv.command(&["frame-back-step"])?;
        actual = position()?;
    }
    mpv.command(&[
        "screenshot-to-file",
        output.to_str().ok_or("Invalid temporary path")?,
        "video",
    ])?;
    Ok(actual)
}

/// Decode one frame with a trusted renderer-provided filter chain.
pub fn extract_filtered_frame(
    source: &str,
    source_ms: i64,
    output: &Path,
    filter: &str,
) -> Result<(), String> {
    if source_ms < 0 {
        return Err("Frame time cannot be negative".into());
    }
    let directory = tempfile::tempdir().map_err(|e| e.to_string())?;
    let mpv = probe_handle()?;
    for (key, value) in [
        ("vo", "image"),
        ("vo-image-format", "png"),
        ("frames", "1"),
        ("vf", filter),
        ("sid", "no"),
        ("sub-auto", "no"),
        ("pause", "no"),
    ] {
        mpv.set_option_string(key, value)?;
    }
    mpv.set_option_string(
        "vo-image-outdir",
        directory.path().to_str().ok_or("Invalid probe directory")?,
    )?;
    mpv.set_option_string("start", &format!("{:.3}", source_ms as f64 / 1000.0))?;
    mpv.initialize()?;
    mpv.command(&["loadfile", source])?;
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(15) {
        let event = unsafe { &*libmpv_sys::mpv_wait_event(mpv.handle, 0.05) };
        if event.event_id == libmpv_sys::mpv_event_id_MPV_EVENT_END_FILE {
            let end = unsafe { &*(event.data as *const libmpv_sys::mpv_event_end_file) };
            if end.error < 0 {
                return Err(format!("Frame extraction failed: {}", end.error));
            }
            drop(mpv);
            let frame = std::fs::read_dir(directory.path())
                .map_err(|e| e.to_string())?
                .flatten()
                .map(|e| e.path())
                .find(|p| p.extension().is_some_and(|e| e == "png"))
                .ok_or("Decoder produced no frame")?;
            std::fs::copy(frame, output).map_err(|e| e.to_string())?;
            return Ok(());
        }
    }
    Err("Frame extraction timed out".into())
}
