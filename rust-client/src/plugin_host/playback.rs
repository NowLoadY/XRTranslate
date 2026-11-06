//! Generic playback resources. GPU video stays in the rendering process.
use super::*;
use xrtranslate_client_core::playback::{
    self, MediaBackend,
    remote::{Command, Snapshot},
};
struct Player {
    backend: Box<dyn MediaBackend>,
    size: [u32; 2],
    initialized: bool,
    error: Option<String>,
    last: Value,
}
#[derive(Default)]
pub(crate) struct Players {
    players: BTreeMap<(String, u64), Player>,
    pub graphics: Option<(
        eframe::egui_wgpu::RenderState,
        egui::Context,
        Option<playback::VideoDisplay>,
    )>,
}
impl Players {
    pub fn remove(&mut self, plugin: &str) {
        self.players.retain(|(owner, _), _| owner != plugin);
    }
    pub fn command(&mut self, plugin: &str, id: u64, command: Command) -> Result<(), String> {
        let key = (plugin.to_owned(), id);
        if matches!(command, Command::Close) {
            self.players.remove(&key);
            return Ok(());
        }
        if matches!(command, Command::Open) {
            if self.players.contains_key(&key) {
                return Ok(());
            }
            if self
                .players
                .keys()
                .filter(|(owner, _)| owner == plugin)
                .count()
                >= 8
            {
                return Err("Too many open players".into());
            }
            self.players.insert(
                key,
                Player {
                    backend: playback::create()?,
                    size: [1, 1],
                    initialized: false,
                    error: None,
                    last: Value::Null,
                },
            );
            return Ok(());
        }
        let player = self.players.get_mut(&key).ok_or("Player is not open")?;
        let backend = &mut player.backend;
        match command {
            Command::Load { source } => match source {
                playback::MediaSource::LocalFile(path) => backend.load_local_file(path)?,
                playback::MediaSource::OnlineFile(url)
                | playback::MediaSource::NetworkStream(url) => backend.load_stream_url(url)?,
            },
            Command::Play => backend.play(),
            Command::Pause => backend.pause(),
            Command::Stop => backend.stop(),
            Command::Seek { ms } => backend.seek(ms),
            Command::Volume { value } => backend.set_volume(value),
            Command::Mute { value } => backend.set_mute(value),
            Command::Audio { selection } => backend.select_audio(selection)?,
            Command::RefreshAudio => backend.refresh_audio()?,
            Command::Video { size } => player.size = size.map(|v| v.clamp(1, 8192)),
            Command::Visible { value } => backend.set_video_visible(value),
            Command::Subtitles { text } => backend.set_osd_subtitle(&text),
            Command::Style { style } => backend.set_subtitle_style(&style),
            Command::Channels { channels } => backend.set_channel_routing(&channels),
            Command::AudioOnly { value } => {
                if !value && !player.initialized && backend.supports_video() {
                    let (state, ctx, display) = self
                        .graphics
                        .as_ref()
                        .ok_or("Video rendering is unavailable")?;
                    backend.initialize_video(state, ctx, *display)?;
                    player.initialized = true;
                }
                backend.set_audio_only_mode(value);
            }
            Command::Open | Command::Close => {}
        }
        Ok(())
    }
    pub fn texture(&mut self, plugin: &str, id: u64) -> Option<egui::TextureId> {
        let player = self.players.get_mut(&(plugin.into(), id))?;
        match player.backend.video_frame(player.size) {
            Ok(texture) => texture,
            Err(error) => {
                player.error = Some(error);
                None
            }
        }
    }
    pub fn poll(&mut self) -> Vec<(String, Value)> {
        let mut updates = Vec::new();
        for ((plugin, id), player) in &mut self.players {
            player.backend.tick();
            let b = &player.backend;
            let snapshot = Snapshot {
                time_ms: b.get_time_ms(),
                duration_ms: b.get_duration_ms(),
                status: b.get_status(),
                diagnostics: b.get_diagnostics(),
                tracks: b.audio_tracks().into(),
                channels: b.get_audio_channel_count(),
                layout: b.get_audio_layout(),
                video: b.supports_video() && self.graphics.is_some(),
                error: player.error.take().or_else(|| player.backend.take_error()),
            };
            let value = json!({"id":id,"snapshot":snapshot});
            if value != player.last {
                player.last = value.clone();
                updates.push((plugin.clone(), value));
            }
        }
        updates
    }
}
