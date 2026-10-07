//! Local audio playback without an optional video runtime. Decode off the UI
//! thread, keep at most two seconds queued, and never do I/O in the audio callback.
mod source;
use super::{MediaBackend, PlaybackStatus, PlayerDiagnostics};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::{Sender, bounded};
use parking_lot::Mutex;
use std::{collections::VecDeque, path::PathBuf, sync::Arc, thread::JoinHandle, time::Duration};

#[derive(Default)]
struct State {
    frames: VecDeque<[f32; 2]>,
    generation: u64,
    rate: u32,
    channels: usize,
    duration_ms: i64,
    position: f64,
    phase: f64,
    playing: bool,
    eof: bool,
    volume: f32,
    muted: bool,
    enabled: Vec<bool>,
    error: Option<String>,
}
enum Command {
    Load(source::Source, u64),
    Seek(i64),
    Stop,
    Play,
    Pause,
    Quit,
}
pub struct AudioBackend {
    state: Arc<Mutex<State>>,
    commands: Sender<Command>,
    worker: Option<JoinHandle<()>>,
}
impl AudioBackend {
    pub fn new() -> Result<Self, String> {
        let state = Arc::new(Mutex::new(State {
            volume: 1.0,
            ..Default::default()
        }));
        let (commands, rx) = bounded(16);
        let shared = state.clone();
        let worker = std::thread::Builder::new()
            .name("media-audio-playback".into())
            .spawn(move || {
                let mut source: Option<source::Source> = None;
                let mut output = None;
                let mut generation = 0;
                loop {
                    let active = shared.lock().playing;
                    if !active {
                        output = None;
                    }
                    let command = if active {
                        rx.recv_timeout(Duration::from_millis(5)).ok()
                    } else {
                        rx.recv().ok()
                    };
                    match command {
                        Some(Command::Quit) => break,
                        Some(Command::Load(loaded, loaded_generation)) => {
                            output = None;
                            source = Some(loaded);
                            generation = loaded_generation;
                        }
                        Some(Command::Seek(ms)) => {
                            if {
                                let state = shared.lock();
                                generation == state.generation
                            } && let Some(source) = &mut source
                            {
                                let was_playing = shared.lock().playing;
                                let result = source.seek(ms);
                                let mut state = shared.lock();
                                if generation != state.generation {
                                    continue;
                                }
                                match result {
                                    Ok(()) => {
                                        state.frames.clear();
                                        state.position =
                                            ms.max(0) as f64 * state.rate as f64 / 1000.0;
                                        state.phase = 0.0;
                                        state.eof = false;
                                        state.playing = was_playing;
                                    }
                                    Err(error) => {
                                        state.error = Some(error);
                                        state.playing = false;
                                    }
                                }
                            }
                        }
                        Some(Command::Stop) => {
                            output = None;
                            if let Some(source) = &mut source {
                                let _ = source.seek(0);
                            }
                            let mut state = shared.lock();
                            state.playing = false;
                            state.frames.clear();
                            state.position = 0.0;
                            state.phase = 0.0;
                            state.eof = false;
                        }
                        Some(Command::Play) => {
                            if {
                                let state = shared.lock();
                                generation == state.generation
                            } && let Some(source) = &mut source
                            {
                                let restart = {
                                    let state = shared.lock();
                                    state.eof && state.frames.is_empty()
                                };
                                let result = if restart { source.seek(0) } else { Ok(()) };
                                let mut state = shared.lock();
                                if generation != state.generation {
                                    continue;
                                }
                                match result {
                                    Ok(()) => {
                                        if restart {
                                            state.position = 0.0;
                                            state.phase = 0.0;
                                            state.eof = false;
                                        }
                                        state.playing = true;
                                    }
                                    Err(error) => {
                                        state.error = Some(error);
                                        state.playing = false;
                                    }
                                }
                            }
                        }
                        Some(Command::Pause) => shared.lock().playing = false,
                        None => {}
                    }
                    if {
                        let state = shared.lock();
                        !state.playing || generation != state.generation
                    } {
                        continue;
                    }
                    if output.is_none() {
                        match create_output(shared.clone()) {
                            Ok(stream) => output = Some(stream),
                            Err(error) => {
                                let mut state = shared.lock();
                                state.error = Some(error);
                                state.playing = false;
                                continue;
                            }
                        }
                    }
                    let enabled = {
                        let state = shared.lock();
                        if state.eof || state.frames.len() >= state.rate as usize * 2 {
                            continue;
                        }
                        state.enabled.clone()
                    };
                    if let Some(source) = &mut source {
                        let result = source.read(&enabled);
                        let mut state = shared.lock();
                        if generation != state.generation {
                            continue;
                        }
                        match result {
                            Ok(Some(frames)) => state.frames.extend(frames),
                            Ok(None) => state.eof = true,
                            Err(error) => {
                                state.error = Some(error);
                                state.playing = false;
                            }
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            state,
            commands,
            worker: Some(worker),
        })
    }
    fn command(&self, command: Command) {
        if self.commands.send(command).is_err() {
            self.state.lock().error = Some("Audio playback stopped unexpectedly".into());
        }
    }
}
impl Drop for AudioBackend {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Quit);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl MediaBackend for AudioBackend {
    fn load_local_file(&mut self, path: PathBuf) -> Result<(), String> {
        let source = source::Source::open(&path)?;
        let mut state = self.state.lock();
        state.generation = state.generation.wrapping_add(1);
        let generation = state.generation;
        state.playing = false;
        state.frames.clear();
        state.position = 0.0;
        state.phase = 0.0;
        state.eof = false;
        state.error = None;
        state.rate = source.rate;
        state.channels = source.channels;
        state.duration_ms = source.duration_ms;
        drop(state);
        self.command(Command::Load(source, generation));
        Ok(())
    }
    fn load_stream_url(&mut self, _: String) -> Result<(), String> {
        Err("Playback is unavailable for this file.".into())
    }
    fn play(&mut self) {
        self.command(Command::Play);
    }
    fn pause(&mut self) {
        self.state.lock().playing = false;
        self.command(Command::Pause);
    }
    fn stop(&mut self) {
        self.state.lock().playing = false;
        self.command(Command::Stop);
    }
    fn seek(&mut self, ms: i64) {
        let duration = self.state.lock().duration_ms;
        let position = if duration > 0 {
            ms.clamp(0, duration.saturating_sub(1))
        } else {
            ms.max(0)
        };
        self.command(Command::Seek(position));
    }
    fn set_volume(&mut self, volume: f32) {
        self.state.lock().volume = volume.clamp(0.0, 1.0);
    }
    fn set_mute(&mut self, mute: bool) {
        self.state.lock().muted = mute;
    }
    fn get_time_ms(&self) -> i64 {
        let state = self.state.lock();
        (state.position * 1000.0 / state.rate.max(1) as f64) as i64
    }
    fn get_duration_ms(&self) -> i64 {
        self.state.lock().duration_ms
    }
    fn get_status(&self) -> PlaybackStatus {
        let state = self.state.lock();
        if state.playing {
            PlaybackStatus::Playing
        } else if state.position > 0.0 && !(state.eof && state.frames.is_empty()) {
            PlaybackStatus::Paused
        } else {
            PlaybackStatus::Stopped
        }
    }
    fn get_diagnostics(&self) -> PlayerDiagnostics {
        Default::default()
    }
    fn tick(&mut self) {}
    fn take_error(&mut self) -> Option<String> {
        self.state.lock().error.take()
    }
    fn supports_video(&self) -> bool {
        false
    }
    fn attach_native_host(&mut self, _: *mut std::ffi::c_void) {}
    fn set_osd_subtitle(&mut self, _: &str) {}
    fn get_audio_channel_count(&self) -> Option<usize> {
        let count = self.state.lock().channels;
        (count > 0).then_some(count)
    }
    fn get_audio_layout(&self) -> Option<String> {
        None
    }
    fn set_channel_routing(&mut self, channels: &[super::super::task::AudioChannelItem]) {
        let enabled = channels.iter().map(|channel| channel.playback).collect();
        let changed = {
            let mut state = self.state.lock();
            if state.enabled == enabled {
                false
            } else {
                state.enabled = enabled;
                true
            }
        };
        if changed {
            self.seek(self.get_time_ms());
        }
    }
    fn set_audio_only_mode(&mut self, _: bool) {}
}
fn create_output(state: Arc<Mutex<State>>) -> Result<cpal::Stream, String> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or("No audio output device is available.")?;
    let supported = device.default_output_config().map_err(|e| e.to_string())?;
    macro_rules! stream {
        ($sample:ty) => {
            build_output::<$sample>(&device, supported.config(), state)
        };
    }
    let output = match supported.sample_format() {
        cpal::SampleFormat::F32 => stream!(f32),
        cpal::SampleFormat::F64 => stream!(f64),
        cpal::SampleFormat::I16 => stream!(i16),
        cpal::SampleFormat::I32 => stream!(i32),
        cpal::SampleFormat::U16 => stream!(u16),
        _ => Err("Unsupported audio output format".into()),
    }?;
    output.play().map_err(|e| e.to_string())?;
    Ok(output)
}
fn build_output<T: cpal::SizedSample + cpal::FromSample<f32>>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    state: Arc<Mutex<State>>,
) -> Result<cpal::Stream, String> {
    let error_state = state.clone();
    let channels = config.channels as usize;
    let rate = config.sample_rate as f64;
    device
        .build_output_stream(
            config,
            move |output: &mut [T], _| {
                let Some(mut state) = state.try_lock() else {
                    output.fill(T::from_sample(0.0));
                    return;
                };
                for frame in output.chunks_mut(channels) {
                    let stereo = next_frame(&mut state, rate);
                    for (index, sample) in frame.iter_mut().enumerate() {
                        let value = if channels == 1 {
                            (stereo[0] + stereo[1]) * 0.5
                        } else {
                            stereo[index.min(1)]
                        };
                        *sample = T::from_sample(value);
                    }
                }
            },
            move |error| {
                let mut state = error_state.lock();
                state.error = Some(error.to_string());
                state.playing = false;
            },
            None,
        )
        .map_err(|e| e.to_string())
}
fn next_frame(state: &mut State, output_rate: f64) -> [f32; 2] {
    if !state.playing {
        return [0.0; 2];
    }
    // A short decode underrun must not extrapolate past the next source frame.
    while state.phase >= 1.0 && !state.frames.is_empty() {
        state.frames.pop_front();
        state.position += 1.0;
        state.phase -= 1.0;
    }
    let Some(current) = state.frames.front().copied() else {
        if state.eof {
            state.playing = false;
        }
        return [0.0; 2];
    };
    let next = state.frames.get(1).copied().unwrap_or(current);
    let gain = if state.muted { 0.0 } else { state.volume };
    let frame =
        std::array::from_fn(|i| (current[i] + (next[i] - current[i]) * state.phase as f32) * gain);
    state.phase += state.rate as f64 / output_rate;
    while state.phase >= 1.0 && !state.frames.is_empty() {
        state.frames.pop_front();
        state.position += 1.0;
        state.phase -= 1.0;
    }
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_preserves_pause_volume_mute_and_eof() {
        let mut state = State {
            rate: 48_000,
            playing: true,
            volume: 0.5,
            eof: true,
            frames: VecDeque::from([[0.8, -0.4], [0.4, -0.2]]),
            ..Default::default()
        };
        assert_eq!(next_frame(&mut state, 48_000.0), [0.4, -0.2]);
        state.playing = false;
        assert_eq!(next_frame(&mut state, 48_000.0), [0.0; 2]);
        assert_eq!(state.position, 1.0);
        state.playing = true;
        state.muted = true;
        assert_eq!(next_frame(&mut state, 48_000.0), [0.0; 2]);
        assert_eq!(state.position, 2.0);
        next_frame(&mut state, 48_000.0);
        assert!(!state.playing);
    }

    #[test]
    fn sample_rate_conversion_survives_a_decode_underrun() {
        let mut state = State {
            rate: 96_000,
            playing: true,
            volume: 1.0,
            frames: VecDeque::from([[0.2; 2]]),
            ..Default::default()
        };
        assert_eq!(next_frame(&mut state, 48_000.0), [0.2; 2]);
        assert_eq!(next_frame(&mut state, 48_000.0), [0.0; 2]);
        state.frames.extend([[0.4; 2], [0.6; 2], [0.8; 2]]);
        assert_eq!(next_frame(&mut state, 48_000.0), [0.6; 2]);
        assert_eq!(state.position, 4.0);
    }
}
