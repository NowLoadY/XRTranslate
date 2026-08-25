use audioadapter_buffers::direct::InterleavedSlice;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Sample, Stream};
use crossbeam_channel::{Receiver, Sender, TrySendError, bounded};
use parking_lot::Mutex;
use rubato::{Fft, FixedSync, Indexing, Resampler};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt;
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering},
};
use std::thread;
use std::time::Duration;

#[cfg(windows)]
use wasapi::{
    AudioClient, DeviceEnumerator, Direction, SampleType, SessionState, StreamMode, WaveFormat,
    deinitialize, initialize_mta,
};

pub struct InputDevice {
    /// Stable endpoint ID. Do not use the display name as an identifier.
    pub id: String,
    pub name: String,
}

/// One application that currently owns a Windows render-audio session.
/// `id` is derived from the executable path so a saved selection survives a
/// process restart; `process_id` is refreshed before route activation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioApplication {
    pub id: String,
    pub name: String,
    pub process_id: u32,
    pub active: bool,
}

#[derive(Clone, Debug)]
pub struct InputConfigInfo {
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_format: String,
}

pub const AUDIO_ROUTE_SAMPLE_RATE: u32 = 48_000;
const DEFAULT_ROUTE_QUEUE_MS: u32 = 250;
const MICROPHONE_SUBSCRIBER_QUEUE_CAPACITY: usize = 64;

/// One capture node in a host-owned audio route. An empty device ID selects
/// the current host default; endpoint names are deliberately not identifiers.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioRouteSourceConfig {
    pub device_id: String,
    pub gain: f32,
}

/// Selects whether a system-audio source captures an entire render endpoint or
/// one application's process tree. Application capture is endpoint-independent
/// on supported Windows builds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioRouteLoopbackTarget {
    Endpoint {
        device_id: String,
    },
    Application {
        process_id: u32,
        application_name: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct AudioRouteLoopbackConfig {
    pub target: AudioRouteLoopbackTarget,
    pub gain: f32,
}

impl Default for AudioRouteSourceConfig {
    fn default() -> Self {
        Self {
            device_id: String::new(),
            gain: 1.0,
        }
    }
}

/// Neutral host composition for a low-latency route. `tts_gain: None` means
/// that synthesized speech is not connected to this route.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioRouteConfig {
    pub microphone: Option<AudioRouteSourceConfig>,
    pub system_loopback: Option<AudioRouteLoopbackConfig>,
    pub tts_gain: Option<f32>,
    pub output_device_id: String,
    /// Symmetric linear peak ceiling applied after mixing.
    pub output_ceiling: f32,
    pub queue_capacity_ms: u32,
}

impl Default for AudioRouteConfig {
    fn default() -> Self {
        Self {
            microphone: None,
            system_loopback: None,
            tts_gain: Some(1.0),
            output_device_id: String::new(),
            output_ceiling: 1.0,
            queue_capacity_ms: DEFAULT_ROUTE_QUEUE_MS,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioRouteState {
    Starting,
    Running,
    Stopping,
    Stopped,
    Faulted,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AudioRouteStatus {
    pub state: AudioRouteState,
    pub last_error: Option<String>,
    pub dropped_samples: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AudioRouteLevels {
    pub microphone: f32,
    pub system_loopback: f32,
    pub tts: f32,
    pub output: f32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AudioRouteError {
    InvalidConfiguration(String),
    #[cfg(not(windows))]
    UnsupportedCapability(String),
    DeviceUnavailable(String),
    StreamStart(String),
}

impl fmt::Display for AudioRouteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (kind, detail) = match self {
            Self::InvalidConfiguration(detail) => ("invalid audio route", detail),
            #[cfg(not(windows))]
            Self::UnsupportedCapability(detail) => ("unsupported audio capability", detail),
            Self::DeviceUnavailable(detail) => ("audio device unavailable", detail),
            Self::StreamStart(detail) => ("could not start audio route", detail),
        };
        write!(formatter, "{kind}: {detail}")
    }
}

impl std::error::Error for AudioRouteError {}

struct AudioRouteSourceBuffer {
    queue: Arc<Mutex<VecDeque<f32>>>,
    capacity: usize,
    gain: AtomicU32,
    level: Arc<AtomicU32>,
    playback_tail_samples: Arc<AtomicU64>,
    dropped_samples: Arc<AtomicU64>,
}

impl AudioRouteSourceBuffer {
    fn new(capacity: usize, gain: f32, dropped_samples: Arc<AtomicU64>) -> Self {
        Self {
            queue: Arc::new(Mutex::new(VecDeque::with_capacity(capacity))),
            capacity,
            gain: AtomicU32::new(gain.to_bits()),
            level: Arc::new(AtomicU32::new(0.0f32.to_bits())),
            playback_tail_samples: Arc::new(AtomicU64::new(0)),
            dropped_samples,
        }
    }

    fn push(&self, mut samples: Vec<f32>) {
        for sample in &mut samples {
            *sample = sample.clamp(-1.0, 1.0);
        }
        let input_excess = samples.len().saturating_sub(self.capacity);
        if input_excess != 0 {
            samples.drain(..input_excess);
        }
        let mut queue = self.queue.lock();
        let queued_excess = queue
            .len()
            .saturating_add(samples.len())
            .saturating_sub(self.capacity);
        if queued_excess != 0 {
            queue.drain(..queued_excess);
        }
        queue.extend(samples);
        self.dropped_samples
            .fetch_add((input_excess + queued_excess) as u64, Ordering::Relaxed);
    }

    fn gain(&self) -> f32 {
        f32::from_bits(self.gain.load(Ordering::Relaxed))
    }

    fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }
}

struct AudioRouteControl {
    state: AtomicU8,
    last_error: Mutex<Option<String>>,
    dropped_samples: Arc<AtomicU64>,
    output_level: Arc<AtomicU32>,
    output_sample_rate: u32,
    microphone: Option<Arc<AudioRouteSourceBuffer>>,
    system_loopback: Option<Arc<AudioRouteSourceBuffer>>,
    tts: Option<Arc<AudioRouteSourceBuffer>>,
    routed_tts_targets: Arc<Mutex<Vec<RoutedTtsTarget>>>,
    resources: Mutex<Option<AudioRouteResources>>,
}

#[derive(Clone)]
struct RoutedTtsTarget {
    control: Weak<AudioRouteControl>,
    source: Arc<AudioRouteSourceBuffer>,
    output_sample_rate: u32,
}

impl RoutedTtsTarget {
    fn is_running(&self) -> bool {
        self.control.upgrade().is_some_and(|control| {
            decode_route_state(control.state.load(Ordering::Acquire)) == AudioRouteState::Running
        })
    }

    fn enqueue_samples(&self, samples: &[f32]) {
        update_input_level(samples, &self.source.level);
        self.source.push(samples.to_vec());
        let queued_samples = self.source.queue.lock().len();
        self.source.playback_tail_samples.store(
            queued_samples as u64 * u64::from(self.output_sample_rate)
                / u64::from(AUDIO_ROUTE_SAMPLE_RATE)
                + u64::from(self.output_sample_rate) * 150 / 1_000,
            Ordering::Release,
        );
    }
}

/// Cloneable control/data handle for a route. It owns no plugin policy and can
/// safely be retained by a controller after the translation session stops.
#[derive(Clone)]
pub struct AudioRouteHandle {
    control: Arc<AudioRouteControl>,
}

impl AudioRouteHandle {
    pub fn status(&self) -> AudioRouteStatus {
        AudioRouteStatus {
            state: decode_route_state(self.control.state.load(Ordering::Acquire)),
            last_error: self.control.last_error.lock().clone(),
            dropped_samples: self.control.dropped_samples.load(Ordering::Relaxed),
        }
    }

    pub fn levels(&self) -> AudioRouteLevels {
        AudioRouteLevels {
            microphone: route_source_level(&self.control.microphone),
            system_loopback: route_source_level(&self.control.system_loopback),
            tts: self.control.tts.as_ref().map_or(0.0, |source| {
                (source.playback_tail_samples.load(Ordering::Acquire) != 0)
                    .then(|| source.level())
                    .unwrap_or(0.0)
            }),
            output: f32::from_bits(self.control.output_level.load(Ordering::Relaxed)),
        }
    }

    /// Requests a clean stop independently of translation capture/playback.
    pub fn stop(&self) {
        self.control.routed_tts_targets.lock().retain(|target| {
            !target
                .control
                .upgrade()
                .is_some_and(|control| Arc::ptr_eq(&control, &self.control))
        });
        let resources = self.control.resources.lock().take();
        let Some(resources) = resources else {
            return;
        };
        for source in [
            &self.control.microphone,
            &self.control.system_loopback,
            &self.control.tts,
        ]
        .into_iter()
        .flatten()
        {
            source.queue.lock().clear();
            source.level.store(0.0f32.to_bits(), Ordering::Relaxed);
            source.playback_tail_samples.store(0, Ordering::Release);
        }
        self.control
            .output_level
            .store(0.0f32.to_bits(), Ordering::Relaxed);
        self.control.state.store(
            encode_route_state(AudioRouteState::Stopping),
            Ordering::Release,
        );
        let control = Arc::clone(&self.control);
        let spawn = thread::Builder::new()
            .name("audio-route-reaper".into())
            .spawn(move || {
                resources.stop();
                control.state.store(
                    encode_route_state(AudioRouteState::Stopped),
                    Ordering::Release,
                );
            });
        if let Err(error) = spawn {
            self.control.state.store(
                encode_route_state(AudioRouteState::Faulted),
                Ordering::Release,
            );
            *self.control.last_error.lock() = Some(format!("cannot stop audio route: {error}"));
        }
    }
}

pub struct AudioSystem {
    host: cpal::Host,
    active_captures: Vec<ActiveCapture>,
    tts_player: Option<TtsPlayer>,
    audio_routes: Vec<AudioRouteHandle>,
    routed_tts_targets: Arc<Mutex<Vec<RoutedTtsTarget>>>,
    microphone_fanout: Option<(String, Arc<MicrophoneFanout>)>,
}

#[derive(Clone)]
pub struct TtsPlayerHandle {
    queue: Arc<Mutex<VecDeque<f32>>>,
    sample_rate: u32,
    source_sample_rate: u32,
    max_queued_samples: Option<usize>,
    dropped_samples: Option<Arc<AtomicU64>>,
    playback_tail_samples: Arc<AtomicU64>,
    playback_clock_rate: u32,
    level: Option<Arc<AudioRouteSourceBuffer>>,
    routed_tts_targets: Arc<Mutex<Vec<RoutedTtsTarget>>>,
    legacy_available: bool,
}

impl TtsPlayerHandle {
    pub fn play_pcm(&self, pcm: &[u8]) -> Result<(), String> {
        if pcm.len() < 2 {
            return Ok(());
        }
        if self.source_sample_rate == 0 {
            return Err("TTS source sample rate must be greater than zero".into());
        }
        let routed_targets = self
            .routed_tts_targets
            .lock()
            .iter()
            .filter(|target| target.is_running())
            .cloned()
            .collect::<Vec<_>>();
        if !routed_targets.is_empty() {
            let samples = resample_mono(
                pcm16_mono_samples(pcm),
                self.source_sample_rate,
                AUDIO_ROUTE_SAMPLE_RATE,
            )?;
            for target in &routed_targets {
                target.enqueue_samples(&samples);
            }
            log::info!(
                "Queued TTS audio for {} routed outputs: input_bytes={}, output_samples={}, source_rate={}, route_rate={}",
                routed_targets.len(),
                pcm.len(),
                samples.len(),
                self.source_sample_rate,
                AUDIO_ROUTE_SAMPLE_RATE,
            );
            return Ok(());
        }
        if !self.legacy_available {
            return Err("the audio route used by this TTS handle is no longer running".into());
        }
        let samples = pcm16_mono_samples(pcm);
        let samples = resample_mono(samples, self.source_sample_rate, self.sample_rate)?;
        if let Some(level) = &self.level {
            update_input_level(&samples, &level.level);
        }
        let output_samples = samples.len();
        let mut queue = self.queue.lock();
        queue.extend(samples);
        if let Some(capacity) = self.max_queued_samples {
            let excess = queue.len().saturating_sub(capacity);
            queue.drain(..excess);
            if let Some(dropped) = &self.dropped_samples {
                dropped.fetch_add(excess as u64, Ordering::Relaxed);
            }
        }
        let queued_samples = queue.len();
        // Keep ASR suppression active for a short device-buffer tail after the
        // last queued sample has been rendered. This avoids reopening a
        // loopback recognizer while WASAPI still has synthesized speech in
        // flight.
        self.playback_tail_samples.store(
            queued_samples as u64 * u64::from(self.playback_clock_rate)
                / u64::from(self.sample_rate)
                + u64::from(self.playback_clock_rate) * 150 / 1_000,
            Ordering::Release,
        );
        drop(queue);
        log::info!(
            "Queued TTS audio for playback: input_bytes={}, output_samples={}, queued_samples={}, source_rate={}, output_rate={}",
            pcm.len(),
            output_samples,
            queued_samples,
            self.source_sample_rate,
            self.sample_rate
        );
        Ok(())
    }

    pub fn is_playing(&self) -> bool {
        let routed_targets = self
            .routed_tts_targets
            .lock()
            .iter()
            .filter(|target| target.is_running())
            .cloned()
            .collect::<Vec<_>>();
        if !routed_targets.is_empty() {
            return routed_targets.iter().any(|target| {
                !target.source.queue.lock().is_empty()
                    || target.source.playback_tail_samples.load(Ordering::Acquire) != 0
            }) || self.playback_tail_samples.load(Ordering::Acquire) != 0;
        }
        if !self.legacy_available {
            return false;
        }
        !self.queue.lock().is_empty() || self.playback_tail_samples.load(Ordering::Acquire) != 0
    }
}

struct TtsPlayer {
    queue: Arc<Mutex<VecDeque<f32>>>,
    sample_rate: u32,
    device_id: String,
    playback_tail_samples: Arc<AtomicU64>,
    _stream: Stream,
}

enum ActiveCapture {
    MicrophoneSubscription(thread::JoinHandle<()>),
    #[cfg(windows)]
    Loopback(LoopbackCapture),
}

/// A single physical microphone stream shared by multiple rendered routes.
/// Opening the same input endpoint more than once is rejected by some audio
/// backends, so route-local workers subscribe to this fanout instead.
struct MicrophoneFanout {
    senders: Arc<Mutex<Vec<Sender<Vec<f32>>>>>,
    sample_rate: u32,
    _stream: Stream,
}

impl MicrophoneFanout {
    fn attach(&self) -> Receiver<Vec<f32>> {
        // Both mode can have a render-route subscriber and an ASR subscriber
        // on the same physical stream. Keep enough buffered audio for a brief
        // scheduling stall so one subscriber does not immediately lose a turn.
        let (tx, rx) = bounded::<Vec<f32>>(MICROPHONE_SUBSCRIBER_QUEUE_CAPACITY);
        self.senders.lock().push(tx);
        rx
    }
}

struct AudioRouteResources {
    output: Stream,
    inputs: Vec<Stream>,
    source_workers: Vec<thread::JoinHandle<()>>,
    shared_microphones: Vec<Arc<MicrophoneFanout>>,
    #[cfg(windows)]
    loopback: Option<LoopbackCapture>,
}

impl AudioRouteResources {
    fn stop(self) {
        let Self {
            output,
            inputs,
            source_workers,
            shared_microphones,
            #[cfg(windows)]
            loopback,
        } = self;
        drop(output);
        drop(inputs);
        #[cfg(windows)]
        if let Some(loopback) = loopback {
            loopback.stop();
        }
        for worker in source_workers {
            let _ = worker.join();
        }
        drop(shared_microphones);
    }
}

impl ActiveCapture {
    fn stop(self) {
        match self {
            Self::MicrophoneSubscription(worker) => {
                let _ = thread::Builder::new()
                    .name("audio-subscription-reaper".into())
                    .spawn(move || {
                        let _ = worker.join();
                    });
            }
            #[cfg(windows)]
            Self::Loopback(capture) => capture.stop(),
        }
    }
}

#[cfg(windows)]
struct LoopbackCapture {
    stop_requested: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

#[cfg(windows)]
impl LoopbackCapture {
    fn stop(mut self) {
        self.stop_requested.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            reap_worker(worker);
        }
    }
}

#[cfg(windows)]
impl Drop for LoopbackCapture {
    fn drop(&mut self) {
        self.stop_requested.store(true, Ordering::Release);
    }
}

#[cfg(windows)]
fn start_route_loopback_capture(
    target: &AudioRouteLoopbackTarget,
    output_tx: Sender<Vec<f32>>,
    source: Arc<AudioRouteSourceBuffer>,
    control: Arc<AudioRouteControl>,
) -> Result<LoopbackCapture, AudioRouteError> {
    let target = target.clone();
    let stop_requested = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop_requested);
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let worker = thread::Builder::new()
        .name("audio-route-loopback".into())
        .spawn(move || {
            if let Err(error) = run_loopback_capture(
                &target,
                Some(output_tx),
                Arc::clone(&source.level),
                worker_stop,
                &ready_tx,
                AUDIO_ROUTE_SAMPLE_RATE,
                Some(Arc::clone(&source.dropped_samples)),
            ) {
                let _ = ready_tx.send(Err(error.clone()));
                log::error!("Audio route WASAPI loopback stopped: {error}");
                *control.last_error.lock() = Some(error);
                control.state.store(
                    encode_route_state(AudioRouteState::Faulted),
                    Ordering::Release,
                );
            }
        })
        .map_err(|error| {
            AudioRouteError::StreamStart(format!("cannot start system-loopback worker: {error}"))
        })?;
    match ready_rx.recv_timeout(Duration::from_secs(2)) {
        Ok(Ok(())) => Ok(LoopbackCapture {
            stop_requested,
            worker: Some(worker),
        }),
        Ok(Err(error)) => {
            stop_requested.store(true, Ordering::Release);
            reap_worker(worker);
            Err(AudioRouteError::StreamStart(error))
        }
        Err(_) => {
            stop_requested.store(true, Ordering::Release);
            reap_worker(worker);
            Err(AudioRouteError::StreamStart(
                "timed out while opening the selected system-loopback endpoint".into(),
            ))
        }
    }
}

#[cfg(windows)]
fn reap_worker(worker: thread::JoinHandle<()>) {
    let _ = thread::Builder::new()
        .name("wasapi-worker-reaper".into())
        .spawn(move || {
            let _ = worker.join();
        });
}

impl AudioSystem {
    /// Returns the latest lock-free RMS envelopes for the currently installed
    /// real-time routes. The audio callbacks already maintain these meters, so
    /// graph visualizations never need to inspect or copy PCM samples.
    pub fn active_audio_route_levels(&self) -> Vec<AudioRouteLevels> {
        self.audio_routes
            .iter()
            .map(AudioRouteHandle::levels)
            .collect()
    }

    pub fn new() -> Self {
        Self {
            host: cpal::default_host(),
            active_captures: Vec::new(),
            tts_player: None,
            audio_routes: Vec::new(),
            routed_tts_targets: Arc::new(Mutex::new(Vec::new())),
            microphone_fanout: None,
        }
    }

    /// List all available input devices
    pub fn available_devices(&self) -> Vec<InputDevice> {
        let mut devices = Vec::new();
        if let Ok(input_devices) = self.host.input_devices() {
            for device in input_devices {
                match (device.id(), device.description()) {
                    (Ok(id), Ok(description)) => devices.push(InputDevice {
                        id: id.to_string(),
                        name: description.name().to_owned(),
                    }),
                    (Err(error), _) | (_, Err(error)) => {
                        log::warn!("Skipping input device that cannot be described: {error}");
                    }
                }
            }
        }
        devices
    }

    /// Lists render endpoints. Virtual microphone cables appear here as an
    /// output endpoint and can therefore receive game-bound synthesized audio.
    pub fn available_output_devices(&self) -> Vec<InputDevice> {
        self.host
            .output_devices()
            .map(|devices| {
                devices
                    .filter_map(|device| match (device.id(), device.description()) {
                        (Ok(id), Ok(description)) => Some(InputDevice {
                            id: id.to_string(),
                            name: description.name().to_owned(),
                        }),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Lists applications that currently own at least one render-audio
    /// session. The stable executable identity is kept separate from the live
    /// PID so Audio Studio can rebind a saved selection after an app restarts.
    #[cfg(windows)]
    pub fn available_audio_applications(&self) -> Vec<AudioApplication> {
        match self.try_available_audio_applications() {
            Ok(applications) => applications,
            Err(error) => {
                log::warn!("Could not enumerate application audio sessions: {error}");
                Vec::new()
            }
        }
    }

    #[cfg(not(windows))]
    pub fn available_audio_applications(&self) -> Vec<AudioApplication> {
        Vec::new()
    }

    /// Fallible form used by demand-driven discovery so a transient failure can
    /// preserve the last-good application list instead of looking like an
    /// authoritative empty result.
    #[cfg(windows)]
    pub fn try_available_audio_applications(&self) -> Result<Vec<AudioApplication>, String> {
        enumerate_audio_applications()
    }

    #[cfg(not(windows))]
    pub fn try_available_audio_applications(&self) -> Result<Vec<AudioApplication>, String> {
        Ok(Vec::new())
    }

    /// Read the current default capture format for the selected device.
    pub fn input_config(&self, device_id: &str) -> Result<InputConfigInfo, String> {
        let device = if device_id.is_empty() {
            self.host
                .default_input_device()
                .ok_or("No default input device available.")?
        } else {
            let parsed_id = device_id
                .parse()
                .map_err(|error| format!("Invalid microphone ID '{device_id}': {error}"))?;
            self.host
                .device_by_id(&parsed_id)
                .ok_or_else(|| format!("Microphone '{device_id}' is no longer available"))?
        };
        let config = device
            .default_input_config()
            .map_err(|error| format!("Failed to read microphone format: {error}"))?;
        Ok(InputConfigInfo {
            sample_rate: config.sample_rate(),
            channels: config.channels(),
            sample_format: config.sample_format().to_string(),
        })
    }

    /// Stop the currently active audio stream
    pub fn stop(&mut self) {
        for capture in self.active_captures.drain(..) {
            capture.stop();
        }
        self.clear_tts_playback();
        if self.audio_routes.is_empty() {
            self.microphone_fanout = None;
        }
    }

    pub fn clear_tts_playback(&mut self) {
        if let Some(player) = &self.tts_player {
            player.queue.lock().clear();
            player.playback_tail_samples.store(0, Ordering::Release);
        }
        let routed_targets = self.routed_tts_targets.lock().clone();
        for target in routed_targets {
            target.source.queue.lock().clear();
            target
                .source
                .level
                .store(0.0f32.to_bits(), Ordering::Relaxed);
            target
                .source
                .playback_tail_samples
                .store(0, Ordering::Release);
        }
    }

    /// Get a handle to the TTS player that can be safely sent to other threads.
    pub fn tts_handle(
        &mut self,
        source_sample_rate: u32,
        device_id: &str,
    ) -> Result<TtsPlayerHandle, String> {
        let routed_target = self
            .routed_tts_targets
            .lock()
            .iter()
            .find(|target| target.is_running())
            .cloned();
        if let Some(target) = routed_target {
            return Ok(TtsPlayerHandle {
                queue: Arc::clone(&target.source.queue),
                sample_rate: AUDIO_ROUTE_SAMPLE_RATE,
                source_sample_rate,
                max_queued_samples: Some(target.source.capacity),
                dropped_samples: Some(Arc::clone(&target.source.dropped_samples)),
                playback_tail_samples: Arc::clone(&target.source.playback_tail_samples),
                playback_clock_rate: target.output_sample_rate,
                level: Some(Arc::clone(&target.source)),
                routed_tts_targets: Arc::clone(&self.routed_tts_targets),
                legacy_available: false,
            });
        }
        self.ensure_tts_player(device_id)?;
        self.tts_player
            .as_ref()
            .map(|p| TtsPlayerHandle {
                queue: Arc::clone(&p.queue),
                sample_rate: p.sample_rate,
                source_sample_rate,
                max_queued_samples: None,
                dropped_samples: None,
                playback_tail_samples: Arc::clone(&p.playback_tail_samples),
                playback_clock_rate: p.sample_rate,
                level: None,
                routed_tts_targets: Arc::clone(&self.routed_tts_targets),
                legacy_available: true,
            })
            .ok_or_else(|| "TTS output stream was not initialized".into())
    }

    /// Build a complete set of replacement routes before swapping it into the
    /// host. If any route fails to build, every newly built route is stopped
    /// and the previous set remains active.
    pub fn replace_audio_routes(
        &mut self,
        configs: Vec<AudioRouteConfig>,
    ) -> Result<Vec<AudioRouteHandle>, AudioRouteError> {
        let mut microphone_counts = HashMap::<String, usize>::new();
        for config in &configs {
            if let Some(source) = &config.microphone {
                *microphone_counts
                    .entry(source.device_id.clone())
                    .or_default() += 1;
            }
        }
        let mut microphone_fanouts = HashMap::<String, Arc<MicrophoneFanout>>::new();
        for (device_id, count) in microphone_counts {
            if let Some((active_id, fanout)) = &self.microphone_fanout
                && active_id == &device_id
            {
                microphone_fanouts.insert(device_id, Arc::clone(fanout));
            } else if count > 1 {
                let fanout = Arc::new(self.build_microphone_fanout(&device_id)?);
                microphone_fanouts.insert(device_id, fanout);
            }
        }
        let mut replacements = Vec::with_capacity(configs.len());
        for config in configs {
            let fanout = config
                .microphone
                .as_ref()
                .and_then(|source| microphone_fanouts.get(&source.device_id).cloned());
            match self.build_audio_route(config, fanout) {
                Ok(handle) => replacements.push(handle),
                Err(error) => {
                    for handle in replacements {
                        handle.stop();
                    }
                    return Err(error);
                }
            }
        }

        let replacement_targets = replacements
            .iter()
            .filter_map(|handle| {
                handle.control.tts.as_ref().map(|source| RoutedTtsTarget {
                    control: Arc::downgrade(&handle.control),
                    source: Arc::clone(source),
                    output_sample_rate: handle.control.output_sample_rate,
                })
            })
            .collect::<Vec<_>>();
        if let Some((device_id, fanout)) = microphone_fanouts.into_iter().next() {
            self.microphone_fanout = Some((device_id, fanout));
        }
        if !replacement_targets.is_empty() {
            // Stop legacy queueing without dropping its render-tail guard: a
            // shared-mode output may still reach loopback briefly after the
            // route switch.
            if let Some(player) = &self.tts_player {
                player.queue.lock().clear();
                player.playback_tail_samples.fetch_max(
                    u64::from(player.sample_rate) * 150 / 1_000,
                    Ordering::Release,
                );
            }
        }

        *self.routed_tts_targets.lock() = replacement_targets;
        let previous = std::mem::replace(&mut self.audio_routes, replacements.clone());
        for handle in previous {
            handle.stop();
        }
        if replacements.is_empty() && self.active_captures.is_empty() {
            self.microphone_fanout = None;
        }
        for (index, handle) in replacements.iter().enumerate() {
            let status = handle.status();
            let levels = handle.levels();
            log::info!(
                "Audio route {index} started: state={:?}, last_error={:?}, dropped_samples={}, microphone_level={:.3}, loopback_level={:.3}, tts_level={:.3}, output_level={:.3}",
                status.state,
                status.last_error,
                status.dropped_samples,
                levels.microphone,
                levels.system_loopback,
                levels.tts,
                levels.output,
            );
        }
        Ok(replacements)
    }

    fn build_audio_route(
        &self,
        config: AudioRouteConfig,
        microphone_fanout: Option<Arc<MicrophoneFanout>>,
    ) -> Result<AudioRouteHandle, AudioRouteError> {
        validate_audio_route_config(&config)?;

        #[cfg(not(windows))]
        if config.system_loopback.is_some() {
            return Err(AudioRouteError::UnsupportedCapability(
                "system loopback routing currently requires the Windows WASAPI host; select a microphone/TTS-only route or install a platform backend"
                    .into(),
            ));
        }

        let output_device = self.resolve_route_output(&config.output_device_id)?;
        let output_config = output_device.default_output_config().map_err(|error| {
            AudioRouteError::DeviceUnavailable(format!(
                "cannot read the selected output format: {error}"
            ))
        })?;
        let output_rate = output_config.sample_rate();
        let output_channels = output_config.channels() as usize;
        let output_format = output_config.sample_format();
        let output_stream_config: cpal::StreamConfig = output_config.into();
        let capacity = (u64::from(AUDIO_ROUTE_SAMPLE_RATE) * u64::from(config.queue_capacity_ms)
            / 1_000) as usize;
        let dropped_samples = Arc::new(AtomicU64::new(0));
        let microphone = config.microphone.as_ref().map(|source| {
            Arc::new(AudioRouteSourceBuffer::new(
                capacity,
                source.gain,
                Arc::clone(&dropped_samples),
            ))
        });
        let system_loopback = config.system_loopback.as_ref().map(|source| {
            Arc::new(AudioRouteSourceBuffer::new(
                capacity,
                source.gain,
                Arc::clone(&dropped_samples),
            ))
        });
        let tts = config.tts_gain.map(|gain| {
            Arc::new(AudioRouteSourceBuffer::new(
                capacity,
                gain,
                Arc::clone(&dropped_samples),
            ))
        });
        let output_level = Arc::new(AtomicU32::new(0.0f32.to_bits()));
        let control = Arc::new(AudioRouteControl {
            state: AtomicU8::new(encode_route_state(AudioRouteState::Starting)),
            last_error: Mutex::new(None),
            dropped_samples,
            output_level: Arc::clone(&output_level),
            output_sample_rate: output_rate,
            microphone: microphone.clone(),
            system_loopback: system_loopback.clone(),
            tts: tts.clone(),
            routed_tts_targets: Arc::clone(&self.routed_tts_targets),
            resources: Mutex::new(None),
        });

        macro_rules! build_output {
            ($sample:ty) => {
                build_route_output_stream::<$sample>(
                    &output_device,
                    output_stream_config,
                    output_channels,
                    output_rate,
                    microphone,
                    system_loopback,
                    tts,
                    config.output_ceiling,
                    output_level,
                    Arc::clone(&control),
                )
            };
        }
        let output = match output_format {
            cpal::SampleFormat::F32 => build_output!(f32),
            cpal::SampleFormat::F64 => build_output!(f64),
            cpal::SampleFormat::I8 => build_output!(i8),
            cpal::SampleFormat::I16 => build_output!(i16),
            cpal::SampleFormat::I24 => build_output!(cpal::I24),
            cpal::SampleFormat::I32 => build_output!(i32),
            cpal::SampleFormat::I64 => build_output!(i64),
            cpal::SampleFormat::U8 => build_output!(u8),
            cpal::SampleFormat::U16 => build_output!(u16),
            cpal::SampleFormat::U24 => build_output!(cpal::U24),
            cpal::SampleFormat::U32 => build_output!(u32),
            cpal::SampleFormat::U64 => build_output!(u64),
            format => Err(AudioRouteError::StreamStart(format!(
                "unsupported output sample format: {format}"
            ))),
        }?;

        let mut inputs = Vec::new();
        let mut source_workers = Vec::new();
        let mut shared_microphones = Vec::new();
        if let (Some(source), Some(buffer)) = (&config.microphone, &control.microphone) {
            let (stream, worker) = self.build_route_microphone(
                source,
                Arc::clone(buffer),
                Arc::clone(&control),
                microphone_fanout.as_ref(),
            )?;
            if let Some(stream) = stream {
                inputs.push(stream);
            }
            if let Some(fanout) = microphone_fanout {
                shared_microphones.push(fanout);
            }
            source_workers.push(worker);
        }

        #[cfg(windows)]
        let loopback = if let (Some(source), Some(buffer)) =
            (&config.system_loopback, &control.system_loopback)
        {
            let (raw_tx, raw_rx) = bounded::<Vec<f32>>(16);
            source_workers.push(spawn_route_source_worker(
                raw_rx,
                AUDIO_ROUTE_SAMPLE_RATE,
                Arc::clone(buffer),
            )?);
            Some(start_route_loopback_capture(
                &source.target,
                raw_tx,
                Arc::clone(buffer),
                Arc::clone(&control),
            )?)
        } else {
            None
        };

        if let Err(error) = output.play() {
            #[cfg(windows)]
            if let Some(loopback) = loopback {
                loopback.stop();
            }
            drop(inputs);
            return Err(AudioRouteError::StreamStart(format!(
                "cannot start output node: {error}"
            )));
        }
        *control.resources.lock() = Some(AudioRouteResources {
            output,
            inputs,
            source_workers,
            shared_microphones,
            #[cfg(windows)]
            loopback,
        });
        control.state.store(
            encode_route_state(AudioRouteState::Running),
            Ordering::Release,
        );
        let handle = AudioRouteHandle { control };
        Ok(handle)
    }

    fn resolve_route_output(&self, device_id: &str) -> Result<cpal::Device, AudioRouteError> {
        if device_id.is_empty() {
            self.host.default_output_device().ok_or_else(|| {
                AudioRouteError::DeviceUnavailable(
                    "no default render output is available; connect an output or virtual cable"
                        .into(),
                )
            })
        } else {
            let parsed_id = device_id.parse().map_err(|error| {
                AudioRouteError::DeviceUnavailable(format!(
                    "invalid render output ID '{device_id}': {error}"
                ))
            })?;
            self.host.device_by_id(&parsed_id).ok_or_else(|| {
                AudioRouteError::DeviceUnavailable(format!(
                    "render output '{device_id}' is no longer available"
                ))
            })
        }
    }

    fn build_microphone_fanout(
        &self,
        device_id: &str,
    ) -> Result<MicrophoneFanout, AudioRouteError> {
        let device = if device_id.is_empty() {
            self.host.default_input_device().ok_or_else(|| {
                AudioRouteError::DeviceUnavailable(
                    "no default microphone is available for the route".into(),
                )
            })?
        } else {
            let parsed_id = device_id.parse().map_err(|error| {
                AudioRouteError::DeviceUnavailable(format!(
                    "invalid microphone ID '{device_id}': {error}"
                ))
            })?;
            self.host.device_by_id(&parsed_id).ok_or_else(|| {
                AudioRouteError::DeviceUnavailable(format!(
                    "microphone '{device_id}' is no longer available"
                ))
            })?
        };
        let config = device.default_input_config().map_err(|error| {
            AudioRouteError::DeviceUnavailable(format!(
                "cannot read the selected microphone format: {error}"
            ))
        })?;
        let channels = config.channels() as usize;
        let sample_rate = config.sample_rate();
        let sample_format = config.sample_format();
        let stream_config: cpal::StreamConfig = config.into();
        let senders = Arc::new(Mutex::new(Vec::<Sender<Vec<f32>>>::new()));
        let stream = match sample_format {
            cpal::SampleFormat::F32 => build_microphone_fanout_stream::<f32>(
                &device,
                stream_config,
                channels,
                Arc::clone(&senders),
            ),
            cpal::SampleFormat::F64 => build_microphone_fanout_stream::<f64>(
                &device,
                stream_config,
                channels,
                Arc::clone(&senders),
            ),
            cpal::SampleFormat::I8 => build_microphone_fanout_stream::<i8>(
                &device,
                stream_config,
                channels,
                Arc::clone(&senders),
            ),
            cpal::SampleFormat::I16 => build_microphone_fanout_stream::<i16>(
                &device,
                stream_config,
                channels,
                Arc::clone(&senders),
            ),
            cpal::SampleFormat::I24 => build_microphone_fanout_stream::<cpal::I24>(
                &device,
                stream_config,
                channels,
                Arc::clone(&senders),
            ),
            cpal::SampleFormat::I32 => build_microphone_fanout_stream::<i32>(
                &device,
                stream_config,
                channels,
                Arc::clone(&senders),
            ),
            cpal::SampleFormat::I64 => build_microphone_fanout_stream::<i64>(
                &device,
                stream_config,
                channels,
                Arc::clone(&senders),
            ),
            cpal::SampleFormat::U8 => build_microphone_fanout_stream::<u8>(
                &device,
                stream_config,
                channels,
                Arc::clone(&senders),
            ),
            cpal::SampleFormat::U16 => build_microphone_fanout_stream::<u16>(
                &device,
                stream_config,
                channels,
                Arc::clone(&senders),
            ),
            cpal::SampleFormat::U24 => build_microphone_fanout_stream::<cpal::U24>(
                &device,
                stream_config,
                channels,
                Arc::clone(&senders),
            ),
            cpal::SampleFormat::U32 => build_microphone_fanout_stream::<u32>(
                &device,
                stream_config,
                channels,
                Arc::clone(&senders),
            ),
            cpal::SampleFormat::U64 => build_microphone_fanout_stream::<u64>(
                &device,
                stream_config,
                channels,
                Arc::clone(&senders),
            ),
            format => Err(AudioRouteError::StreamStart(format!(
                "unsupported microphone sample format: {format}"
            ))),
        }?;
        stream.play().map_err(|error| {
            AudioRouteError::StreamStart(format!("cannot start microphone node: {error}"))
        })?;
        Ok(MicrophoneFanout {
            senders,
            sample_rate,
            _stream: stream,
        })
    }

    /// Start capturing from a device by name.
    /// If name is empty, uses the system default input device.
    pub fn start_capture(
        &mut self,
        device_id: &str,
        tx: Sender<Vec<f32>>,
        level: Arc<AtomicU32>,
    ) -> Result<(), String> {
        let fanout = match &self.microphone_fanout {
            Some((active_id, fanout)) if active_id == device_id => Arc::clone(fanout),
            _ => {
                let fanout = Arc::new(
                    self.build_microphone_fanout(device_id)
                        .map_err(|error| error.to_string())?,
                );
                self.microphone_fanout = Some((device_id.to_owned(), Arc::clone(&fanout)));
                fanout
            }
        };
        let worker = Self::spawn_processing_worker_with_level(
            fanout.attach(),
            fanout.sample_rate,
            16_000,
            tx,
            Some(level),
        )
        .map_err(|error| format!("Failed to start microphone processing: {error}"))?;
        self.add_active_capture(ActiveCapture::MicrophoneSubscription(worker));
        Ok(())
    }

    #[allow(dead_code)]
    fn build_stream<T: Sample + cpal::SizedSample>(
        &self,
        device: &cpal::Device,
        input: (cpal::StreamConfig, usize, u32, u32),
        tx: Sender<Vec<f32>>,
        level: Arc<AtomicU32>,
    ) -> Result<Stream, String>
    where
        f32: cpal::FromSample<T>,
    {
        let (config, channels, src_rate, target_rate) = input;
        // 1. Resampler setup (if rates don't match)
        let (raw_tx, raw_rx) = bounded::<Vec<f32>>(32);
        Self::spawn_processing_worker(raw_rx, src_rate, target_rate, tx)?;

        let err_fn = |err| log::error!("An error occurred on the input audio stream: {}", err);

        let stream = device
            .build_input_stream(
                config,
                move |data: &[T], _: &cpal::InputCallbackInfo| {
                    // Keep the high-priority CPAL callback small: format conversion and mixdown only.
                    let mono: Vec<f32> = data
                        .chunks(channels)
                        .map(|frame| {
                            if frame.len() >= 6 {
                                // 5.1 / 7.1 Multi-channel Dialogue Isolation (Physical Noise Cancellation):
                                // Layout: [0: Left, 1: Right, 2: Center, 3: LFE(Subwoofer), 4: Surround L, 5: Surround R]
                                // - Center (frame[2]) contains 95%+ of actor dialogue.
                                // - LFE (frame[3]) is pure low-frequency rumble/explosions (0% dialogue), completely discarded.
                                // - Surround (frame[4], frame[5]) contains ambient/reverb (0% dialogue), heavily attenuated.
                                // - Left & Right contain music & panning sound effects, kept at low ratio for rare off-center lines.
                                let l = f32::from_sample(frame[0]);
                                let r = f32::from_sample(frame[1]);
                                let c = f32::from_sample(frame[2]);
                                let ls = f32::from_sample(frame[4]);
                                let rs = f32::from_sample(frame[5]);
                                c * 0.85 + (l + r) * 0.12 + (ls + rs) * 0.03
                            } else if frame.len() == 2 {
                                let l = f32::from_sample(frame[0]);
                                let r = f32::from_sample(frame[1]);
                                (l + r) * 0.5
                            } else {
                                frame
                                    .iter()
                                    .map(|sample| f32::from_sample(*sample))
                                    .sum::<f32>()
                                    / frame.len() as f32
                            }
                        })
                        .collect();
                    update_input_level(&mono, &level);
                    let _ = raw_tx.try_send(mono);
                },
                err_fn,
                None,
            )
            .map_err(|e| format!("Failed to build input stream: {}", e))?;

        Ok(stream)
    }

    fn build_route_microphone(
        &self,
        source: &AudioRouteSourceConfig,
        sink: Arc<AudioRouteSourceBuffer>,
        control: Arc<AudioRouteControl>,
        fanout: Option<&Arc<MicrophoneFanout>>,
    ) -> Result<(Option<Stream>, thread::JoinHandle<()>), AudioRouteError> {
        if let Some(fanout) = fanout {
            let worker =
                spawn_route_source_worker(fanout.attach(), fanout.sample_rate, Arc::clone(&sink))?;
            return Ok((None, worker));
        }
        let device = if source.device_id.is_empty() {
            self.host.default_input_device().ok_or_else(|| {
                AudioRouteError::DeviceUnavailable(
                    "no default microphone is available for the route".into(),
                )
            })?
        } else {
            let parsed_id = source.device_id.parse().map_err(|error| {
                AudioRouteError::DeviceUnavailable(format!(
                    "invalid microphone ID '{}': {error}",
                    source.device_id
                ))
            })?;
            self.host.device_by_id(&parsed_id).ok_or_else(|| {
                AudioRouteError::DeviceUnavailable(format!(
                    "microphone '{}' is no longer available",
                    source.device_id
                ))
            })?
        };
        let config = device.default_input_config().map_err(|error| {
            AudioRouteError::DeviceUnavailable(format!(
                "cannot read the selected microphone format: {error}"
            ))
        })?;
        let sample_rate = config.sample_rate();
        let channels = config.channels() as usize;
        let sample_format = config.sample_format();
        let stream_config: cpal::StreamConfig = config.into();
        let (raw_tx, raw_rx) = bounded::<Vec<f32>>(16);
        let worker = spawn_route_source_worker(raw_rx, sample_rate, Arc::clone(&sink))?;

        macro_rules! build_input {
            ($sample:ty) => {
                build_route_input_stream::<$sample>(
                    &device,
                    stream_config,
                    channels,
                    raw_tx,
                    sink,
                    control,
                )
            };
        }
        let stream = match sample_format {
            cpal::SampleFormat::F32 => build_input!(f32),
            cpal::SampleFormat::F64 => build_input!(f64),
            cpal::SampleFormat::I8 => build_input!(i8),
            cpal::SampleFormat::I16 => build_input!(i16),
            cpal::SampleFormat::I24 => build_input!(cpal::I24),
            cpal::SampleFormat::I32 => build_input!(i32),
            cpal::SampleFormat::I64 => build_input!(i64),
            cpal::SampleFormat::U8 => build_input!(u8),
            cpal::SampleFormat::U16 => build_input!(u16),
            cpal::SampleFormat::U24 => build_input!(cpal::U24),
            cpal::SampleFormat::U32 => build_input!(u32),
            cpal::SampleFormat::U64 => build_input!(u64),
            format => Err(AudioRouteError::StreamStart(format!(
                "unsupported microphone sample format: {format}"
            ))),
        }?;
        Ok((Some(stream), worker))
    }

    fn spawn_processing_worker(
        raw_rx: Receiver<Vec<f32>>,
        src_rate: u32,
        target_rate: u32,
        output_tx: Sender<Vec<f32>>,
    ) -> Result<(), String> {
        Self::spawn_processing_worker_with_level(raw_rx, src_rate, target_rate, output_tx, None)
            .map(|_| ())
    }

    fn spawn_processing_worker_with_level(
        raw_rx: Receiver<Vec<f32>>,
        src_rate: u32,
        target_rate: u32,
        output_tx: Sender<Vec<f32>>,
        level: Option<Arc<AtomicU32>>,
    ) -> Result<thread::JoinHandle<()>, String> {
        let mut resampler = if src_rate != target_rate {
            Some(
                Fft::<f32>::new(
                    src_rate as usize,
                    target_rate as usize,
                    1024,
                    1,
                    FixedSync::Input,
                )
                .map_err(|error| format!("Failed to create resampler: {error}"))?,
            )
        } else {
            None
        };

        thread::Builder::new()
            .name("audio-resampler".into())
            .spawn(move || {
                let mut pending = VecDeque::new();
                'worker: while let Ok(samples) = raw_rx.recv() {
                    if let Some(level) = &level {
                        update_input_level(&samples, level);
                    }
                    if let Some(resampler) = &mut resampler {
                        pending.extend(samples);
                        while pending.len() >= resampler.input_frames_next() {
                            let input_len = resampler.input_frames_next();
                            let input: Vec<f32> = pending.drain(..input_len).collect();
                            let output_capacity = resampler.output_frames_max();
                            let input_adapter = InterleavedSlice::new(&input, 1, input_len)
                                .expect("valid mono input");
                            let mut output = vec![0.0; output_capacity];
                            let mut output_adapter =
                                InterleavedSlice::new_mut(&mut output, 1, output_capacity)
                                    .expect("valid mono output");
                            if let Ok((_, frames_written)) = resampler.process_into_buffer(
                                &input_adapter,
                                &mut output_adapter,
                                None,
                            ) {
                                output.truncate(frames_written);
                                match output_tx.send_timeout(output, Duration::from_millis(100)) {
                                    Ok(()) => {}
                                    Err(crossbeam_channel::SendTimeoutError::Timeout(_)) => {}
                                    Err(crossbeam_channel::SendTimeoutError::Disconnected(_)) => {
                                        break 'worker;
                                    }
                                }
                            }
                        }
                    } else {
                        match output_tx.send_timeout(samples, Duration::from_millis(100)) {
                            Ok(()) => {}
                            Err(crossbeam_channel::SendTimeoutError::Timeout(_)) => {}
                            Err(crossbeam_channel::SendTimeoutError::Disconnected(_)) => {
                                break 'worker;
                            }
                        }
                    }
                }
            })
            .map_err(|error| format!("Failed to start audio processing thread: {error}"))
    }

    fn add_active_capture(&mut self, capture: ActiveCapture) {
        self.active_captures.push(capture);
    }

    fn ensure_tts_player(&mut self, device_id: &str) -> Result<(), String> {
        if self
            .tts_player
            .as_ref()
            .is_some_and(|player| player.device_id == device_id)
        {
            return Ok(());
        }
        self.tts_player = None;
        let device = if device_id.is_empty() {
            self.host
                .default_output_device()
                .ok_or("No default audio output device available for TTS playback")?
        } else {
            let parsed_id = device_id
                .parse()
                .map_err(|error| format!("Invalid TTS output ID '{device_id}': {error}"))?;
            self.host
                .device_by_id(&parsed_id)
                .ok_or_else(|| format!("TTS output '{device_id}' is no longer available"))?
        };
        let config = device
            .default_output_config()
            .map_err(|error| format!("Cannot read TTS output format: {error}"))?;
        let sample_rate = config.sample_rate();
        let channels = config.channels() as usize;
        let sample_format = config.sample_format();
        let queue = Arc::new(Mutex::new(VecDeque::new()));
        let playback_tail_samples = Arc::new(AtomicU64::new(0));
        let stream_config: cpal::StreamConfig = config.into();
        let stream = match sample_format {
            cpal::SampleFormat::F32 => build_tts_output_stream::<f32>(
                &device,
                stream_config,
                channels,
                Arc::clone(&queue),
                Arc::clone(&playback_tail_samples),
            ),
            cpal::SampleFormat::F64 => build_tts_output_stream::<f64>(
                &device,
                stream_config,
                channels,
                Arc::clone(&queue),
                Arc::clone(&playback_tail_samples),
            ),
            cpal::SampleFormat::I8 => build_tts_output_stream::<i8>(
                &device,
                stream_config,
                channels,
                Arc::clone(&queue),
                Arc::clone(&playback_tail_samples),
            ),
            cpal::SampleFormat::I16 => build_tts_output_stream::<i16>(
                &device,
                stream_config,
                channels,
                Arc::clone(&queue),
                Arc::clone(&playback_tail_samples),
            ),
            cpal::SampleFormat::I24 => build_tts_output_stream::<cpal::I24>(
                &device,
                stream_config,
                channels,
                Arc::clone(&queue),
                Arc::clone(&playback_tail_samples),
            ),
            cpal::SampleFormat::I32 => build_tts_output_stream::<i32>(
                &device,
                stream_config,
                channels,
                Arc::clone(&queue),
                Arc::clone(&playback_tail_samples),
            ),
            cpal::SampleFormat::I64 => build_tts_output_stream::<i64>(
                &device,
                stream_config,
                channels,
                Arc::clone(&queue),
                Arc::clone(&playback_tail_samples),
            ),
            cpal::SampleFormat::U8 => build_tts_output_stream::<u8>(
                &device,
                stream_config,
                channels,
                Arc::clone(&queue),
                Arc::clone(&playback_tail_samples),
            ),
            cpal::SampleFormat::U16 => build_tts_output_stream::<u16>(
                &device,
                stream_config,
                channels,
                Arc::clone(&queue),
                Arc::clone(&playback_tail_samples),
            ),
            cpal::SampleFormat::U24 => build_tts_output_stream::<cpal::U24>(
                &device,
                stream_config,
                channels,
                Arc::clone(&queue),
                Arc::clone(&playback_tail_samples),
            ),
            cpal::SampleFormat::U32 => build_tts_output_stream::<u32>(
                &device,
                stream_config,
                channels,
                Arc::clone(&queue),
                Arc::clone(&playback_tail_samples),
            ),
            cpal::SampleFormat::U64 => build_tts_output_stream::<u64>(
                &device,
                stream_config,
                channels,
                Arc::clone(&queue),
                Arc::clone(&playback_tail_samples),
            ),
            format => Err(format!("Unsupported TTS output sample format: {format}")),
        }?;
        stream
            .play()
            .map_err(|error| format!("Cannot start TTS output stream: {error}"))?;
        log::info!(
            "TTS output stream started: device_id={:?}, sample_rate={}, channels={}, sample_format={}",
            device_id,
            sample_rate,
            channels,
            sample_format
        );
        self.tts_player = Some(TtsPlayer {
            queue,
            sample_rate,
            device_id: device_id.to_owned(),
            playback_tail_samples,
            _stream: stream,
        });
        Ok(())
    }
}

fn validate_route_gain(gain: f32) -> Result<(), AudioRouteError> {
    if !gain.is_finite() || !(0.0..=8.0).contains(&gain) {
        return Err(AudioRouteError::InvalidConfiguration(
            "source gains must be finite values between 0.0 and 8.0".into(),
        ));
    }
    Ok(())
}

fn validate_audio_route_config(config: &AudioRouteConfig) -> Result<(), AudioRouteError> {
    if config.microphone.is_none() && config.system_loopback.is_none() && config.tts_gain.is_none()
    {
        return Err(AudioRouteError::InvalidConfiguration(
            "connect at least one microphone, system-loopback, or TTS source".into(),
        ));
    }
    if !(20..=2_000).contains(&config.queue_capacity_ms) {
        return Err(AudioRouteError::InvalidConfiguration(
            "queue_capacity_ms must be between 20 and 2000 milliseconds".into(),
        ));
    }
    if let Some(source) = &config.microphone {
        validate_route_gain(source.gain)?;
    }
    if let Some(source) = &config.system_loopback {
        validate_route_gain(source.gain)?;
        if let AudioRouteLoopbackTarget::Application { process_id, .. } = &source.target {
            if *process_id == 0 {
                return Err(AudioRouteError::InvalidConfiguration(
                    "application audio capture requires a live process".into(),
                ));
            }
            if *process_id == std::process::id() {
                return Err(AudioRouteError::InvalidConfiguration(
                    "capturing XRTranslate's own process into its audio route would create a feedback loop"
                        .into(),
                ));
            }
        }
        if let AudioRouteLoopbackTarget::Endpoint { device_id } = &source.target
            && device_id == &config.output_device_id
        {
            let endpoint = if device_id.is_empty() {
                "the default render endpoint"
            } else {
                "the same explicit render endpoint"
            };
            return Err(AudioRouteError::InvalidConfiguration(format!(
                "system loopback and route output both select {endpoint}; choose a different output (normally a virtual cable) to prevent an audio feedback loop"
            )));
        }
    }
    if let Some(gain) = config.tts_gain {
        validate_route_gain(gain)?;
    }
    if !config.output_ceiling.is_finite() || !(0.01..=1.0).contains(&config.output_ceiling) {
        return Err(AudioRouteError::InvalidConfiguration(
            "output_ceiling must be a finite linear value between 0.01 and 1.0".into(),
        ));
    }
    Ok(())
}

fn route_source_level(source: &Option<Arc<AudioRouteSourceBuffer>>) -> f32 {
    source.as_ref().map_or(0.0, |source| source.level())
}

fn encode_route_state(state: AudioRouteState) -> u8 {
    match state {
        AudioRouteState::Starting => 0,
        AudioRouteState::Running => 1,
        AudioRouteState::Stopping => 2,
        AudioRouteState::Stopped => 3,
        AudioRouteState::Faulted => 4,
    }
}

fn decode_route_state(state: u8) -> AudioRouteState {
    match state {
        0 => AudioRouteState::Starting,
        1 => AudioRouteState::Running,
        2 => AudioRouteState::Stopping,
        3 => AudioRouteState::Stopped,
        _ => AudioRouteState::Faulted,
    }
}

fn pcm16_mono_samples(pcm: &[u8]) -> Vec<f32> {
    pcm.chunks_exact(2)
        .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]) as f32 / 32768.0)
        .collect()
}

fn build_route_input_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    channels: usize,
    raw_tx: Sender<Vec<f32>>,
    source: Arc<AudioRouteSourceBuffer>,
    control: Arc<AudioRouteControl>,
) -> Result<Stream, AudioRouteError>
where
    T: Sample + cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    device
        .build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                let mono = data
                    .chunks(channels)
                    .map(microphone_frame_to_mono::<T>)
                    .collect::<Vec<_>>();
                update_input_level(&mono, &source.level);
                if let Err(error) = raw_tx.try_send(mono) {
                    source
                        .dropped_samples
                        .fetch_add(error.into_inner().len() as u64, Ordering::Relaxed);
                }
            },
            move |error| {
                let message = format!("route microphone stream failed: {error}");
                log::error!("{message}");
                *control.last_error.lock() = Some(message);
                control.state.store(
                    encode_route_state(AudioRouteState::Faulted),
                    Ordering::Release,
                );
            },
            None,
        )
        .map_err(|error| {
            AudioRouteError::StreamStart(format!("cannot create microphone node: {error}"))
        })
}

fn build_microphone_fanout_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    channels: usize,
    senders: Arc<Mutex<Vec<Sender<Vec<f32>>>>>,
) -> Result<Stream, AudioRouteError>
where
    T: Sample + cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    device
        .build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                let mono = data
                    .chunks(channels)
                    .map(microphone_frame_to_mono::<T>)
                    .collect::<Vec<_>>();
                let mut subscribers = senders.lock();
                subscribers.retain(|sender| match sender.try_send(mono.clone()) {
                    Ok(()) | Err(TrySendError::Full(_)) => true,
                    Err(TrySendError::Disconnected(_)) => false,
                });
            },
            move |error| {
                log::error!("shared microphone stream failed: {error}");
            },
            None,
        )
        .map_err(|error| {
            AudioRouteError::StreamStart(format!("cannot create shared microphone node: {error}"))
        })
}

fn microphone_frame_to_mono<T>(frame: &[T]) -> f32
where
    T: Sample,
    f32: cpal::FromSample<T>,
{
    if frame.len() >= 6 {
        let left = f32::from_sample(frame[0]);
        let right = f32::from_sample(frame[1]);
        let center = f32::from_sample(frame[2]);
        let surround_left = f32::from_sample(frame[4]);
        let surround_right = f32::from_sample(frame[5]);
        center * 0.85 + (left + right) * 0.12 + (surround_left + surround_right) * 0.03
    } else if frame.len() == 2 {
        (f32::from_sample(frame[0]) + f32::from_sample(frame[1])) * 0.5
    } else {
        frame
            .iter()
            .map(|sample| f32::from_sample(*sample))
            .sum::<f32>()
            / frame.len().max(1) as f32
    }
}

fn spawn_route_source_worker(
    raw_rx: Receiver<Vec<f32>>,
    source_rate: u32,
    sink: Arc<AudioRouteSourceBuffer>,
) -> Result<thread::JoinHandle<()>, AudioRouteError> {
    let mut resampler = if source_rate == AUDIO_ROUTE_SAMPLE_RATE {
        None
    } else {
        Some(
            Fft::<f32>::new(
                source_rate as usize,
                AUDIO_ROUTE_SAMPLE_RATE as usize,
                480,
                1,
                FixedSync::Input,
            )
            .map_err(|error| {
                AudioRouteError::StreamStart(format!(
                    "cannot initialize the 48 kHz route resampler: {error}"
                ))
            })?,
        )
    };
    thread::Builder::new()
        .name("audio-route-source".into())
        .spawn(move || {
            let mut pending = VecDeque::new();
            while let Ok(samples) = raw_rx.recv() {
                update_input_level(&samples, &sink.level);
                let Some(resampler) = &mut resampler else {
                    sink.push(samples);
                    continue;
                };
                pending.extend(samples);
                while pending.len() >= resampler.input_frames_next() {
                    let input_len = resampler.input_frames_next();
                    let input = pending.drain(..input_len).collect::<Vec<_>>();
                    let input_adapter = InterleavedSlice::new(&input, 1, input_len)
                        .expect("valid mono route input");
                    let capacity = resampler.output_frames_max();
                    let mut output = vec![0.0; capacity];
                    let mut output_adapter = InterleavedSlice::new_mut(&mut output, 1, capacity)
                        .expect("valid mono route output");
                    match resampler.process_into_buffer(&input_adapter, &mut output_adapter, None) {
                        Ok((_, written)) => {
                            output.truncate(written);
                            sink.push(output);
                        }
                        Err(error) => {
                            log::error!("Audio route resampling failed: {error}");
                            sink.dropped_samples
                                .fetch_add(input_len as u64, Ordering::Relaxed);
                        }
                    }
                }
            }
        })
        .map_err(|error| {
            AudioRouteError::StreamStart(format!("cannot start route resampler: {error}"))
        })
}

#[derive(Default)]
struct RouteRateReader {
    phase: f64,
    current: f32,
    next: f32,
    primed: bool,
}

impl RouteRateReader {
    fn read(&mut self, queue: &mut VecDeque<f32>, output_rate: u32) -> f32 {
        if !self.primed {
            self.current = queue.pop_front().unwrap_or(0.0);
            self.next = queue.pop_front().unwrap_or(self.current);
            self.primed = true;
        }
        let sample = self.current + (self.next - self.current) * self.phase as f32;
        self.phase += f64::from(AUDIO_ROUTE_SAMPLE_RATE) / f64::from(output_rate);
        while self.phase >= 1.0 {
            self.current = self.next;
            self.next = queue.pop_front().unwrap_or(0.0);
            self.phase -= 1.0;
        }
        sample
    }
}

#[allow(clippy::too_many_arguments)]
fn build_route_output_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    channels: usize,
    output_rate: u32,
    microphone: Option<Arc<AudioRouteSourceBuffer>>,
    system_loopback: Option<Arc<AudioRouteSourceBuffer>>,
    tts: Option<Arc<AudioRouteSourceBuffer>>,
    output_ceiling: f32,
    output_level: Arc<AtomicU32>,
    control: Arc<AudioRouteControl>,
) -> Result<Stream, AudioRouteError>
where
    T: Sample + cpal::SizedSample + cpal::FromSample<f32>,
{
    let callback_control = Arc::clone(&control);
    let mut microphone_reader = RouteRateReader::default();
    let mut loopback_reader = RouteRateReader::default();
    let mut tts_reader = RouteRateReader::default();
    device
        .build_output_stream(
            config,
            move |output: &mut [T], _: &cpal::OutputCallbackInfo| {
                let mut microphone_queue = microphone.as_ref().map(|source| source.queue.lock());
                let mut loopback_queue = system_loopback.as_ref().map(|source| source.queue.lock());
                let mut tts_queue = tts.as_ref().map(|source| source.queue.lock());
                let microphone_gain = microphone.as_ref().map_or(0.0, |source| source.gain());
                let loopback_gain = system_loopback.as_ref().map_or(0.0, |source| source.gain());
                let tts_gain = tts.as_ref().map_or(0.0, |source| source.gain());
                let mut energy = 0.0;
                let mut frames = 0;
                for frame in output.chunks_mut(channels) {
                    let microphone_sample = microphone_queue
                        .as_mut()
                        .map_or(0.0, |queue| microphone_reader.read(queue, output_rate));
                    let loopback_sample = loopback_queue
                        .as_mut()
                        .map_or(0.0, |queue| loopback_reader.read(queue, output_rate));
                    let tts_sample = tts_queue
                        .as_mut()
                        .map_or(0.0, |queue| tts_reader.read(queue, output_rate));
                    if let Some(tts) = &tts {
                        decrement_playback_tail(&tts.playback_tail_samples);
                    }
                    let mixed = (microphone_sample * microphone_gain
                        + loopback_sample * loopback_gain
                        + tts_sample * tts_gain)
                        .clamp(-output_ceiling, output_ceiling);
                    energy += mixed * mixed;
                    frames += 1;
                    for channel in frame {
                        *channel = T::from_sample(mixed);
                    }
                }
                update_input_level_from_energy(energy, frames, &output_level);
            },
            move |error| {
                let message = format!("route output stream failed: {error}");
                log::error!("{message}");
                *callback_control.last_error.lock() = Some(message);
                callback_control.state.store(
                    encode_route_state(AudioRouteState::Faulted),
                    Ordering::Release,
                );
            },
            None,
        )
        .map_err(|error| {
            AudioRouteError::StreamStart(format!("cannot create output node: {error}"))
        })
}

fn build_tts_output_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    channels: usize,
    queue: Arc<Mutex<VecDeque<f32>>>,
    playback_tail_samples: Arc<AtomicU64>,
) -> Result<Stream, String>
where
    T: cpal::Sample + cpal::SizedSample + cpal::FromSample<f32>,
{
    let error_callback = |error| log::error!("TTS output stream error: {error}");
    device
        .build_output_stream(
            config,
            move |output: &mut [T], _: &cpal::OutputCallbackInfo| {
                let mut pending = queue.lock();
                for frame in output.chunks_mut(channels) {
                    let sample = pending.pop_front().unwrap_or(0.0);
                    decrement_playback_tail(&playback_tail_samples);
                    for channel in frame {
                        *channel = T::from_sample(sample);
                    }
                }
            },
            error_callback,
            None,
        )
        .map_err(|error| format!("Cannot create TTS output stream: {error}"))
}

fn decrement_playback_tail(remaining: &AtomicU64) {
    // Each stream owns one output callback, so a relaxed decrement is enough.
    // Avoid a compare/exchange on every silent frame.
    if remaining.load(Ordering::Relaxed) != 0 {
        remaining.fetch_sub(1, Ordering::Relaxed);
    }
}

fn resample_mono(
    samples: Vec<f32>,
    source_rate: u32,
    target_rate: u32,
) -> Result<Vec<f32>, String> {
    if source_rate == target_rate || samples.len() < 2 {
        return Ok(samples);
    }
    let expected =
        ((samples.len() as u64 * u64::from(target_rate)).div_ceil(u64::from(source_rate))) as usize;
    let mut resampler = Fft::<f32>::new(
        source_rate as usize,
        target_rate as usize,
        1024,
        1,
        FixedSync::Input,
    )
    .map_err(|error| format!("Cannot initialize TTS resampler: {error}"))?;
    let mut output = Vec::with_capacity(expected);
    let mut offset = 0;
    let mut trim_remaining = resampler.output_delay();
    while output.len() < expected {
        let required = resampler.input_frames_next();
        let valid = samples.len().saturating_sub(offset).min(required);
        let mut input = if valid == 0 {
            vec![0.0; required]
        } else {
            samples[offset..offset + valid].to_vec()
        };
        input.resize(required, 0.0);
        offset += valid;
        let input = InterleavedSlice::new(&input, 1, required)
            .map_err(|error| format!("Invalid TTS resampler input: {error}"))?;
        let capacity = resampler.output_frames_max();
        let mut chunk = vec![0.0; capacity];
        let mut output_adapter = InterleavedSlice::new_mut(&mut chunk, 1, capacity)
            .map_err(|error| format!("Invalid TTS resampler output: {error}"))?;
        let indexing = Indexing {
            partial_len: (valid < required).then_some(valid),
            ..Indexing::default()
        };
        let before = output.len();
        let (_, written) = resampler
            .process_into_buffer(&input, &mut output_adapter, Some(&indexing))
            .map_err(|error| format!("TTS resampling failed: {error}"))?;
        chunk.truncate(written);
        let trim = trim_remaining.min(chunk.len());
        chunk.drain(..trim);
        trim_remaining -= trim;
        chunk.truncate(expected - output.len());
        output.extend(chunk);
        if valid == 0 && output.len() == before {
            return Err("TTS resampler could not flush its delayed output".into());
        }
    }
    Ok(output)
}

#[cfg(windows)]
impl AudioSystem {
    /// List playback endpoints that can be captured with WASAPI loopback.
    pub fn available_loopback_devices(&self) -> Vec<InputDevice> {
        let Ok(enumerator) = DeviceEnumerator::new() else {
            return Vec::new();
        };
        let Ok(devices) = enumerator.get_device_collection(&Direction::Render) else {
            return Vec::new();
        };
        devices
            .into_iter()
            .filter_map(Result::ok)
            .filter_map(|device| {
                Some(InputDevice {
                    id: device.get_id().ok()?,
                    name: device.get_friendlyname().ok()?,
                })
            })
            .collect()
    }

    /// Read the Windows shared-mode format used by an output endpoint.
    pub fn loopback_config(&self, device_id: &str) -> Result<InputConfigInfo, String> {
        let enumerator = DeviceEnumerator::new()
            .map_err(|error| format!("Cannot enumerate playback devices: {error}"))?;
        let device = if device_id.is_empty() {
            enumerator
                .get_default_device(&Direction::Render)
                .map_err(|error| format!("No default playback device available: {error}"))?
        } else {
            enumerator
                .get_device(device_id)
                .map_err(|error| format!("Playback device is no longer available: {error}"))?
        };
        let format = device
            .get_device_format()
            .map_err(|error| format!("Cannot read playback format: {error}"))?;
        Ok(InputConfigInfo {
            sample_rate: format.get_samplespersec(),
            channels: format.get_nchannels(),
            sample_format: format
                .get_subformat()
                .map(|sample_type| sample_type.to_string())
                .unwrap_or_else(|_| "Unknown".into()),
        })
    }

    /// Capture the selected output endpoint through Windows WASAPI loopback.
    pub fn start_loopback_capture(
        &mut self,
        device_id: &str,
        output_tx: Sender<Vec<f32>>,
        level: Arc<AtomicU32>,
    ) -> Result<(), String> {
        self.start_loopback_target(
            AudioRouteLoopbackTarget::Endpoint {
                device_id: device_id.to_owned(),
            },
            Some(output_tx),
            level,
            "wasapi-loopback",
        )
    }

    pub fn start_application_loopback_capture(
        &mut self,
        process_id: u32,
        application_name: &str,
        output_tx: Sender<Vec<f32>>,
        level: Arc<AtomicU32>,
    ) -> Result<(), String> {
        self.start_loopback_target(
            AudioRouteLoopbackTarget::Application {
                process_id,
                application_name: application_name.to_owned(),
            },
            Some(output_tx),
            level,
            "wasapi-application-loopback",
        )
    }

    fn start_loopback_target(
        &mut self,
        target: AudioRouteLoopbackTarget,
        output_tx: Option<Sender<Vec<f32>>>,
        level: Arc<AtomicU32>,
        worker_name: &str,
    ) -> Result<(), String> {
        let stop_requested = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop_requested);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name(worker_name.into())
            .spawn(move || {
                if let Err(error) = run_loopback_capture(
                    &target,
                    output_tx,
                    level,
                    worker_stop,
                    &ready_tx,
                    16_000,
                    None,
                ) {
                    let _ = ready_tx.send(Err(error.clone()));
                    log::error!("WASAPI loopback capture stopped: {error}");
                }
            })
            .map_err(|error| format!("Failed to start WASAPI loopback worker: {error}"))?;

        match ready_rx.recv_timeout(Duration::from_secs(2)) {
            Ok(Ok(())) => {
                self.add_active_capture(ActiveCapture::Loopback(LoopbackCapture {
                    stop_requested,
                    worker: Some(worker),
                }));
                Ok(())
            }
            Ok(Err(error)) => {
                stop_requested.store(true, Ordering::Release);
                reap_worker(worker);
                Err(error)
            }
            Err(_) => {
                stop_requested.store(true, Ordering::Release);
                reap_worker(worker);
                Err("Timed out while opening the WASAPI loopback device".into())
            }
        }
    }
}

#[cfg(not(windows))]
impl AudioSystem {
    /// Linux capture uses CPAL input devices. System-audio loopback is
    /// backend-specific (PipeWire/PulseAudio) and is intentionally reported as
    /// unavailable until a dedicated implementation is selected.
    pub fn available_loopback_devices(&self) -> Vec<InputDevice> {
        Vec::new()
    }

    pub fn loopback_config(&self, _device_id: &str) -> Result<InputConfigInfo, String> {
        Err("system-audio loopback is not available on this build".into())
    }

    pub fn start_loopback_capture(
        &mut self,
        _device_id: &str,
        _output_tx: Sender<Vec<f32>>,
        _level: Arc<AtomicU32>,
    ) -> Result<(), String> {
        Err("system-audio loopback is not available on this build".into())
    }

    pub fn start_application_loopback_capture(
        &mut self,
        _process_id: u32,
        _application_name: &str,
        _output_tx: Sender<Vec<f32>>,
        _level: Arc<AtomicU32>,
    ) -> Result<(), String> {
        Err("application-audio loopback is not available on this build".into())
    }
}

#[cfg(windows)]
fn enumerate_audio_applications() -> Result<Vec<AudioApplication>, String> {
    initialize_mta()
        .ok()
        .map_err(|error| format!("Cannot initialize WASAPI: {error}"))?;
    struct ComApartmentGuard;
    impl Drop for ComApartmentGuard {
        fn drop(&mut self) {
            deinitialize();
        }
    }
    let _apartment = ComApartmentGuard;
    let enumerator = DeviceEnumerator::new()
        .map_err(|error| format!("Cannot enumerate playback devices: {error}"))?;
    let endpoints = enumerator
        .get_device_collection(&Direction::Render)
        .map_err(|error| format!("Cannot enumerate render endpoints: {error}"))?;
    let mut applications = BTreeMap::<String, AudioApplication>::new();
    for endpoint in &endpoints {
        let Ok(endpoint) = endpoint else { continue };
        let Ok(manager) = endpoint.get_iaudiosessionmanager() else {
            continue;
        };
        let Ok(sessions) = manager.get_audiosessionenumerator() else {
            continue;
        };
        let Ok(count) = sessions.get_count() else {
            continue;
        };
        for index in 0..count {
            let Ok(session) = sessions.get_session(index) else {
                continue;
            };
            let Ok(process_id) = session.get_process_id() else {
                continue;
            };
            if process_id == 0 {
                continue;
            }
            let Some(path) = process_executable_path(process_id) else {
                continue;
            };
            let id = path.to_lowercase();
            let name = std::path::Path::new(&path)
                .file_stem()
                .and_then(|name| name.to_str())
                .filter(|name| !name.trim().is_empty())
                .unwrap_or("Audio application")
                .to_owned();
            let active = session
                .get_state()
                .is_ok_and(|state| state == SessionState::Active);
            let candidate = AudioApplication {
                id: id.clone(),
                name,
                process_id,
                active,
            };
            match applications.get_mut(&id) {
                Some(current) if !current.active && candidate.active => *current = candidate,
                None => {
                    applications.insert(id, candidate);
                }
                _ => {}
            }
        }
    }
    let mut applications = applications.into_values().collect::<Vec<_>>();
    applications.sort_by(|left, right| {
        right
            .active
            .cmp(&left.active)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    Ok(applications)
}

#[cfg(windows)]
fn process_executable_path(process_id: u32) -> Option<String> {
    use windows::Win32::{
        Foundation::CloseHandle,
        System::Threading::{
            OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
            QueryFullProcessImageNameW,
        },
    };
    use windows::core::PWSTR;

    let process =
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id).ok()? };
    let mut buffer = vec![0u16; 32_768];
    let mut length = buffer.len() as u32;
    let result = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
    };
    let _ = unsafe { CloseHandle(process) };
    result.ok()?;
    String::from_utf16(&buffer[..length as usize]).ok()
}

#[cfg(windows)]
fn run_loopback_capture(
    target: &AudioRouteLoopbackTarget,
    output_tx: Option<Sender<Vec<f32>>>,
    level: Arc<AtomicU32>,
    stop_requested: Arc<AtomicBool>,
    ready_tx: &std::sync::mpsc::SyncSender<Result<(), String>>,
    target_rate: u32,
    dropped_samples: Option<Arc<AtomicU64>>,
) -> Result<(), String> {
    initialize_mta()
        .ok()
        .map_err(|error| format!("Cannot initialize WASAPI: {error}"))?;
    let (mut client, name) = open_loopback_client(target)?;
    let format = WaveFormat::new(32, 32, &SampleType::Float, 48_000, 2, None);
    let mode = StreamMode::EventsShared {
        autoconvert: true,
        buffer_duration_hns: 200_000,
    };
    client
        .initialize_client(&format, &Direction::Capture, &mode)
        .map_err(|error| format!("Cannot initialize WASAPI loopback capture: {error}"))?;
    let event = client
        .set_get_eventhandle()
        .map_err(|error| format!("Cannot create WASAPI loopback event: {error}"))?;
    let capture = client
        .get_audiocaptureclient()
        .map_err(|error| format!("Cannot create WASAPI capture client: {error}"))?;
    let raw_tx = match output_tx {
        Some(output_tx) if target_rate == AUDIO_ROUTE_SAMPLE_RATE => Some(output_tx),
        Some(output_tx) => {
            let (raw_tx, raw_rx) = bounded::<Vec<f32>>(32);
            AudioSystem::spawn_processing_worker(
                raw_rx,
                AUDIO_ROUTE_SAMPLE_RATE,
                target_rate,
                output_tx,
            )?;
            Some(raw_tx)
        }
        None => None,
    };
    client
        .start_stream()
        .map_err(|error| format!("Cannot start WASAPI loopback capture: {error}"))?;
    log::info!("Started WASAPI loopback capture on '{name}'");
    let _ = ready_tx.send(Ok(()));

    let bytes_per_frame = format.get_blockalign() as usize;
    let mut pending = VecDeque::new();
    let mut last_audio_received = std::time::Instant::now();
    let mut last_waiting_log = std::time::Instant::now();
    while !stop_requested.load(Ordering::Acquire) {
        let pending_before_read = pending.len();
        capture
            .read_from_device_to_deque(&mut pending)
            .map_err(|error| format!("WASAPI loopback read failed: {error}"))?;
        if pending.len() > pending_before_read {
            last_audio_received = std::time::Instant::now();
        } else if last_audio_received.elapsed() >= Duration::from_secs(3)
            && last_waiting_log.elapsed() >= Duration::from_secs(3)
        {
            log::info!(
                "WASAPI loopback is running but has not received audio samples; verify that this is the active Windows playback device"
            );
            last_waiting_log = std::time::Instant::now();
        }
        while pending.len() >= bytes_per_frame * 960 {
            let samples = take_loopback_mono(&mut pending, 960);
            update_input_level(&samples, &level);
            if let Some(raw_tx) = &raw_tx {
                if let Err(error) = raw_tx.try_send(samples)
                    && let Some(dropped_samples) = &dropped_samples
                {
                    dropped_samples.fetch_add(error.into_inner().len() as u64, Ordering::Relaxed);
                }
            }
        }
        // Keep route stop/apply responsive. `wait_for_event` takes
        // milliseconds; the former 100_000 value could strand a worker for
        // roughly 100 seconds after a route change.
        let _ = event.wait_for_event(100);
    }
    let _ = client.stop_stream();
    Ok(())
}

fn update_input_level(samples: &[f32], level: &AtomicU32) {
    if samples.is_empty() {
        return;
    }

    update_input_level_from_energy(
        samples.iter().map(|sample| sample * sample).sum(),
        samples.len(),
        level,
    );
}

fn update_input_level_from_energy(sum: f32, sample_count: usize, level: &AtomicU32) {
    if sample_count == 0 {
        return;
    }
    let rms = (sum / sample_count as f32).sqrt().clamp(0.0, 1.0);
    let previous = f32::from_bits(level.load(Ordering::Relaxed));
    // A quick rise and gentle fall makes speech activity readable without the
    // meter flickering at the audio callback rate.
    let smoothed = if rms > previous {
        previous * 0.35 + rms * 0.65
    } else {
        previous * 0.8 + rms * 0.2
    };
    level.store(smoothed.to_bits(), Ordering::Relaxed);
}

#[cfg(windows)]
fn open_loopback_client(
    target: &AudioRouteLoopbackTarget,
) -> Result<(AudioClient, String), String> {
    if let AudioRouteLoopbackTarget::Application {
        process_id,
        application_name,
    } = target
    {
        if *process_id == 0 {
            return Err("The selected application's audio session is no longer available".into());
        }
        let client = AudioClient::new_application_loopback_client(*process_id, true)
            .map_err(|error| format!("Cannot capture audio from {application_name}: {error}"))?;
        return Ok((client, format!("{application_name} (process {process_id})")));
    }
    let AudioRouteLoopbackTarget::Endpoint { device_id } = target else {
        unreachable!();
    };
    // Endpoint IDs can briefly remain enumerable while Windows is re-registering a
    // USB, Bluetooth, VR, or virtual-audio device.  In that window Activate can
    // return ERROR_FILE_NOT_FOUND (0x80070002).  Re-resolve the endpoint before
    // each attempt so the default selection also follows an endpoint change.
    const MAX_ATTEMPTS: u8 = 3;
    let target = if device_id.is_empty() {
        "the default playback device"
    } else {
        "the selected playback device"
    };
    let mut last_error = None;

    for attempt in 1..=MAX_ATTEMPTS {
        let result = (|| -> Result<(AudioClient, String), String> {
            let enumerator = DeviceEnumerator::new()
                .map_err(|error| format!("Cannot enumerate playback devices: {error}"))?;
            let device = if device_id.is_empty() {
                enumerator
                    .get_default_device(&Direction::Render)
                    .map_err(|error| format!("No default playback device available: {error}"))?
            } else {
                enumerator
                    .get_device(device_id)
                    .map_err(|error| format!("Playback device is no longer available: {error}"))?
            };
            let name = device
                .get_friendlyname()
                .unwrap_or_else(|_| "selected playback device".into());
            let client = device
                .get_iaudioclient()
                .map_err(|error| format!("Cannot open WASAPI loopback device: {error}"))?;
            Ok((client, name))
        })();

        match result {
            Ok(client) => return Ok(client),
            Err(error) => {
                last_error = Some(error);
                if attempt < MAX_ATTEMPTS {
                    log::warn!(
                        "WASAPI loopback activation failed for {target} (attempt {attempt}/{MAX_ATTEMPTS}); retrying"
                    );
                    thread::sleep(Duration::from_millis(250));
                }
            }
        }
    }

    Err(format!(
        "Cannot open WASAPI loopback device after {MAX_ATTEMPTS} attempts ({target}): {}",
        last_error.unwrap_or_else(|| "unknown error".into())
    ))
}

#[cfg(windows)]
fn take_loopback_mono(pending: &mut VecDeque<u8>, frames: usize) -> Vec<f32> {
    let mut mono = Vec::with_capacity(frames);
    for _ in 0..frames {
        let left = f32::from_le_bytes([
            pending.pop_front().unwrap_or_default(),
            pending.pop_front().unwrap_or_default(),
            pending.pop_front().unwrap_or_default(),
            pending.pop_front().unwrap_or_default(),
        ]);
        let right = f32::from_le_bytes([
            pending.pop_front().unwrap_or_default(),
            pending.pop_front().unwrap_or_default(),
            pending.pop_front().unwrap_or_default(),
            pending.pop_front().unwrap_or_default(),
        ]);
        mono.push((left + right) * 0.5);
    }
    mono
}

#[cfg(test)]
mod tests {
    use super::{
        AudioRouteConfig, AudioRouteError, AudioRouteLoopbackConfig, AudioRouteLoopbackTarget,
        AudioRouteSourceBuffer, AudioRouteSourceConfig, resample_mono, validate_audio_route_config,
    };
    use std::sync::{Arc, atomic::AtomicU64};

    #[test]
    fn tts_resampling_preserves_duration_and_signal() {
        let source = (0..4_410)
            .map(|frame| ((frame as f32 / 44_100.0) * 440.0 * std::f32::consts::TAU).sin())
            .collect();
        let output = resample_mono(source, 44_100, 48_000).unwrap();
        assert_eq!(output.len(), 4_800);
        assert!(output.iter().all(|sample| sample.is_finite()));
        assert!(output.iter().any(|sample| sample.abs() > 0.5));
    }

    #[test]
    fn route_rejects_a_direct_loopback_feedback_cycle() {
        let config = AudioRouteConfig {
            system_loopback: Some(AudioRouteLoopbackConfig {
                target: AudioRouteLoopbackTarget::Endpoint {
                    device_id: String::new(),
                },
                gain: 1.0,
            }),
            tts_gain: None,
            ..AudioRouteConfig::default()
        };
        let error = validate_audio_route_config(&config).unwrap_err();
        assert!(matches!(error, AudioRouteError::InvalidConfiguration(_)));
        assert!(error.to_string().contains("feedback loop"));
    }

    #[test]
    fn route_source_queue_is_bounded_and_keeps_recent_audio() {
        let dropped = Arc::new(AtomicU64::new(0));
        let source = AudioRouteSourceBuffer::new(3, 1.0, Arc::clone(&dropped));
        source.push(vec![0.1, 0.2, 0.3, 0.4, 0.5]);
        assert_eq!(
            source.queue.lock().iter().copied().collect::<Vec<_>>(),
            vec![0.3, 0.4, 0.5]
        );
        assert_eq!(dropped.load(std::sync::atomic::Ordering::Relaxed), 2);
    }

    #[test]
    fn route_rejects_invalid_gain_and_queue_latency() {
        let invalid_gain = AudioRouteConfig {
            microphone: Some(AudioRouteSourceConfig {
                gain: f32::NAN,
                ..AudioRouteSourceConfig::default()
            }),
            ..AudioRouteConfig::default()
        };
        assert!(validate_audio_route_config(&invalid_gain).is_err());

        let invalid_latency = AudioRouteConfig {
            queue_capacity_ms: 2_001,
            ..AudioRouteConfig::default()
        };
        assert!(validate_audio_route_config(&invalid_latency).is_err());

        let invalid_ceiling = AudioRouteConfig {
            output_ceiling: 1.1,
            ..AudioRouteConfig::default()
        };
        assert!(validate_audio_route_config(&invalid_ceiling).is_err());
    }
}
