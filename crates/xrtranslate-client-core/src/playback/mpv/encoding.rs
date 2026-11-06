use super::MpvHandle;
use std::{
    path::Path,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};

/// Fit speech with the installed audio filter, preserving voice pitch.
pub fn encode_audio(
    input: &Path,
    output: &Path,
    mut tempo: f64,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    if !super::is_mpv_dll_available() {
        return Err("The media playback runtime is required to fit speech timing".into());
    }
    let mpv = MpvHandle::create()?;
    let mut filters = Vec::new();
    while tempo > 2.0 {
        filters.push("atempo=2".to_owned());
        tempo /= 2.0;
    }
    filters.push(format!("atempo={tempo:.8}"));
    for (name, value) in [
        ("config", "no"),
        ("load-scripts", "no"),
        ("terminal", "no"),
        ("o", output.to_str().ok_or("Invalid audio output path")?),
        ("of", "wav"),
        ("oac", "pcm_s16le"),
        ("ao", "lavc"),
        ("vid", "no"),
        ("vo", "null"),
        ("keep-open", "no"),
        ("idle", "yes"),
    ] {
        mpv.set_option_string(name, value)?;
    }
    mpv.set_option_string("af", &format!("lavfi=[{}]", filters.join(",")))?;
    mpv.initialize()?;
    mpv.command(&[
        "loadfile",
        input.to_str().ok_or("Invalid speech input path")?,
    ])?;
    wait_for_encoding(mpv, output, None, cancelled, None)
}

pub enum VideoAudio<'a> {
    Original(Option<i64>),
    Speech {
        path: &'a Path,
        background: Option<&'a Path>,
    },
}

pub struct VideoEncoding<'a> {
    pub subtitles: Option<&'a Path>,
    pub style: &'a crate::playback::SubtitleStyle,
    pub audio: VideoAudio<'a>,
}

/// Uses the installed playback engine's encoder and subtitle renderer.
pub fn encode_video(
    input: &Path,
    output: &Path,
    options: VideoEncoding<'_>,
    progress: &AtomicU32,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    if !super::is_mpv_dll_available() {
        return Err("The video playback runtime is required for export".into());
    }
    let mpv = MpvHandle::create()?;
    let mut style = options.style.clone();
    style.normalize();
    let font_size = style.size.to_string();
    let color = style.rgb();
    for (name, value) in [
        ("config", "no"),
        ("load-scripts", "no"),
        ("terminal", "no"),
        ("o", output.to_str().ok_or("Invalid output path")?),
        ("of", "mp4"),
        ("ofopts", "movflags=+faststart"),
        ("ovc", "libx264"),
        ("ovcopts", "preset=medium,crf=20"),
        ("oac", "aac"),
        ("oacopts", "b=192000"),
        ("vo", "lavc"),
        ("ao", "lavc"),
        ("hwdec", "no"),
        ("sub-auto", "no"),
        (
            "sid",
            if options.subtitles.is_some() {
                "auto"
            } else {
                "no"
            },
        ),
        ("audio-file-auto", "no"),
        // Plain text uses the same Unicode wrapping as the playback overlay.
        ("sub-ass-override", "strip"),
        ("sub-vsfilter-bidi-compat", "no"),
        ("sub-font", style.font.as_str()),
        ("sub-font-size", font_size.as_str()),
        ("sub-color", color.as_str()),
        ("sub-border-size", "1.5"),
        ("sub-shadow-offset", "1"),
        ("sub-margin-x", "24"),
        ("sub-margin-y", "24"),
        ("sub-scale-with-window", "no"),
        ("sub-use-margins", "no"),
        ("osd-level", "0"),
        ("keep-open", "no"),
        ("idle", "yes"),
    ] {
        let name_c = std::ffi::CString::new(name).map_err(|e| e.to_string())?;
        let value_c = std::ffi::CString::new(value).map_err(|e| e.to_string())?;
        let result = unsafe {
            libmpv_sys::mpv_set_option_string(mpv.handle, name_c.as_ptr(), value_c.as_ptr())
        };
        if result < 0 {
            return Err(format!("Video export option {name}: {result}"));
        }
    }
    if let Some(subtitles) = options.subtitles {
        set_files(&mpv, c"sub-files", &[subtitles])?;
    }
    match &options.audio {
        VideoAudio::Original(track) => {
            if let Some(track) = track {
                mpv.set_option_string("aid", &track.to_string())?;
            }
        }
        VideoAudio::Speech { path, background } => {
            let mut files = vec![*path];
            files.extend(*background);
            for file in &files {
                if !file.is_file() {
                    return Err(format!("Audio file is missing: {}", file.display()));
                }
            }
            set_files(&mpv, c"audio-files", &files)?;
            // Never fall back to the source voices if an external track fails.
            mpv.set_option_string("aid", "no")?;
        }
    }
    mpv.initialize()?;
    if matches!(options.audio, VideoAudio::Speech { .. }) {
        let error = unsafe { libmpv_sys::mpv_hook_add(mpv.handle, 0, c"on_preloaded".as_ptr(), 0) };
        if error < 0 {
            return Err(format!("Cannot prepare export audio: {error}"));
        }
    }
    mpv.command(&["loadfile", input.to_str().ok_or("Invalid source path")?])?;
    wait_for_encoding(mpv, output, Some(progress), cancelled, Some(&options.audio))
}

// Native arrays preserve commas, Unicode and other punctuation in file paths.
fn set_files(mpv: &MpvHandle, name: &std::ffi::CStr, paths: &[&Path]) -> Result<(), String> {
    let strings = paths
        .iter()
        .map(|path| {
            std::ffi::CString::new(path.to_str().ok_or("Invalid media path")?)
                .map_err(|e| e.to_string())
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut filenames: Vec<_> = strings
        .iter()
        .map(|path| libmpv_sys::mpv_node {
            u: libmpv_sys::mpv_node__bindgen_ty_1 {
                string: path.as_ptr() as *mut _,
            },
            format: libmpv_sys::mpv_format_MPV_FORMAT_STRING,
        })
        .collect();
    let mut list = libmpv_sys::mpv_node_list {
        num: filenames.len() as i32,
        values: filenames.as_mut_ptr(),
        keys: std::ptr::null_mut(),
    };
    let mut files = libmpv_sys::mpv_node {
        u: libmpv_sys::mpv_node__bindgen_ty_1 { list: &mut list },
        format: libmpv_sys::mpv_format_MPV_FORMAT_NODE_ARRAY,
    };
    let error = unsafe {
        libmpv_sys::mpv_set_option(
            mpv.handle,
            name.as_ptr(),
            libmpv_sys::mpv_format_MPV_FORMAT_NODE,
            &mut files as *mut _ as *mut _,
        )
    };
    if error < 0 {
        return Err(format!("Cannot load export files: {error}"));
    }
    Ok(())
}

fn select_export_audio(mpv: &MpvHandle, audio: &VideoAudio<'_>) -> Result<(), String> {
    let VideoAudio::Speech { path, background } = audio else {
        return Ok(());
    };
    let mut node: libmpv_sys::mpv_node = unsafe { std::mem::zeroed() };
    let error = unsafe {
        libmpv_sys::mpv_get_property(
            mpv.handle,
            c"track-list".as_ptr(),
            libmpv_sys::mpv_format_MPV_FORMAT_NODE,
            &mut node as *mut _ as *mut _,
        )
    };
    if error < 0 {
        return Err("Cannot read export audio tracks".into());
    }
    let tracks = unsafe { super::tracks::read(&node) };
    unsafe { libmpv_sys::mpv_free_node_contents(&mut node) };
    let track_id = |path: &Path| -> Result<i64, String> {
        let path = path.canonicalize().map_err(|e| e.to_string())?;
        tracks
            .iter()
            .find(|track| {
                track
                    .external
                    .as_ref()
                    .and_then(|file| file.canonicalize().ok())
                    .as_ref()
                    == Some(&path)
            })
            .map(|track| track.id)
            .ok_or_else(|| "Could not load the selected export audio".into())
    };
    let speech = track_id(path)?;
    let graph = if let Some(background) = background {
        let background = track_id(background)?;
        format!(
            "[aid{speech}]aformat=channel_layouts=stereo[voice];[aid{background}]aformat=channel_layouts=stereo[bg];[voice][bg]amix=inputs=2:duration=longest:normalize=0,alimiter=limit=0.95:level=0:latency=1[ao]"
        )
    } else {
        format!("[aid{speech}]anull[ao]")
    };
    mpv.command(&["set", "lavfi-complex", &graph])?;
    // A moved speech clip must not extend the original video's duration.
    if let Some(duration) = mpv.get_property_f64("duration").filter(|d| *d > 0.0) {
        mpv.command(&["set", "end", &duration.to_string()])?;
    }
    Ok(())
}

fn wait_for_encoding(
    mpv: MpvHandle,
    output: &Path,
    progress: Option<&AtomicU32>,
    cancelled: &AtomicBool,
    audio: Option<&VideoAudio<'_>>,
) -> Result<(), String> {
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Media encoding cancelled".into());
        }
        let event = unsafe { &*libmpv_sys::mpv_wait_event(mpv.handle, 0.1) };
        if event.event_id == libmpv_sys::mpv_event_id_MPV_EVENT_HOOK {
            let hook = unsafe { &*(event.data as *const libmpv_sys::mpv_event_hook) };
            let result = audio.map_or(Ok(()), |audio| select_export_audio(&mpv, audio));
            let error = unsafe { libmpv_sys::mpv_hook_continue(mpv.handle, hook.id) };
            result?;
            if error < 0 {
                return Err("Could not start video export".into());
            }
        }
        if event.event_id == libmpv_sys::mpv_event_id_MPV_EVENT_END_FILE {
            let end = unsafe { &*(event.data as *const libmpv_sys::mpv_event_end_file) };
            if end.reason == libmpv_sys::mpv_end_file_reason_MPV_END_FILE_REASON_EOF as i32
                && end.error >= 0
            {
                break;
            }
            return Err(format!("Media encoding failed ({})", end.error));
        }
        if event.event_id == libmpv_sys::mpv_event_id_MPV_EVENT_SHUTDOWN {
            return Err("Media encoding stopped before completion".into());
        }
        if let (Some(position), Some(duration)) = (
            mpv.get_property_f64("time-pos"),
            mpv.get_property_f64("duration"),
        ) && duration > 0.0
            && let Some(progress) = progress
        {
            progress.store(
                (position / duration * 1000.0).clamp(0.0, 999.0) as u32,
                Ordering::Relaxed,
            );
        }
    }
    // Destruction finalizes the container before the caller publishes the file.
    drop(mpv);
    if cancelled.load(Ordering::Relaxed) {
        return Err("Media encoding cancelled".into());
    }
    if std::fs::metadata(output).map_err(|e| e.to_string())?.len() == 0 {
        return Err("Media encoding produced an empty file".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playback::SubtitleStyle;

    #[test]
    #[ignore = "requires the installed video playback runtime and encoders"]
    fn burned_bilingual_text_wraps_inside_the_video() {
        let subtitles_content = concat!(
            "1\n00:00:00,000 --> 00:00:01,000\n",
            "Boston Dynamics and Hyundai Motor Group are betting that Atlas is the strongest contender to win that race and usher in a new generation of machines designed to power human progress everywhere.\n",
            "波士顿动力和现代汽车集团都认为，Atlas是最有希望赢得这场竞赛的选手，它将引领新一代机器的出现，助力全球人类的进步。\n",
        );
        for (width, height, size) in [(1280, 720, 48.0), (640, 360, 48.0), (720, 1280, 24.0)] {
            let directory = tempfile::tempdir().unwrap();
            let subtitles = directory.path().join("subtitles.srt");
            let output = directory.path().join("subtitled.mp4");
            std::fs::write(&subtitles, subtitles_content).unwrap();
            encode_video(
                Path::new(&format!(
                    "av://lavfi:color=c=black:s={width}x{height}:r=25:d=1"
                )),
                &output,
                VideoEncoding {
                    subtitles: Some(&subtitles),
                    style: &SubtitleStyle {
                        size,
                        ..Default::default()
                    },
                    audio: VideoAudio::Original(None),
                },
                &AtomicU32::new(0),
                &AtomicBool::new(false),
            )
            .unwrap();

            // Inspect the encoded frame, so this catches renderer wrapping failures.
            let reader = MpvHandle::create().unwrap();
            for (name, value) in [
                ("config", "no"),
                ("load-scripts", "no"),
                ("terminal", "no"),
                ("vo", "image"),
                ("vo-image-format", "png"),
                ("vo-image-outdir", directory.path().to_str().unwrap()),
                ("audio", "no"),
                ("frames", "1"),
                ("start", "0.5"),
                ("idle", "yes"),
            ] {
                reader.set_option_string(name, value).unwrap();
            }
            reader.initialize().unwrap();
            reader
                .command(&["loadfile", output.to_str().unwrap()])
                .unwrap();
            for _ in 0..100 {
                if unsafe { (*libmpv_sys::mpv_wait_event(reader.handle, 0.1)).event_id }
                    == libmpv_sys::mpv_event_id_MPV_EVENT_END_FILE
                {
                    break;
                }
            }
            drop(reader);
            let frame = image::open(directory.path().join("00000001.png"))
                .unwrap()
                .to_rgb8();
            let mut visible = 0;
            for (x, y, pixel) in frame.enumerate_pixels() {
                if pixel.0.iter().any(|&channel| channel > 80) {
                    visible += 1;
                    assert!(
                        x > 2 && x < width - 3 && y > 2 && y < height - 3,
                        "Subtitle touches the video edge at {x},{y} in {width}x{height}"
                    );
                }
            }
            assert!(
                visible > 200,
                "Subtitles must actually be burned into the frame"
            );
        }
    }
}
