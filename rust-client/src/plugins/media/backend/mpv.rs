#![cfg(feature = "mpv")]

use super::{MediaBackend, PlaybackStatus};
use std::ffi::{CStr, CString};
use std::path::PathBuf;
mod encoding;
mod tracks;
pub(crate) use encoding::encode_video;
#[cfg(test)]
mod tests;

struct MpvHandle {
    handle: *mut libmpv_sys::mpv_handle,
}

unsafe impl Send for MpvHandle {}
unsafe impl Sync for MpvHandle {}

impl Drop for MpvHandle {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                libmpv_sys::mpv_terminate_destroy(self.handle);
            }
        }
    }
}

impl MpvHandle {
    fn create() -> Result<Self, String> {
        let handle = unsafe { libmpv_sys::mpv_create() };
        if handle.is_null() {
            return Err("Failed to create MPV handle".into());
        }

        Ok(Self { handle })
    }

    fn initialize(&self) -> Result<(), String> {
        let error = unsafe { libmpv_sys::mpv_initialize(self.handle) };
        if error < 0 {
            Err(format!("mpv_initialize failed: {error}"))
        } else {
            Ok(())
        }
    }

    fn new() -> Result<Self, String> {
        let instance = Self::create()?;
        #[cfg(any(windows, target_os = "linux"))]
        instance.set_option_string("vo", "libmpv")?;
        instance.set_option_string("hwdec", "auto-safe")?;
        instance.set_option_string("keep-open", "always")?;
        instance.set_option_string("msg-level", "all=error")?;
        instance.set_option_string("input-default-bindings", "no")?;
        instance.set_option_string("input-vo-keyboard", "no")?;
        instance.set_option_string("audio-channels", "stereo")?;
        instance.set_option_string("audio-normalize-downmix", "yes")?;
        // The media timeline owns subtitles; sidecar/embedded tracks must not
        // render a second copy beneath its overlay.
        instance.set_option_string("sub-auto", "no")?;
        instance.set_option_string("sid", "no")?;
        let subtitle_style = super::super::subtitles::SubtitleStyle::default();
        instance.set_option_string("osd-font", &subtitle_style.font)?;
        instance.set_option_string("osd-scale-by-window", "yes")?;
        instance.set_option_string("osd-font-size", &subtitle_style.size.to_string())?;
        instance.set_option_string("osd-color", &subtitle_style.rgb())?;
        instance.set_option_string("osd-border-color", "#000000")?;
        instance.set_option_string("osd-border-size", "1.5")?;
        instance.set_option_string("osd-shadow-offset", "1")?;
        instance.set_option_string("osd-align-y", "bottom")?;
        instance.set_option_string("osd-align-x", "center")?;
        instance.set_option_string("osd-margin-y", "24")?;
        instance.set_option_string("osd-margin-x", "24")?;

        // The render API never waits on the UI's graphics context for an audio
        // clock deadline. Let mpv schedule callbacks at the actual display time.
        instance.set_option_string("video-timing-offset", "0")?;
        instance.initialize()?;
        instance.observe_properties()?;

        Ok(instance)
    }

    // Export owns a separate worker/core and needs the synchronous result.
    fn command(&self, args: &[&str]) -> Result<(), String> {
        self.submit_command(args, false)
    }

    fn command_async(&self, args: &[&str]) -> Result<(), String> {
        self.submit_command(args, true)
    }

    fn submit_command(&self, args: &[&str], asynchronous: bool) -> Result<(), String> {
        let cstrings: Result<Vec<CString>, _> = args.iter().map(|s| CString::new(*s)).collect();
        let cstrings = cstrings.map_err(|e| e.to_string())?;
        let mut ptrs: Vec<*const std::os::raw::c_char> =
            cstrings.iter().map(|cs| cs.as_ptr()).collect();
        ptrs.push(std::ptr::null());

        let err = unsafe {
            if asynchronous {
                libmpv_sys::mpv_command_async(
                    self.handle,
                    u64::from(args.first() == Some(&"loadfile")),
                    ptrs.as_mut_ptr(),
                )
            } else {
                libmpv_sys::mpv_command(self.handle, ptrs.as_mut_ptr())
            }
        };
        if err < 0 {
            let err_str = unsafe {
                let s = libmpv_sys::mpv_error_string(err);
                if s.is_null() {
                    format!("MPV error {}", err)
                } else {
                    CStr::from_ptr(s).to_string_lossy().into_owned()
                }
            };
            return Err(err_str);
        }
        Ok(())
    }

    fn set_option_string(&self, name: &str, val: &str) -> Result<(), String> {
        let name = CString::new(name).map_err(|e| e.to_string())?;
        let value = CString::new(val).map_err(|e| e.to_string())?;
        let error = unsafe {
            libmpv_sys::mpv_set_option_string(self.handle, name.as_ptr(), value.as_ptr())
        };
        if error < 0 {
            Err(format!("Failed to configure MPV: {error}"))
        } else {
            Ok(())
        }
    }

    fn set_property_string(&self, name: &str, val: &str) -> Result<(), String> {
        let c_name = CString::new(name).map_err(|e| e.to_string())?;
        let c_val = CString::new(val).map_err(|e| e.to_string())?;
        let err = unsafe {
            let mut value = c_val.as_ptr();
            libmpv_sys::mpv_set_property_async(
                self.handle,
                0,
                c_name.as_ptr(),
                libmpv_sys::mpv_format_MPV_FORMAT_STRING,
                &mut value as *mut _ as *mut _,
            )
        };
        if err < 0 {
            Err(format!("Failed to set property {}", name))
        } else {
            Ok(())
        }
    }

    fn set_property_bool(&self, name: &str, val: bool) -> Result<(), String> {
        let c_name = CString::new(name).map_err(|e| e.to_string())?;
        let mut flag: std::os::raw::c_int = if val { 1 } else { 0 };
        let err = unsafe {
            libmpv_sys::mpv_set_property_async(
                self.handle,
                0,
                c_name.as_ptr(),
                libmpv_sys::mpv_format_MPV_FORMAT_FLAG,
                &mut flag as *mut _ as *mut std::ffi::c_void,
            )
        };
        if err < 0 {
            Err(format!("Failed to set property {}", name))
        } else {
            Ok(())
        }
    }

    fn set_property_f64(&self, name: &str, val: f64) -> Result<(), String> {
        let c_name = CString::new(name).map_err(|e| e.to_string())?;
        let mut v = val;
        let err = unsafe {
            libmpv_sys::mpv_set_property_async(
                self.handle,
                0,
                c_name.as_ptr(),
                libmpv_sys::mpv_format_MPV_FORMAT_DOUBLE,
                &mut v as *mut _ as *mut std::ffi::c_void,
            )
        };
        if err < 0 {
            Err(format!("Failed to set property {}", name))
        } else {
            Ok(())
        }
    }

    // Only the offline export worker polls synchronously; playback uses events.
    fn get_property_f64(&self, name: &str) -> Option<f64> {
        let name = CString::new(name).ok()?;
        let mut value = 0.0;
        let error = unsafe {
            libmpv_sys::mpv_get_property(
                self.handle,
                name.as_ptr(),
                libmpv_sys::mpv_format_MPV_FORMAT_DOUBLE,
                &mut value as *mut _ as *mut _,
            )
        };
        (error >= 0).then_some(value)
    }

    fn observed_properties() -> [(&'static CStr, libmpv_sys::mpv_format); 14] {
        use libmpv_sys::*;
        [
            (c"track-list", mpv_format_MPV_FORMAT_NODE),
            (c"time-pos", mpv_format_MPV_FORMAT_DOUBLE),
            (c"duration", mpv_format_MPV_FORMAT_DOUBLE),
            (c"eof-reached", mpv_format_MPV_FORMAT_FLAG),
            (c"pause", mpv_format_MPV_FORMAT_FLAG),
            (c"audio-params/channel-count", mpv_format_MPV_FORMAT_INT64),
            (c"audio-params/hr-channels", mpv_format_MPV_FORMAT_STRING),
            (c"hwdec-current", mpv_format_MPV_FORMAT_STRING),
            (c"video-format", mpv_format_MPV_FORMAT_STRING),
            (c"dwidth", mpv_format_MPV_FORMAT_INT64),
            (c"dheight", mpv_format_MPV_FORMAT_INT64),
            (c"estimated-vf-fps", mpv_format_MPV_FORMAT_DOUBLE),
            (c"container-fps", mpv_format_MPV_FORMAT_DOUBLE),
            (c"vo-drop-frame-count", mpv_format_MPV_FORMAT_INT64),
        ]
    }

    fn observe_properties(&self) -> Result<(), String> {
        for (name, format) in Self::observed_properties() {
            let error =
                unsafe { libmpv_sys::mpv_observe_property(self.handle, 0, name.as_ptr(), format) };
            if error < 0 {
                return Err(format!("Cannot observe MPV state: {error}"));
            }
        }
        Ok(())
    }

    fn refresh_properties(&self) {
        // Some nested properties (notably audio-params/*) do not notify when
        // restored after a reload. Reconfiguration requests a fresh snapshot,
        // asynchronously and once per event batch, never once per UI getter.
        for (name, format) in Self::observed_properties() {
            let error = unsafe {
                libmpv_sys::mpv_get_property_async(self.handle, 0, name.as_ptr(), format)
            };
            if error < 0 {
                log::warn!(
                    "Cannot request MPV state {}: {error}",
                    name.to_string_lossy()
                );
            }
        }
    }
}

// Owned snapshots of property events. Getters used by layout, subtitles and
// scrolling never synchronously wait for the player core or video renderer.
#[derive(Default)]
struct PlaybackSnapshot {
    audio_tracks: Vec<super::AudioTrack>,
    time_ms: i64,
    duration_ms: i64,
    eof: bool,
    paused: Option<bool>,
    channels: Option<usize>,
    layout: Option<String>,
    diagnostics: super::PlayerDiagnostics,
    estimated_fps: Option<f64>,
    container_fps: Option<f64>,
}

impl PlaybackSnapshot {
    /// The event and its payload are valid only until the next mpv_wait_event.
    unsafe fn update(&mut self, property: &libmpv_sys::mpv_event_property) {
        use libmpv_sys::*;
        if property.name.is_null() {
            return;
        }
        let name = unsafe { CStr::from_ptr(property.name) }.to_bytes();
        let flag = (!property.data.is_null() && property.format == mpv_format_MPV_FORMAT_FLAG)
            .then(|| unsafe { *(property.data as *const i32) != 0 });
        let number = (!property.data.is_null() && property.format == mpv_format_MPV_FORMAT_DOUBLE)
            .then(|| unsafe { *(property.data as *const f64) })
            .filter(|value| value.is_finite());
        let integer = (!property.data.is_null() && property.format == mpv_format_MPV_FORMAT_INT64)
            .then(|| unsafe { *(property.data as *const i64) });
        let string = || {
            if property.data.is_null() || property.format != mpv_format_MPV_FORMAT_STRING {
                return None;
            }
            let value = unsafe { *(property.data as *const *const std::ffi::c_char) };
            (!value.is_null()).then(|| {
                unsafe { CStr::from_ptr(value) }
                    .to_string_lossy()
                    .into_owned()
            })
        };
        match name {
            b"track-list"
                if !property.data.is_null() && property.format == mpv_format_MPV_FORMAT_NODE =>
            {
                self.audio_tracks = unsafe { tracks::read(&*(property.data as *const mpv_node)) };
            }
            b"time-pos" => {
                self.time_ms = number.map(|seconds| (seconds * 1000.0) as i64).unwrap_or(0)
            }
            b"duration" => {
                self.duration_ms = number.map(|seconds| (seconds * 1000.0) as i64).unwrap_or(0)
            }
            b"eof-reached" => self.eof = flag.unwrap_or(false),
            b"pause" => self.paused = flag,
            b"audio-params/channel-count" => {
                self.channels = integer
                    .filter(|&count| count > 0)
                    .map(|count| count as usize)
            }
            b"audio-params/hr-channels" => self.layout = string(),
            b"hwdec-current" => {
                self.diagnostics.hwdec_current = string().unwrap_or_else(|| "no".into())
            }
            b"video-format" => self.diagnostics.video_codec = string().unwrap_or_default(),
            b"dwidth" => self.diagnostics.width = integer.unwrap_or(0),
            b"dheight" => self.diagnostics.height = integer.unwrap_or(0),
            b"estimated-vf-fps" => self.estimated_fps = number.filter(|&fps| fps > 0.0),
            b"container-fps" => self.container_fps = number.filter(|&fps| fps > 0.0),
            b"vo-drop-frame-count" => self.diagnostics.dropped_frames = integer.unwrap_or(0),
            _ => {}
        }
        self.diagnostics.fps = self.estimated_fps.or(self.container_fps).unwrap_or(0.0);
    }
}

pub struct MpvBackend {
    // Render context must be freed (and its worker joined) before the mpv core.
    #[cfg(any(windows, target_os = "linux"))]
    video: Option<super::gpu::VideoRenderer>,
    mpv: MpvHandle,
    status: PlaybackStatus,
    snapshot: PlaybackSnapshot,
    error: Option<String>,
    active_channels: Vec<super::super::task::AudioChannelItem>,
    audio_selection: super::PlaybackAudio,
    loading: bool,
    subtitle_style: super::super::subtitles::SubtitleStyle,
    subtitle_text: String,
}

fn reconcile_playback_status(
    current: PlaybackStatus,
    end_file_event: bool,
    eof_reached: bool,
    paused: Option<bool>,
) -> PlaybackStatus {
    if end_file_event || eof_reached {
        return PlaybackStatus::Stopped;
    }
    match (current, paused) {
        (PlaybackStatus::Playing, Some(true)) => PlaybackStatus::Paused,
        (PlaybackStatus::Paused, Some(false)) => PlaybackStatus::Playing,
        _ => current,
    }
}

#[cfg(windows)]
fn is_mpv_dll_available() -> bool {
    use windows::Win32::System::LibraryLoader::LoadLibraryW;
    use windows::core::w;
    unsafe {
        if let Ok(handle) = LoadLibraryW(w!("mpv-2.dll")) {
            let _ = windows::Win32::Foundation::FreeLibrary(handle);
            true
        } else if let Ok(handle) = LoadLibraryW(w!("libmpv-2.dll")) {
            let _ = windows::Win32::Foundation::FreeLibrary(handle);
            true
        } else {
            false
        }
    }
}

#[cfg(not(windows))]
fn is_mpv_dll_available() -> bool {
    true
}

impl MpvBackend {
    pub fn new() -> Result<Self, String> {
        if !is_mpv_dll_available() {
            return Err("libmpv runtime (mpv-2.dll) not found".into());
        }

        let mpv = MpvHandle::new()?;

        Ok(Self {
            #[cfg(any(windows, target_os = "linux"))]
            video: None,
            mpv,
            status: PlaybackStatus::Stopped,
            snapshot: PlaybackSnapshot::default(),
            error: None,
            active_channels: Vec::new(),
            audio_selection: Default::default(),
            loading: false,
            subtitle_style: Default::default(),
            subtitle_text: String::new(),
        })
    }

    fn load(&mut self, source: &str) -> Result<(), String> {
        self.mpv.command_async(&["loadfile", source])?;
        self.loading = true;
        self.snapshot.audio_tracks.clear();
        self.snapshot.channels = None;
        // Observations only report changed values. Keep metadata and pause state
        // across loads, or an unchanged duration/channel count/pause value would
        // be lost when reopening the same file. New values arrive via events.
        self.snapshot.time_ms = 0;
        self.snapshot.eof = false;
        self.error = None;
        self.status = if self.snapshot.paused == Some(true) {
            PlaybackStatus::Paused
        } else {
            PlaybackStatus::Playing
        };
        Ok(())
    }
}

impl MediaBackend for MpvBackend {
    fn audio_tracks(&self) -> &[super::AudioTrack] {
        &self.snapshot.audio_tracks
    }

    fn select_audio(&mut self, selection: super::PlaybackAudio) -> Result<(), String> {
        if self.audio_selection != selection {
            self.snapshot.channels = None;
        }
        self.audio_selection = selection;
        if self.loading {
            return Ok(());
        }
        match &self.audio_selection {
            super::PlaybackAudio::Original(id) => self.mpv.set_property_string(
                "aid",
                &id.map(|id| id.to_string()).unwrap_or_else(|| "auto".into()),
            ),
            super::PlaybackAudio::External(path) => {
                if let Some(track) = self
                    .snapshot
                    .audio_tracks
                    .iter()
                    .find(|t| t.external.as_ref() == Some(path))
                {
                    self.mpv.set_property_string("aid", &track.id.to_string())
                } else {
                    self.mpv
                        .command_async(&["audio-add", &path.to_string_lossy(), "select"])?;
                    for track in self
                        .snapshot
                        .audio_tracks
                        .iter()
                        .filter(|track| track.external.is_some())
                    {
                        self.mpv
                            .command_async(&["audio-remove", &track.id.to_string()])?;
                    }
                    Ok(())
                }
            }
        }
    }

    fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }

    fn load_local_file(&mut self, path: PathBuf) -> Result<(), String> {
        self.load(&path.to_string_lossy())
    }

    fn load_stream_url(&mut self, url: String) -> Result<(), String> {
        self.load(&url)
    }

    fn play(&mut self) {
        if self.snapshot.eof || self.status == PlaybackStatus::Stopped {
            let _ = self.mpv.command_async(&["seek", "0", "absolute"]);
            self.snapshot.eof = false;
        }
        self.snapshot.paused = Some(false);
        let _ = self.mpv.set_property_bool("pause", false);
        self.status = PlaybackStatus::Playing;
    }

    fn pause(&mut self) {
        self.snapshot.paused = Some(true);
        let _ = self.mpv.set_property_bool("pause", true);
        self.status = PlaybackStatus::Paused;
    }

    fn stop(&mut self) {
        let _ = self.mpv.command_async(&["stop"]);
        let _ = self.mpv.command_async(&["show-text", "", "0", "1"]);
        self.subtitle_text.clear();
        self.status = PlaybackStatus::Stopped;
    }

    fn seek(&mut self, ms: i64) {
        let was_stopped = self.status == PlaybackStatus::Stopped;
        if was_stopped {
            let _ = self.mpv.set_property_bool("pause", true);
        }
        self.snapshot.time_ms = ms.max(0);
        self.snapshot.eof = false;
        let secs = (ms.max(0) as f64) / 1000.0;
        let _ = self
            .mpv
            .command_async(&["seek", &secs.to_string(), "absolute"]);
        if was_stopped {
            self.status = PlaybackStatus::Paused;
        }
    }

    fn set_volume(&mut self, volume: f32) {
        let _ = self.mpv.set_property_f64("volume", (volume * 100.0) as f64);
    }

    fn set_mute(&mut self, mute: bool) {
        let _ = self.mpv.set_property_bool("mute", mute);
    }

    fn get_time_ms(&self) -> i64 {
        self.snapshot.time_ms
    }
    fn get_duration_ms(&self) -> i64 {
        self.snapshot.duration_ms
    }
    fn get_status(&self) -> PlaybackStatus {
        self.status
    }
    fn get_diagnostics(&self) -> super::PlayerDiagnostics {
        self.snapshot.diagnostics.clone()
    }

    fn tick(&mut self) {
        use libmpv_sys::{
            mpv_error_string, mpv_event_end_file,
            mpv_event_id_MPV_EVENT_AUDIO_RECONFIG as AUDIO_RECONFIG,
            mpv_event_id_MPV_EVENT_COMMAND_REPLY as COMMAND_REPLY,
            mpv_event_id_MPV_EVENT_END_FILE as END_FILE,
            mpv_event_id_MPV_EVENT_FILE_LOADED as FILE_LOADED,
            mpv_event_id_MPV_EVENT_GET_PROPERTY_REPLY as GET_PROPERTY_REPLY,
            mpv_event_id_MPV_EVENT_NONE as NONE,
            mpv_event_id_MPV_EVENT_PLAYBACK_RESTART as PLAYBACK_RESTART,
            mpv_event_id_MPV_EVENT_PROPERTY_CHANGE as PROPERTY_CHANGE,
            mpv_event_id_MPV_EVENT_VIDEO_RECONFIG as VIDEO_RECONFIG, mpv_event_property,
            mpv_wait_event,
        };
        let mut end_file_event = false;
        let mut refresh = false;
        // A bounded, non-blocking drain keeps bursts of property events from
        // monopolizing a UI frame. mpv coalesces changes to observed properties.
        for _ in 0..256 {
            let event = unsafe { mpv_wait_event(self.mpv.handle, 0.0).as_ref() };
            let Some(event) = event else { break };
            match event.event_id {
                NONE => break,
                PROPERTY_CHANGE | GET_PROPERTY_REPLY if !event.data.is_null() => {
                    unsafe {
                        self.snapshot
                            .update(&*(event.data as *const mpv_event_property))
                    };
                }
                END_FILE => {
                    end_file_event = true;
                    if let Some(end) = unsafe { (event.data as *const mpv_event_end_file).as_ref() }
                        && end.error < 0
                    {
                        self.error = Some(
                            unsafe { CStr::from_ptr(mpv_error_string(end.error)) }
                                .to_string_lossy()
                                .into_owned(),
                        );
                    }
                }
                FILE_LOADED => {
                    self.loading = false;
                    if let Err(error) = self.select_audio(self.audio_selection.clone()) {
                        self.error = Some(error);
                    }
                    end_file_event = false;
                    refresh = true;
                    self.snapshot.eof = false;
                    self.status = if self.snapshot.paused == Some(true) {
                        PlaybackStatus::Paused
                    } else {
                        PlaybackStatus::Playing
                    };
                    // Apply routing once per loaded file, not on PLAYBACK_RESTART:
                    // changing audio filters can itself restart playback.
                    if !self.active_channels.is_empty() {
                        self.set_channel_routing(&self.active_channels.clone());
                    }
                }
                PLAYBACK_RESTART => {
                    end_file_event = false;
                    refresh = true;
                }
                AUDIO_RECONFIG | VIDEO_RECONFIG => refresh = true,
                COMMAND_REPLY if event.error < 0 => {
                    self.error = Some(
                        unsafe { CStr::from_ptr(mpv_error_string(event.error)) }
                            .to_string_lossy()
                            .into_owned(),
                    );
                    self.status = PlaybackStatus::Stopped;
                }
                _ => {}
            }
        }
        if refresh {
            self.mpv.refresh_properties();
        }
        self.status = reconcile_playback_status(
            self.status,
            end_file_event,
            self.snapshot.eof,
            self.snapshot.paused,
        );
    }

    #[cfg(any(windows, target_os = "linux"))]
    fn initialize_video(
        &mut self,
        state: &eframe::egui_wgpu::RenderState,
        ctx: &eframe::egui::Context,
        display: Option<super::VideoDisplay>,
    ) -> Result<(), String> {
        if self.video.is_none() {
            self.video = Some(super::gpu::VideoRenderer::new(
                self.mpv.handle,
                state,
                ctx,
                display,
            )?);
        }
        Ok(())
    }

    #[cfg(any(windows, target_os = "linux"))]
    fn video_frame(&mut self, size: [u32; 2]) -> Result<Option<eframe::egui::TextureId>, String> {
        self.video
            .as_mut()
            .map(|video| video.frame(size))
            .transpose()
    }

    #[cfg(any(windows, target_os = "linux"))]
    fn set_video_visible(&mut self, visible: bool) {
        if let Some(video) = &self.video {
            video.set_visible(visible);
        }
    }

    fn set_osd_subtitle(&mut self, text: &str) {
        if self.subtitle_text != text {
            let duration = if text.is_empty() { "0" } else { "2147483647" };
            if self
                .mpv
                .command_async(&["show-text", text, duration, "1"])
                .is_ok()
            {
                self.subtitle_text = text.to_owned();
            }
        }
    }

    fn set_subtitle_style(&mut self, style: &super::super::subtitles::SubtitleStyle) {
        if &self.subtitle_style != style {
            let _ = self.mpv.set_property_string("osd-font", &style.font);
            let _ = self
                .mpv
                .set_property_f64("osd-font-size", style.size as f64);
            let _ = self.mpv.set_property_string("osd-color", &style.rgb());
            self.subtitle_style = style.clone();
        }
    }

    fn get_audio_channel_count(&self) -> Option<usize> {
        self.snapshot.channels
    }
    fn get_audio_layout(&self) -> Option<String> {
        self.snapshot.layout.clone()
    }

    fn set_channel_routing(&mut self, channels: &[super::super::task::AudioChannelItem]) {
        self.active_channels = channels.to_vec();

        if channels.is_empty() {
            let _ = self.mpv.set_property_string("af", "");
            return;
        }

        let all_enabled = channels.iter().all(|c| c.playback);
        if all_enabled {
            let _ = self.mpv.set_property_string("af", "");
            return;
        }

        let mut left_terms = Vec::new();
        let mut right_terms = Vec::new();

        for ch in channels {
            if !ch.playback {
                continue;
            }
            let idx = ch.index;
            if ch.is_left {
                left_terms.push(format!("1.0*c{}", idx));
            }
            if ch.is_right {
                right_terms.push(format!("1.0*c{}", idx));
            }
            if ch.is_center {
                if ch.id == "fc" {
                    left_terms.push(format!("0.707*c{}", idx));
                    right_terms.push(format!("0.707*c{}", idx));
                } else if ch.id == "lfe" {
                    left_terms.push(format!("0.2*c{}", idx));
                    right_terms.push(format!("0.2*c{}", idx));
                } else {
                    left_terms.push(format!("1.0*c{}", idx));
                    right_terms.push(format!("1.0*c{}", idx));
                }
            }
        }

        let c0_expr = if left_terms.is_empty() {
            "c0=0".to_string()
        } else {
            format!("c0={}", left_terms.join("+"))
        };

        let c1_expr = if right_terms.is_empty() {
            "c1=0".to_string()
        } else {
            format!("c1={}", right_terms.join("+"))
        };

        let filter = format!("lavfi=[pan=stereo|{}|{}]", c0_expr, c1_expr);
        let res = self.mpv.set_property_string("af", &filter);
        if let Err(e) = res {
            log::error!("Failed to apply MPV audio filter '{}': {}", filter, e);
        } else {
            log::info!("Applied MPV audio filter: {}", filter);
        }
    }

    fn set_audio_only_mode(&mut self, enabled: bool) {
        let _ = self
            .mpv
            .set_property_string("vid", if enabled { "no" } else { "auto" });
    }
}
