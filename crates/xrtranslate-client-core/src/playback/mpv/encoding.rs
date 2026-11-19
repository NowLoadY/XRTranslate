use super::MpvHandle;
use std::{
    path::Path,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};

/// Fit speech with the installed audio filter, preserving voice pitch.
pub fn encode_audio(
    input: &Path,
    output: &Path,
    tempo: f64,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    encode_audio_filtered(input, output, tempo, None, cancelled)
}

pub fn encode_audio_volume(
    input: &Path,
    output: &Path,
    volume: u16,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    encode_audio_filtered(input, output, 1.0, Some(volume), cancelled)
}

fn encode_audio_filtered(
    input: &Path,
    output: &Path,
    mut tempo: f64,
    volume: Option<u16>,
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
    if let Some(volume) = volume.filter(|volume| *volume != 100) {
        filters.push(format!("volume={:.4}", volume.min(200) as f64 / 100.0));
        filters.push(super::tracks::LIMITER.to_owned());
    }
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

/// Decode a bounded interval to RGB for native vision workers. The EDL rebases
/// timestamps to zero before the sampling filter, including after a scene cut.
pub fn decode_rgb_samples(
    source: &str,
    start_ms: i64,
    duration_ms: i64,
    size: [u32; 2],
    fps: u32,
    output: &Path,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    // A reel can end between two frame timestamps. Such a tail still displays a
    // picture, while a new fps-filtered interval may have no frame to emit.
    if fps > 0 && duration_ms > 0 && duration_ms.saturating_mul(i64::from(fps)) < 1000 {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Media encoding cancelled".into());
        }
        let file = tempfile::Builder::new()
            .suffix(".png")
            .tempfile()
            .map_err(|e| e.to_string())?
            .into_temp_path();
        super::capture_displayed_frame(source, start_ms, &file)?;
        let sample = image::open(&file).map_err(|e| e.to_string())?.into_rgb8();
        let fitted = image::imageops::resize(
            &sample,
            size[0],
            size[1],
            image::imageops::FilterType::Triangle,
        );
        return std::fs::write(output, fitted.as_raw()).map_err(|e| e.to_string());
    }
    decode_video_samples(
        source,
        start_ms,
        duration_ms,
        size,
        fps,
        false,
        output,
        cancelled,
    )
}

pub fn decode_video_samples(
    source: &str,
    start_ms: i64,
    duration_ms: i64,
    size: [u32; 2],
    fps: u32,
    alpha: bool,
    output: &Path,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    if !super::is_mpv_dll_available() {
        return Err("Install the playback runtime".into());
    }
    let mpv = MpvHandle::create()?;
    for (k, v) in [
        ("config", "no"),
        ("load-scripts", "no"),
        ("terminal", "no"),
        ("audio", "no"),
        ("sid", "no"),
        ("vo", "lavc"),
        ("ovc", "rawvideo"),
        ("of", "rawvideo"),
        ("hwdec", "no"),
        ("idle", "yes"),
        ("keep-open", "no"),
    ] {
        mpv.set_option_string(k, v)?;
    }
    mpv.set_option_string("o", output.to_str().ok_or("Invalid output path")?)?;
    mpv.set_option_string(
        "vf",
        &format!(
            "lavfi=[fps={fps},scale={}:{},format={}]",
            size[0],
            size[1],
            if alpha { "rgba" } else { "rgb24" }
        ),
    )?;
    mpv.initialize()?;
    let edl = format!(
        "edl://%{}%{source},{:.3},{:.3}",
        source.len(),
        start_ms as f64 / 1000.0,
        duration_ms as f64 / 1000.0
    );
    mpv.command(&["loadfile", &edl])?;
    wait_for_encoding(mpv, output, None, cancelled, None).map_err(|e| {
        format!(
            "Cannot decode video samples at {start_ms}–{} ms ({fps} fps): {e}",
            start_ms.saturating_add(duration_ms)
        )
    })
}

pub type VideoAudio = super::super::PlaybackAudio;

pub struct VideoEncoding<'a> {
    pub subtitles: Option<&'a Path>,
    pub style: &'a crate::playback::SubtitleStyle,
    pub audio: VideoAudio,
}

/// Uses the installed playback engine's encoder and subtitle renderer.
pub fn encode_video(
    input: &Path,
    output: &Path,
    options: VideoEncoding<'_>,
    progress: &AtomicU32,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    encode_video_edit(
        input.to_str().ok_or("Invalid source path")?,
        output,
        options,
        None,
        progress,
        cancelled,
    )
}

/// Edit playback and encoding share a virtual EDL and an authored ASS track.
/// `video_filter` is generated by the trusted renderer, never copied from a package.
pub fn encode_video_edit(
    source: &str,
    output: &Path,
    options: VideoEncoding<'_>,
    video_filter: Option<&str>,
    progress: &AtomicU32,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    encode_video_edit_range(
        source,
        output,
        options,
        video_filter,
        None,
        progress,
        cancelled,
    )
}

pub fn encode_video_edit_range(
    source: &str,
    output: &Path,
    options: VideoEncoding<'_>,
    video_filter: Option<&str>,
    range: Option<[i64; 2]>,
    progress: &AtomicU32,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    encode_mp4(
        source,
        output,
        options,
        video_filter,
        progress,
        cancelled,
        if range.is_some() {
            "preset=veryfast,crf=20"
        } else {
            "preset=medium,crf=20"
        },
        range,
    )
}

/// A short reusable source for a still picture. Preserve the pixels before the final encode.
pub fn encode_still_tile(source: &str, output: &Path) -> Result<(), String> {
    encode_mp4(
        source,
        output,
        VideoEncoding {
            subtitles: None,
            style: &Default::default(),
            audio: VideoAudio::Original(None),
        },
        None,
        &AtomicU32::new(0),
        &AtomicBool::new(false),
        "preset=ultrafast,crf=0",
        None,
    )
}

/// Cache a fitted picture stream; the material reel provides its audio tracks separately.
pub fn encode_material_picture(
    source: &str,
    output: &Path,
    video_filter: &str,
) -> Result<(), String> {
    encode_mp4(
        source,
        output,
        VideoEncoding {
            subtitles: None,
            style: &Default::default(),
            audio: VideoAudio::Mix(Vec::new()),
        },
        Some(video_filter),
        &AtomicU32::new(0),
        &AtomicBool::new(false),
        "preset=ultrafast,crf=16",
        None,
    )
}

/// Normalize the selected material track, or its default track, to stereo 48 kHz PCM.
pub fn encode_material_audio(
    source: &str,
    output: &Path,
    track: impl Into<Option<i64>>,
) -> Result<(), String> {
    let mpv = MpvHandle::create()?;
    for (key, value) in [
        ("config", "no"),
        ("load-scripts", "no"),
        ("terminal", "no"),
        ("vo", "null"),
        ("vid", "no"),
        ("ao", "lavc"),
        ("o", output.to_str().ok_or("Invalid material audio path")?),
        ("of", "wav"),
        ("oac", "pcm_s16le"),
        ("audio-channels", "stereo"),
        ("audio-samplerate", "48000"),
        ("keep-open", "no"),
        ("idle", "yes"),
        ("audio-file-auto", "no"),
    ] {
        mpv.set_option_string(key, value)?;
    }
    encode_with_audio(
        mpv,
        source,
        output,
        &VideoAudio::Original(track.into()),
        None,
        &AtomicBool::new(false),
        None,
    )
}

fn encode_mp4(
    source: &str,
    output: &Path,
    options: VideoEncoding<'_>,
    video_filter: Option<&str>,
    progress: &AtomicU32,
    cancelled: &AtomicBool,
    encoder_options: &str,
    range: Option<[i64; 2]>,
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
        ("ovcopts", encoder_options),
        ("oac", "aac"),
        // Native AAC in the bundled player can stall on quiet mono input at a
        // fixed 192 kb/s. Quality mode adapts to the signal and channel layout.
        ("oacopts", "flags=+qscale,global_quality=236"),
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
        (
            "sub-ass-override",
            if options
                .subtitles
                .is_some_and(|path| path.extension().is_some_and(|e| e == "ass"))
            {
                "no"
            } else {
                "strip"
            },
        ),
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
        set_files(
            &mpv,
            c"sub-files",
            &[subtitles.to_str().ok_or("Invalid subtitle path")?],
        )?;
    }
    if let Some(filter) = video_filter {
        mpv.set_option_string("vf", filter)?;
    }
    if let Some([start, end]) = range {
        mpv.set_option_string("start", &format!("{:.3}", start as f64 / 1000.))?;
        mpv.set_option_string("end", &format!("{:.3}", end as f64 / 1000.))?;
    }
    encode_with_audio(
        mpv,
        source,
        output,
        &options.audio,
        Some(progress),
        cancelled,
        range.map(|[_, end_ms]| ReviewRange {
            end_ms,
            clock_only: false,
        }),
    )
}

/// Encode an interval on the edited clock with the same track routing, fades and levels as MP4.
pub fn encode_audio_review(
    source: &str,
    output: &Path,
    audio: &VideoAudio,
    [start, end]: [i64; 2],
    clock_only: bool,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    if start < 0 || end <= start || end - start > 15_000 {
        return Err("Choose an audio review interval of up to 15 seconds".into());
    }
    if !super::is_mpv_dll_available() {
        return Err("The media playback runtime is required for audio review".into());
    }
    let mpv = MpvHandle::create()?;
    for (name, value) in [
        ("config", "no"),
        ("load-scripts", "no"),
        ("terminal", "no"),
        ("o", output.to_str().ok_or("Invalid audio review path")?),
        ("of", "wav"),
        ("oac", "pcm_s16le"),
        ("ao", "lavc"),
        ("vid", "no"),
        ("vo", "null"),
        ("sid", "no"),
        ("audio-file-auto", "no"),
        ("keep-open", "no"),
        ("idle", "yes"),
    ] {
        mpv.set_option_string(name, value)?;
    }
    mpv.set_option_string("start", &format!("{:.3}", start as f64 / 1000.))?;
    mpv.set_option_string("end", &format!("{:.3}", end as f64 / 1000.))?;
    encode_with_audio(
        mpv,
        source,
        output,
        audio,
        None,
        cancelled,
        Some(ReviewRange {
            end_ms: end,
            clock_only,
        }),
    )
}

#[derive(Clone, Copy)]
struct ReviewRange {
    end_ms: i64,
    // A picture project supplies a seekable silent clock instead of decoding its images.
    clock_only: bool,
}

fn encode_with_audio(
    mpv: MpvHandle,
    source: &str,
    output: &Path,
    audio: &VideoAudio,
    progress: Option<&AtomicU32>,
    cancelled: &AtomicBool,
    review: Option<ReviewRange>,
) -> Result<(), String> {
    match audio {
        VideoAudio::Original(track) => {
            if let Some(track) = track {
                mpv.set_option_string("aid", &track.to_string())?;
            }
        }
        selection => {
            // Changing sample rates or channel layouts inside an EDL restarts mpv's
            // mixing graph. Its flushed buffers can overlap the next buffer's PTS.
            // The final filter stays alive across those restarts and preserves the
            // edited clock in both the AAC export and the PCM review excerpt.
            mpv.set_option_string("af", "lavfi=[aresample=async=1:min_hard_comp=0.001]")?;
            for source in selection.sources() {
                if let VideoAudio::External(path) = source
                    && !path.is_file()
                {
                    return Err(format!("Audio file is missing: {}", path.display()));
                }
            }
            let files: Vec<_> = selection
                .sources()
                .into_iter()
                .filter_map(VideoAudio::external_uri)
                .collect();
            if !files.is_empty() {
                set_files(
                    &mpv,
                    c"audio-files",
                    &files.iter().map(|uri| uri.as_ref()).collect::<Vec<_>>(),
                )?;
            }
            // Never fall back to the source voices if an external track fails.
            mpv.set_option_string("aid", "no")?;
        }
    }
    mpv.initialize()?;
    unsafe { libmpv_sys::mpv_request_log_messages(mpv.handle, c"error".as_ptr()) };
    if review.is_some() || !matches!(audio, VideoAudio::Original(_)) {
        let error = unsafe { libmpv_sys::mpv_hook_add(mpv.handle, 0, c"on_preloaded".as_ptr(), 0) };
        if error < 0 {
            return Err(format!("Cannot prepare export audio: {error}"));
        }
    }
    mpv.command(&["loadfile", source])?;
    wait_for_encoding(mpv, output, progress, cancelled, Some((audio, review)))
}

// Native arrays preserve commas, Unicode and other punctuation in file paths.
fn set_files(mpv: &MpvHandle, name: &std::ffi::CStr, paths: &[&str]) -> Result<(), String> {
    let strings = paths
        .iter()
        .map(|path| std::ffi::CString::new(*path).map_err(|e| e.to_string()))
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

pub(super) fn read_audio_tracks(mpv: &MpvHandle) -> Result<Vec<super::super::AudioTrack>, String> {
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
    Ok(tracks)
}

fn select_export_audio(
    mpv: &MpvHandle,
    audio: &VideoAudio,
    review: Option<ReviewRange>,
) -> Result<(), String> {
    let mut tracks = read_audio_tracks(mpv)?;
    let mut silent_clock = None;
    if review.is_some_and(|r| r.clock_only) {
        silent_clock = tracks.iter().find(|t| t.external.is_none()).map(|t| t.id);
        tracks.retain(|t| t.external.is_some());
    }
    if matches!(audio, VideoAudio::Original(_)) && !tracks.is_empty() {
        return Ok(()); // Keep the default or explicitly selected source track.
    }
    let mut inputs = Vec::new();
    for (source, settings) in audio.inputs() {
        let Some(track) = super::tracks::resolve(source, &tracks) else {
            if matches!(source, VideoAudio::Original(None)) {
                continue;
            }
            return Err("Could not load the selected export audio".into());
        };
        inputs.push((track.id, settings.cloned().unwrap_or_default()));
    }
    inputs.sort_by_key(|(id, _)| *id);
    inputs.dedup_by_key(|(id, _)| *id);
    if inputs.is_empty()
        && let Some(id) = silent_clock
    {
        inputs.push((id, Default::default()));
    }
    if inputs.is_empty()
        && let Some(range) = review
    {
        mpv.set_property_string(
            "lavfi-complex",
            &format!(
                "anullsrc=r=48000:cl=stereo:d={:.3}[ao]",
                range.end_ms as f64 / 1000.
            ),
        )?;
    } else {
        super::tracks::apply(mpv, &inputs)?;
    }
    // A moved speech clip must not extend the original video's duration.
    if review.is_none()
        && !inputs.is_empty()
        && let Some(duration) = mpv.get_property_f64("duration").filter(|d| *d > 0.0)
    {
        mpv.command(&["set", "end", &duration.to_string()])?;
    }
    Ok(())
}

fn wait_for_encoding(
    mpv: MpvHandle,
    output: &Path,
    progress: Option<&AtomicU32>,
    cancelled: &AtomicBool,
    audio: Option<(&VideoAudio, Option<ReviewRange>)>,
) -> Result<(), String> {
    let mut details = Vec::new();
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Media encoding cancelled".into());
        }
        let event = unsafe { &*libmpv_sys::mpv_wait_event(mpv.handle, 0.1) };
        if event.event_id == libmpv_sys::mpv_event_id_MPV_EVENT_LOG_MESSAGE && details.len() < 3 {
            let message = unsafe { &*(event.data as *const libmpv_sys::mpv_event_log_message) };
            let text = unsafe { std::ffi::CStr::from_ptr(message.text) }.to_string_lossy();
            let line: String = text.trim().chars().take(200).collect();
            if !line.is_empty() && !details.contains(&line) {
                details.push(line);
            }
        }
        if event.event_id == libmpv_sys::mpv_event_id_MPV_EVENT_HOOK {
            let hook = unsafe { &*(event.data as *const libmpv_sys::mpv_event_hook) };
            let result = audio.map_or(Ok(()), |(audio, end)| select_export_audio(&mpv, audio, end));
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
            let detail = if details.is_empty() {
                String::new()
            } else {
                format!(": {}", details.join(" · "))
            };
            return Err(format!("Media encoding failed ({}){detail}", end.error));
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
    fn mixed_format_sound_edits_keep_contiguous_samples_and_source_time() {
        use symphonia::core::{audio::SampleBuffer, io::MediaSourceStream, probe::Hint};

        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.mp4");
        let silence = dir.path().join("stereo.wav");
        let tap = dir.path().join("mono.wav");
        let progress = AtomicU32::new(0);
        let cancel = AtomicBool::new(false);
        let style = SubtitleStyle::default();
        let encoding = |audio| VideoEncoding {
            audio,
            subtitles: None,
            style: &style,
        };
        let input = concat!(
            "av://lavfi:color=c=black:s=96x64:r=30:d=4[out0];",
            "aevalsrc='0.35*sin(2*PI*420*t)*(between(t,2.58,2.78)+between(t,3.19,3.39))':s=48000:d=4[out1]"
        );
        encode_video(
            Path::new(input),
            &source,
            encoding(VideoAudio::default()),
            &progress,
            &cancel,
        )
        .unwrap();
        encode_material_audio("av://lavfi:anullsrc=r=48000:cl=stereo:d=4", &silence, 1).unwrap();
        encode_audio(
            Path::new("av://lavfi:sine=frequency=1200:sample_rate=44100:duration=0.24"),
            &tap,
            1.,
            &cancel,
        )
        .unwrap();
        let segment = |file: &Path, start: i64, length: i64| {
            let file = file.to_str().unwrap();
            format!(
                "%{}%{file},{:.3},{:.3}",
                file.len(),
                start as f64 / 1000.,
                length as f64 / 1000.
            )
        };
        let picture = format!(
            "edl://{};{}",
            segment(&source, 250, 1250),
            segment(&source, 1700, 1800)
        );
        let mut tracks = vec![VideoAudio::default()];
        for at in [160, 410, 780, 1350, 1710] {
            tracks.push(VideoAudio::ExternalStream(format!(
                "edl://{};{};{}",
                segment(&silence, 0, at),
                segment(&tap, 0, 120),
                segment(&silence, 0, 3050 - at - 120)
            )));
        }
        let audio = VideoAudio::Mix(tracks);
        let video = dir.path().join("mix.mp4");
        let review = dir.path().join("passage.wav");
        encode_video_edit(
            &picture,
            &video,
            encoding(audio.clone()),
            Some("lavfi=[fps=25]"),
            &progress,
            &cancel,
        )
        .unwrap();
        encode_audio_review(&picture, &review, &audio, [1300, 3000], false, &cancel).unwrap();
        for (file, origin_ms) in [(&video, 0), (&review, 1300)] {
            // Decode consecutive packet contents, as a PCM or transcription consumer does.
            let stream = MediaSourceStream::new(
                Box::new(std::fs::File::open(file).unwrap()),
                Default::default(),
            );
            let mut format = symphonia::default::get_probe()
                .format(
                    &Hint::new(),
                    stream,
                    &Default::default(),
                    &Default::default(),
                )
                .unwrap()
                .format;
            let track = format
                .tracks()
                .iter()
                .find(|t| t.codec_params.sample_rate.is_some())
                .unwrap();
            let id = track.id;
            let rate = track.codec_params.sample_rate.unwrap() as usize;
            let mut decoder = symphonia::default::get_codecs()
                .make(&track.codec_params, &Default::default())
                .unwrap();
            let mut timestamps = Vec::new();
            let mut pcm = Vec::new();
            while let Ok(packet) = format.next_packet() {
                if packet.track_id() != id {
                    continue;
                }
                timestamps.push(packet.ts());
                let frame = decoder.decode(&packet).unwrap();
                let spec = *frame.spec();
                let mut samples = SampleBuffer::<f32>::new(frame.capacity() as u64, spec);
                samples.copy_interleaved_ref(frame);
                pcm.extend(
                    samples
                        .samples()
                        .chunks_exact(spec.channels.count())
                        .map(|frame| frame[0]),
                );
            }
            if file == &video {
                assert!(timestamps.len() > 120);
                assert!(
                    timestamps.windows(2).all(|pair| pair[1] - pair[0] == 1024),
                    "Every AAC frame must have one distinct 1024-sample interval"
                );
            }
            // Source 2580 and 3190 map to edited 2130 and 2740 after the second cut.
            for (ms, audible) in [
                (2050, false),
                (2210, true),
                (2440, false),
                (2820, true),
                (2980, false),
            ] {
                let start = (ms - origin_ms) * rate / 1000;
                let peak = pcm[start..start + rate / 100]
                    .iter()
                    .fold(0.0_f32, |v, x| v.max(x.abs()));
                assert_eq!(peak > 0.1, audible, "{} at {ms} ms", file.display());
            }
        }
    }

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
