use super::MpvHandle;
use std::{
    path::Path,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};

/// Uses the installed playback engine's encoder and subtitle renderer.
pub(crate) fn encode_video(
    input: &Path,
    output: &Path,
    subtitles: &Path,
    style: &crate::plugins::media::subtitles::SubtitleStyle,
    progress: &AtomicU32,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    if !super::is_mpv_dll_available() {
        return Err("The video playback runtime is required for export".into());
    }
    let mpv = MpvHandle::create()?;
    let mut style = style.clone();
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
        ("sid", "auto"),
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
    // Pass a native array so commas and Unicode in paths stay literal.
    let subtitle_path = std::ffi::CString::new(subtitles.to_str().ok_or("Invalid subtitle path")?)
        .map_err(|e| e.to_string())?;
    let mut filename = libmpv_sys::mpv_node {
        u: libmpv_sys::mpv_node__bindgen_ty_1 {
            string: subtitle_path.as_ptr() as *mut _,
        },
        format: libmpv_sys::mpv_format_MPV_FORMAT_STRING,
    };
    let mut list = libmpv_sys::mpv_node_list {
        num: 1,
        values: &mut filename,
        keys: std::ptr::null_mut(),
    };
    let mut files = libmpv_sys::mpv_node {
        u: libmpv_sys::mpv_node__bindgen_ty_1 { list: &mut list },
        format: libmpv_sys::mpv_format_MPV_FORMAT_NODE_ARRAY,
    };
    let error = unsafe {
        libmpv_sys::mpv_set_option(
            mpv.handle,
            c"sub-files".as_ptr(),
            libmpv_sys::mpv_format_MPV_FORMAT_NODE,
            &mut files as *mut _ as *mut _,
        )
    };
    if error < 0 {
        return Err(format!("Cannot load export subtitles: {error}"));
    }
    mpv.initialize()?;
    mpv.command(&["loadfile", input.to_str().ok_or("Invalid source path")?])?;
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Video export cancelled".into());
        }
        let event = unsafe { &*libmpv_sys::mpv_wait_event(mpv.handle, 0.1) };
        if event.event_id == libmpv_sys::mpv_event_id_MPV_EVENT_END_FILE {
            let end = unsafe { &*(event.data as *const libmpv_sys::mpv_event_end_file) };
            if end.reason == libmpv_sys::mpv_end_file_reason_MPV_END_FILE_REASON_EOF as i32
                && end.error >= 0
            {
                break;
            }
            return Err(format!("Video export failed ({})", end.error));
        }
        if event.event_id == libmpv_sys::mpv_event_id_MPV_EVENT_SHUTDOWN {
            return Err("Video export stopped before completion".into());
        }
        if let (Some(position), Some(duration)) = (
            mpv.get_property_f64("time-pos"),
            mpv.get_property_f64("duration"),
        ) && duration > 0.0
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
        return Err("Video export cancelled".into());
    }
    if std::fs::metadata(output).map_err(|e| e.to_string())?.len() == 0 {
        return Err("Video export produced an empty file".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::media::subtitles::{SubtitleStyle, SubtitleTimeline};

    #[test]
    #[ignore = "requires the installed video playback runtime and encoders"]
    fn burned_bilingual_text_wraps_inside_the_video() {
        let timeline = SubtitleTimeline::parse(concat!(
            "1\n00:00:00,000 --> 00:00:01,000\n",
            "Boston Dynamics and Hyundai Motor Group are betting that Atlas is the strongest contender to win that race and usher in a new generation of machines designed to power human progress everywhere.\n",
            "波士顿动力和现代汽车集团都认为，Atlas是最有希望赢得这场竞赛的选手，它将引领新一代机器的出现，助力全球人类的进步。\n",
        )).unwrap();
        for (width, height, size) in [(1280, 720, 48.0), (640, 360, 48.0), (720, 1280, 24.0)] {
            let directory = tempfile::tempdir().unwrap();
            let subtitles = directory.path().join("subtitles.srt");
            let output = directory.path().join("subtitled.mp4");
            std::fs::write(&subtitles, timeline.export_playback_srt()).unwrap();
            encode_video(
                Path::new(&format!(
                    "av://lavfi:color=c=black:s={width}x{height}:r=25:d=1"
                )),
                &output,
                &subtitles,
                &SubtitleStyle {
                    size,
                    ..Default::default()
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
                reader.set_property_string(name, value).unwrap();
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
