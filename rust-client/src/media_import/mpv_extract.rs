use std::{
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

#[cfg(feature = "mpv")]
use crossbeam_channel::Sender;
#[cfg(feature = "mpv")]
use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

use super::types::{AudioImportError, AudioImportEvent};
#[cfg(feature = "mpv")]
use super::types::{AudioImportProgress, AudioImportStage, IMPORT_SAMPLE_RATE};

pub(super) struct TempFileGuard(pub(super) PathBuf);

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(feature = "mpv")]
fn build_recognition_pan_filter(recognition_channels: &[usize]) -> Option<String> {
    if recognition_channels.is_empty() {
        return None;
    }
    if recognition_channels == [0, 1] {
        return Some("lavfi=[pan=stereo|c0=1.0*c0|c1=1.0*c1]".to_string());
    }
    if recognition_channels.len() == 1 {
        let idx = recognition_channels[0];
        return Some(format!("lavfi=[pan=stereo|c0=1.0*c{idx}|c1=1.0*c{idx}]"));
    }
    let scale = 1.0 / recognition_channels.len() as f32;
    let terms: Vec<String> = recognition_channels
        .iter()
        .map(|&idx| format!("{scale:.4}*c{idx}"))
        .collect();
    let sum_expr = terms.join("+");
    Some(format!("lavfi=[pan=stereo|c0={sum_expr}|c1={sum_expr}]"))
}

#[cfg(feature = "mpv")]
pub(super) fn run_mpv_extract(
    source_path: &Path,
    target_wav: &Path,
    recognition_channels: &[usize],
    stop_requested: &AtomicBool,
    event_tx: &Sender<AudioImportEvent>,
) -> Result<(), AudioImportError> {
    if let Some(parent) = target_wav.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mpv = unsafe { libmpv_sys::mpv_create() };
    if mpv.is_null() {
        return Err(AudioImportError::Unsupported(
            "Failed to create libmpv handle for audio decoding".into(),
        ));
    }
    unsafe {
        use std::ffi::CString;
        let set = |k: &str, v: &str| -> Result<(), AudioImportError> {
            let ck =
                CString::new(k).map_err(|e| AudioImportError::InvalidOptions(e.to_string()))?;
            let cv =
                CString::new(v).map_err(|e| AudioImportError::InvalidOptions(e.to_string()))?;
            let res = libmpv_sys::mpv_set_property_string(mpv, ck.as_ptr(), cv.as_ptr());
            if res < 0 {
                return Err(AudioImportError::Decode(format!(
                    "failed to set mpv property {k}={v}: {res}"
                )));
            }
            Ok(())
        };
        let wav_path_str = target_wav.to_string_lossy().replace('\\', "/");
        set("o", &wav_path_str)?;
        set("of", "wav")?;
        set("ovc", "null")?;
        set("oac", "pcm_s16le")?;
        set("audio-samplerate", "16000")?;
        if let Some(filter) = build_recognition_pan_filter(recognition_channels) {
            set("af", &filter)?;
            set("audio-channels", "stereo")?;
        } else {
            set("audio-channels", "auto-safe")?;
        }
        set("video", "no")?;
        set("vid", "no")?;
        set("vo", "null")?;
        set("audio-pitch-correction", "no")?;
        set("untimed", "yes")?;
        set("demuxer-readahead-secs", "20")?;
        set("msg-level", "all=error")?;
        let init_res = libmpv_sys::mpv_initialize(mpv);
        if init_res < 0 {
            libmpv_sys::mpv_terminate_destroy(mpv);
            return Err(AudioImportError::Unsupported(format!(
                "mpv_initialize failed with error code {init_res}"
            )));
        }

        let path_str = source_path
            .to_str()
            .ok_or_else(|| AudioImportError::InvalidOptions("invalid source media path".into()))?;
        let cmd = [
            CString::new("loadfile").unwrap(),
            CString::new(path_str).unwrap(),
        ];
        let mut ptrs: Vec<*const std::os::raw::c_char> = cmd.iter().map(|s| s.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        let cmd_res = libmpv_sys::mpv_command(mpv, ptrs.as_mut_ptr());
        if cmd_res < 0 {
            libmpv_sys::mpv_terminate_destroy(mpv);
            return Err(AudioImportError::Decode(format!(
                "mpv loadfile failed: {cmd_res}"
            )));
        }

        let mut next_poll = Instant::now();
        loop {
            if stop_requested.load(Ordering::Acquire) {
                let _ = libmpv_sys::mpv_command_string(mpv, CString::new("stop").unwrap().as_ptr());
                libmpv_sys::mpv_terminate_destroy(mpv);
                return Err(AudioImportError::Cancelled);
            }

            let event = libmpv_sys::mpv_wait_event(mpv, 0.05);
            if !event.is_null() {
                let event_id = (*event).event_id;
                if event_id == libmpv_sys::mpv_event_id_MPV_EVENT_END_FILE {
                    break;
                }
                if event_id == libmpv_sys::mpv_event_id_MPV_EVENT_SHUTDOWN {
                    break;
                }
            }

            if next_poll.elapsed() >= Duration::from_millis(250) {
                next_poll = Instant::now();
                let mut pos_sec: f64 = 0.0;
                let mut dur_sec: f64 = 0.0;
                let c_time_pos = CString::new("time-pos").unwrap();
                let c_duration = CString::new("duration").unwrap();
                let has_pos = libmpv_sys::mpv_get_property(
                    mpv,
                    c_time_pos.as_ptr(),
                    libmpv_sys::mpv_format_MPV_FORMAT_DOUBLE,
                    &mut pos_sec as *mut _ as *mut std::ffi::c_void,
                ) >= 0;
                let has_dur = libmpv_sys::mpv_get_property(
                    mpv,
                    c_duration.as_ptr(),
                    libmpv_sys::mpv_format_MPV_FORMAT_DOUBLE,
                    &mut dur_sec as *mut _ as *mut std::ffi::c_void,
                ) >= 0;

                if has_pos {
                    let position = Duration::from_secs_f64(pos_sec.max(0.0));
                    let duration = if has_dur && dur_sec > 0.0 {
                        Some(Duration::from_secs_f64(dur_sec))
                    } else {
                        None
                    };
                    let fraction = duration.map(|d| (pos_sec / d.as_secs_f64()) as f32);
                    let _ = event_tx.send(AudioImportEvent::Progress(AudioImportProgress {
                        stage: AudioImportStage::Extracting,
                        decoded_source_frames: (pos_sec * IMPORT_SAMPLE_RATE as f64) as u64,
                        total_source_frames: duration
                            .map(|d| (d.as_secs_f64() * IMPORT_SAMPLE_RATE as f64) as u64),
                        position,
                        duration,
                        fraction,
                    }));
                }
            }
        }
        libmpv_sys::mpv_terminate_destroy(mpv);
    }

    if !target_wav.exists() {
        return Err(AudioImportError::Decode(
            "MPV did not produce an extracted audio file".into(),
        ));
    }
    Ok(())
}

#[cfg(not(feature = "mpv"))]
pub(super) fn run_mpv_extract(
    _source_path: &Path,
    _target_wav: &Path,
    _recognition_channels: &[usize],
    _stop_requested: &AtomicBool,
    _event_tx: &crossbeam_channel::Sender<AudioImportEvent>,
) -> Result<(), AudioImportError> {
    Err(AudioImportError::Unsupported(
        "MPV media extraction is unavailable; rebuild with the `mpv` feature".into(),
    ))
}
