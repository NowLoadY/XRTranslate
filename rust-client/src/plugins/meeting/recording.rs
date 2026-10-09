//! Crash-recoverable, non-blocking live-meeting recording.
//!
//! Capture callbacks only call [`RecordingSink::try_append`]. Sample conversion,
//! filesystem writes, flushing, and WAV finalization all happen on a dedicated
//! writer thread. The on-disk `.pcm.part` files are headerless mono signed
//! 16-bit little-endian PCM at 16 kHz, which makes an interrupted recording
//! straightforward to inspect, append to, or finalize after a restart.

use crossbeam_channel::{Sender, bounded};
use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    thread::{self, JoinHandle},
};

pub const RECORDING_SAMPLE_RATE: u32 = 16_000;
pub const RECORDING_CHANNELS: u16 = 1;
pub const RECORDING_BITS_PER_SAMPLE: u16 = 16;

const MICROPHONE_PART_FILE: &str = "microphone.pcm.part";
const MICROPHONE_WAV_FILE: &str = "microphone.wav";
const SYSTEM_AUDIO_PART_FILE: &str = "system-audio.pcm.part";
const SYSTEM_AUDIO_WAV_FILE: &str = "system-audio.wav";

pub type Result<T> = std::result::Result<T, RecordingError>;

#[derive(Debug)]
pub enum RecordingError {
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    InvalidQueueCapacity,
    InvalidPartFile {
        path: PathBuf,
        reason: &'static str,
    },
    WavTooLarge {
        path: PathBuf,
        pcm_bytes: u64,
    },
    FinalRecordingExists(PathBuf),
    NoRecoverableRecording(PathBuf),
    WriterClosed,
    WriterPanicked,
}

impl fmt::Display for RecordingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io {
                operation,
                path,
                source,
            } => write!(f, "failed to {operation} {}: {source}", path.display()),
            Self::InvalidQueueCapacity => f.write_str("recording queue capacity must be non-zero"),
            Self::InvalidPartFile { path, reason } => {
                write!(
                    f,
                    "invalid recording part file {}: {reason}",
                    path.display()
                )
            }
            Self::WavTooLarge { path, pcm_bytes } => write!(
                f,
                "recording {} is too large for a standard WAV file ({pcm_bytes} PCM bytes)",
                path.display()
            ),
            Self::FinalRecordingExists(path) => {
                write!(f, "final recording already exists: {}", path.display())
            }
            Self::NoRecoverableRecording(path) => {
                write!(f, "no recoverable recording found in {}", path.display())
            }
            Self::WriterClosed => f.write_str("recording writer is closed"),
            Self::WriterPanicked => f.write_str("recording writer thread panicked"),
        }
    }
}

impl std::error::Error for RecordingError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingTrack {
    Microphone,
    SystemAudio,
}

impl RecordingTrack {
    const fn part_file_name(self) -> &'static str {
        match self {
            Self::Microphone => MICROPHONE_PART_FILE,
            Self::SystemAudio => SYSTEM_AUDIO_PART_FILE,
        }
    }

    const fn wav_file_name(self) -> &'static str {
        match self {
            Self::Microphone => MICROPHONE_WAV_FILE,
            Self::SystemAudio => SYSTEM_AUDIO_WAV_FILE,
        }
    }
}

/// A normalized 16 kHz mono chunk waiting to be persisted.
#[derive(Debug)]
pub struct RecordingChunk {
    pub track: RecordingTrack,
    pub samples: Vec<f32>,
}

/// A failed non-blocking enqueue. The original chunk is returned to the caller.
#[derive(Debug)]
#[allow(dead_code)]
pub struct TryAppendError {
    pub kind: TryAppendErrorKind,
    pub chunk: RecordingChunk,
}

impl fmt::Display for TryAppendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            TryAppendErrorKind::QueueFull => f.write_str("recording queue is full"),
            TryAppendErrorKind::WriterClosed => f.write_str("recording writer is closed"),
        }
    }
}

impl std::error::Error for TryAppendError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TryAppendErrorKind {
    QueueFull,
    WriterClosed,
}

#[derive(Debug, Clone)]
pub struct RecordingConfig {
    /// A meeting-specific directory. Track file names within it are stable.
    pub directory: PathBuf,
    /// Number of chunks accepted ahead of the disk writer.
    pub queue_capacity: usize,
}

impl RecordingConfig {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
            queue_capacity: 128,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackCheckpoint {
    pub track: RecordingTrack,
    pub part_path: PathBuf,
    pub samples: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordingCheckpoint {
    pub microphone: TrackCheckpoint,
    pub system_audio: TrackCheckpoint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalizedRecording {
    pub microphone_wav: PathBuf,
    pub system_audio_wav: PathBuf,
    pub microphone_samples: u64,
    pub system_audio_samples: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredTrack {
    pub track: RecordingTrack,
    pub part_path: PathBuf,
    pub wav_path: PathBuf,
    pub samples: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoverableRecording {
    pub directory: PathBuf,
    pub tracks: Vec<RecoveredTrack>,
}

/// Cloneable handle intended for audio callbacks.
///
/// `try_append` never waits for the writer and never touches the filesystem.
#[derive(Clone)]
pub struct RecordingSink {
    sender: Sender<WriterCommand>,
}

impl RecordingSink {
    pub fn try_append(
        &self,
        track: RecordingTrack,
        samples: Vec<f32>,
    ) -> std::result::Result<(), TryAppendError> {
        let chunk = RecordingChunk { track, samples };
        match self.sender.try_send(WriterCommand::Samples(chunk)) {
            Ok(()) => Ok(()),
            Err(crossbeam_channel::TrySendError::Full(WriterCommand::Samples(chunk))) => {
                Err(TryAppendError {
                    kind: TryAppendErrorKind::QueueFull,
                    chunk,
                })
            }
            Err(crossbeam_channel::TrySendError::Disconnected(WriterCommand::Samples(chunk))) => {
                Err(TryAppendError {
                    kind: TryAppendErrorKind::WriterClosed,
                    chunk,
                })
            }
            Err(_) => unreachable!("only sample commands are sent through try_append"),
        }
    }
}

/// Owns one background writer and the lifecycle of a live recording.
pub struct MeetingRecording {
    sink: RecordingSink,
    worker: Option<JoinHandle<()>>,
}

impl MeetingRecording {
    /// Starts a new recording or resumes existing `.pcm.part` files.
    ///
    /// If a final WAV already exists, recovery/finalization must be resolved
    /// first so an old recording is never overwritten accidentally.
    pub fn start(config: RecordingConfig) -> Result<Self> {
        if config.queue_capacity == 0 {
            return Err(RecordingError::InvalidQueueCapacity);
        }
        create_dir_all(&config.directory)?;
        let paths = RecordingPaths::new(config.directory);
        paths.ensure_final_files_absent()?;
        let writers = TrackWriters::open(&paths)?;
        let (sender, receiver) = bounded(config.queue_capacity);
        let worker = thread::Builder::new()
            .name("meeting-recording-writer".to_owned())
            .spawn(move || writer_loop(receiver, writers, paths))
            .map_err(|source| RecordingError::Io {
                operation: "spawn recording writer",
                path: PathBuf::from("meeting-recording-writer"),
                source,
            })?;
        Ok(Self {
            sink: RecordingSink { sender },
            worker: Some(worker),
        })
    }

    pub fn sink(&self) -> RecordingSink {
        self.sink.clone()
    }

    /// Flushes both tracks and calls `sync_data`, creating a durable checkpoint.
    /// This is a control-thread operation and may block behind queued audio.
    pub fn checkpoint(&self) -> Result<RecordingCheckpoint> {
        let (response_tx, response_rx) = bounded(1);
        self.sink
            .sender
            .send(WriterCommand::Checkpoint(response_tx))
            .map_err(|_| RecordingError::WriterClosed)?;
        response_rx
            .recv()
            .map_err(|_| RecordingError::WriterClosed)?
    }

    /// Flushes both tracks and leaves their `.pcm.part` files recoverable.
    pub fn stop_without_finalizing(mut self) -> Result<RecordingCheckpoint> {
        let (response_tx, response_rx) = bounded(1);
        self.sink
            .sender
            .send(WriterCommand::Stop(response_tx))
            .map_err(|_| RecordingError::WriterClosed)?;
        let result = response_rx
            .recv()
            .map_err(|_| RecordingError::WriterClosed)?;
        self.join_worker()?;
        result
    }

    /// Atomically publishes both WAV tracks. Part files are removed only after
    /// their corresponding WAV has been durably written and renamed.
    pub fn finalize(mut self) -> Result<FinalizedRecording> {
        let (response_tx, response_rx) = bounded(1);
        self.sink
            .sender
            .send(WriterCommand::Finalize(response_tx))
            .map_err(|_| RecordingError::WriterClosed)?;
        let result = response_rx
            .recv()
            .map_err(|_| RecordingError::WriterClosed)?;
        self.join_worker()?;
        result
    }

    fn join_worker(&mut self) -> Result<()> {
        if let Some(worker) = self.worker.take() {
            worker.join().map_err(|_| RecordingError::WriterPanicked)?;
        }
        Ok(())
    }
}

impl Drop for MeetingRecording {
    fn drop(&mut self) {
        let Some(worker) = self.worker.take() else {
            return;
        };
        // Drop is not part of the capture callback. Waiting here guarantees a
        // controlled shutdown leaves all already-accepted chunks recoverable.
        let (response_tx, response_rx) = bounded(1);
        if self
            .sink
            .sender
            .send(WriterCommand::Stop(response_tx))
            .is_ok()
        {
            let _ = response_rx.recv();
        }
        let _ = worker.join();
    }
}

/// Inspects incomplete tracks without modifying them.
pub fn inspect_recoverable_recording(
    directory: impl AsRef<Path>,
) -> Result<Option<RecoverableRecording>> {
    let paths = RecordingPaths::new(directory.as_ref().to_path_buf());
    let mut tracks = Vec::with_capacity(2);
    for track in [RecordingTrack::Microphone, RecordingTrack::SystemAudio] {
        let part_path = paths.part(track);
        if part_path.exists() {
            tracks.push(RecoveredTrack {
                track,
                samples: part_sample_count(&part_path)?,
                wav_path: paths.wav(track),
                part_path,
            });
        }
    }
    if tracks.is_empty() {
        Ok(None)
    } else {
        Ok(Some(RecoverableRecording {
            directory: paths.directory,
            tracks,
        }))
    }
}

/// Finalizes part files left by an interrupted process. This operation is
/// idempotent if a crash occurred after the WAV rename but before part cleanup.
pub fn finalize_recovered_recording(directory: impl AsRef<Path>) -> Result<FinalizedRecording> {
    let paths = RecordingPaths::new(directory.as_ref().to_path_buf());
    let recovery = inspect_recoverable_recording(&paths.directory)?
        .ok_or_else(|| RecordingError::NoRecoverableRecording(paths.directory.clone()))?;

    let mut microphone_samples =
        wav_sample_count_if_present(&paths.wav(RecordingTrack::Microphone))?.unwrap_or(0);
    let mut system_audio_samples =
        wav_sample_count_if_present(&paths.wav(RecordingTrack::SystemAudio))?.unwrap_or(0);

    for recovered in recovery.tracks {
        let samples = finalize_part(&recovered.part_path, &recovered.wav_path)?;
        match recovered.track {
            RecordingTrack::Microphone => microphone_samples = samples,
            RecordingTrack::SystemAudio => system_audio_samples = samples,
        }
    }

    Ok(FinalizedRecording {
        microphone_wav: paths.wav(RecordingTrack::Microphone),
        system_audio_wav: paths.wav(RecordingTrack::SystemAudio),
        microphone_samples,
        system_audio_samples,
    })
}

enum WriterCommand {
    Samples(RecordingChunk),
    Checkpoint(Sender<Result<RecordingCheckpoint>>),
    Stop(Sender<Result<RecordingCheckpoint>>),
    Finalize(Sender<Result<FinalizedRecording>>),
}

fn writer_loop(
    receiver: crossbeam_channel::Receiver<WriterCommand>,
    mut writers: TrackWriters,
    paths: RecordingPaths,
) {
    while let Ok(command) = receiver.recv() {
        match command {
            WriterCommand::Samples(chunk) => {
                if writers.write_chunk(chunk).is_err() {
                    // Disconnect all producers immediately. The next enqueue and
                    // every lifecycle operation will report WriterClosed.
                    return;
                }
            }
            WriterCommand::Checkpoint(response) => {
                let _ = response.send(writers.checkpoint(&paths));
            }
            WriterCommand::Stop(response) => {
                let _ = response.send(writers.checkpoint(&paths));
                return;
            }
            WriterCommand::Finalize(response) => {
                let result = writers.checkpoint(&paths).and_then(|checkpoint| {
                    drop(writers);
                    finalize_paths(paths, checkpoint)
                });
                let _ = response.send(result);
                return;
            }
        }
    }
    let _ = writers.flush_and_sync();
}

struct RecordingPaths {
    directory: PathBuf,
}

impl RecordingPaths {
    fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    fn part(&self, track: RecordingTrack) -> PathBuf {
        self.directory.join(track.part_file_name())
    }

    fn wav(&self, track: RecordingTrack) -> PathBuf {
        self.directory.join(track.wav_file_name())
    }

    fn ensure_final_files_absent(&self) -> Result<()> {
        for track in [RecordingTrack::Microphone, RecordingTrack::SystemAudio] {
            let wav = self.wav(track);
            if wav.exists() {
                return Err(RecordingError::FinalRecordingExists(wav));
            }
        }
        Ok(())
    }
}

struct TrackWriter {
    path: PathBuf,
    writer: BufWriter<File>,
    samples: u64,
}

impl TrackWriter {
    fn open(path: PathBuf) -> Result<Self> {
        let samples = if path.exists() {
            part_sample_count(&path)?
        } else {
            0
        };
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|source| io_error("open recording part", &path, source))?;
        Ok(Self {
            path,
            writer: BufWriter::new(file),
            samples,
        })
    }

    fn write_samples(&mut self, samples: &[f32]) -> Result<()> {
        let mut pcm = Vec::with_capacity(samples.len() * 2);
        for &sample in samples {
            pcm.extend_from_slice(&float_to_pcm_i16(sample).to_le_bytes());
        }
        self.writer
            .write_all(&pcm)
            .map_err(|source| io_error("write recording part", &self.path, source))?;
        self.samples = self.samples.saturating_add(samples.len() as u64);
        Ok(())
    }

    fn flush_and_sync(&mut self) -> Result<()> {
        self.writer
            .flush()
            .map_err(|source| io_error("flush recording part", &self.path, source))?;
        self.writer
            .get_ref()
            .sync_data()
            .map_err(|source| io_error("sync recording part", &self.path, source))
    }
}

struct TrackWriters {
    microphone: TrackWriter,
    system_audio: TrackWriter,
}

impl TrackWriters {
    fn open(paths: &RecordingPaths) -> Result<Self> {
        Ok(Self {
            microphone: TrackWriter::open(paths.part(RecordingTrack::Microphone))?,
            system_audio: TrackWriter::open(paths.part(RecordingTrack::SystemAudio))?,
        })
    }

    fn write_chunk(&mut self, chunk: RecordingChunk) -> Result<()> {
        match chunk.track {
            RecordingTrack::Microphone => self.microphone.write_samples(&chunk.samples),
            RecordingTrack::SystemAudio => self.system_audio.write_samples(&chunk.samples),
        }
    }

    fn flush_and_sync(&mut self) -> Result<()> {
        self.microphone.flush_and_sync()?;
        self.system_audio.flush_and_sync()
    }

    fn checkpoint(&mut self, paths: &RecordingPaths) -> Result<RecordingCheckpoint> {
        self.flush_and_sync()?;
        Ok(RecordingCheckpoint {
            microphone: TrackCheckpoint {
                track: RecordingTrack::Microphone,
                part_path: paths.part(RecordingTrack::Microphone),
                samples: self.microphone.samples,
            },
            system_audio: TrackCheckpoint {
                track: RecordingTrack::SystemAudio,
                part_path: paths.part(RecordingTrack::SystemAudio),
                samples: self.system_audio.samples,
            },
        })
    }
}

fn finalize_paths(
    paths: RecordingPaths,
    checkpoint: RecordingCheckpoint,
) -> Result<FinalizedRecording> {
    let microphone_wav = paths.wav(RecordingTrack::Microphone);
    let system_audio_wav = paths.wav(RecordingTrack::SystemAudio);
    let microphone_samples = finalize_part(&checkpoint.microphone.part_path, &microphone_wav)?;
    let system_audio_samples =
        finalize_part(&checkpoint.system_audio.part_path, &system_audio_wav)?;
    Ok(FinalizedRecording {
        microphone_wav,
        system_audio_wav,
        microphone_samples,
        system_audio_samples,
    })
}

fn finalize_part(part_path: &Path, wav_path: &Path) -> Result<u64> {
    let pcm_bytes = validated_pcm_len(part_path)?;
    let samples = pcm_bytes / 2;

    if wav_path.exists() {
        if wav_sample_count_if_present(wav_path)? == Some(samples) {
            fs::remove_file(part_path)
                .map_err(|source| io_error("remove finalized recording part", part_path, source))?;
            return Ok(samples);
        }
        return Err(RecordingError::FinalRecordingExists(wav_path.to_path_buf()));
    }
    if pcm_bytes > u64::from(u32::MAX - 36) {
        return Err(RecordingError::WavTooLarge {
            path: part_path.to_path_buf(),
            pcm_bytes,
        });
    }
    let data_size = pcm_bytes as u32;
    let temporary_wav = wav_path.with_extension("wav.tmp");
    let write_result = write_wav_file(part_path, &temporary_wav, data_size);
    if let Err(error) = write_result {
        return Err(error);
    }
    fs::rename(&temporary_wav, wav_path)
        .map_err(|source| io_error("publish finalized WAV", wav_path, source))?;
    fs::remove_file(part_path)
        .map_err(|source| io_error("remove finalized recording part", part_path, source))?;
    Ok(samples)
}

fn write_wav_file(part_path: &Path, temporary_wav: &Path, data_size: u32) -> Result<()> {
    let input = File::open(part_path)
        .map_err(|source| io_error("open recording part", part_path, source))?;
    let output = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(temporary_wav)
        .map_err(|source| io_error("create temporary WAV", temporary_wav, source))?;
    let mut input = BufReader::new(input);
    let mut output = BufWriter::new(output);
    output
        .write_all(&wav_header(data_size))
        .and_then(|_| io::copy(&mut input, &mut output).map(|_| ()))
        .and_then(|_| output.flush())
        .map_err(|source| io_error("write temporary WAV", temporary_wav, source))?;
    output
        .get_ref()
        .sync_all()
        .map_err(|source| io_error("sync temporary WAV", temporary_wav, source))
}

fn wav_header(data_size: u32) -> [u8; 44] {
    let byte_rate = RECORDING_SAMPLE_RATE * u32::from(RECORDING_CHANNELS) * 2;
    let block_align = RECORDING_CHANNELS * 2;
    let riff_size = data_size.saturating_add(36);
    let mut header = [0_u8; 44];
    header[0..4].copy_from_slice(b"RIFF");
    header[4..8].copy_from_slice(&riff_size.to_le_bytes());
    header[8..12].copy_from_slice(b"WAVE");
    header[12..16].copy_from_slice(b"fmt ");
    header[16..20].copy_from_slice(&16_u32.to_le_bytes());
    header[20..22].copy_from_slice(&1_u16.to_le_bytes());
    header[22..24].copy_from_slice(&RECORDING_CHANNELS.to_le_bytes());
    header[24..28].copy_from_slice(&RECORDING_SAMPLE_RATE.to_le_bytes());
    header[28..32].copy_from_slice(&byte_rate.to_le_bytes());
    header[32..34].copy_from_slice(&block_align.to_le_bytes());
    header[34..36].copy_from_slice(&RECORDING_BITS_PER_SAMPLE.to_le_bytes());
    header[36..40].copy_from_slice(b"data");
    header[40..44].copy_from_slice(&data_size.to_le_bytes());
    header
}

fn wav_sample_count_if_present(path: &Path) -> Result<Option<u64>> {
    if !path.exists() {
        return Ok(None);
    }
    let mut file = File::open(path).map_err(|source| io_error("open WAV", path, source))?;
    let mut header = [0_u8; 44];
    file.read_exact(&mut header)
        .map_err(|source| io_error("read WAV header", path, source))?;
    if &header[0..4] != b"RIFF"
        || &header[8..12] != b"WAVE"
        || &header[36..40] != b"data"
        || u16::from_le_bytes([header[22], header[23]]) != RECORDING_CHANNELS
        || u32::from_le_bytes([header[24], header[25], header[26], header[27]])
            != RECORDING_SAMPLE_RATE
        || u16::from_le_bytes([header[34], header[35]]) != RECORDING_BITS_PER_SAMPLE
    {
        return Err(RecordingError::InvalidPartFile {
            path: path.to_path_buf(),
            reason: "existing WAV does not match the recording format",
        });
    }
    let data_size = u32::from_le_bytes([header[40], header[41], header[42], header[43]]) as u64;
    let actual_size = file
        .metadata()
        .map_err(|source| io_error("inspect WAV", path, source))?
        .len();
    if actual_size != data_size + 44 || data_size % 2 != 0 {
        return Err(RecordingError::InvalidPartFile {
            path: path.to_path_buf(),
            reason: "existing WAV has an invalid data length",
        });
    }
    Ok(Some(data_size / 2))
}

fn part_sample_count(path: &Path) -> Result<u64> {
    Ok(validated_pcm_len(path)? / 2)
}

fn validated_pcm_len(path: &Path) -> Result<u64> {
    let length = fs::metadata(path)
        .map_err(|source| io_error("inspect recording part", path, source))?
        .len();
    if length % 2 != 0 {
        return Err(RecordingError::InvalidPartFile {
            path: path.to_path_buf(),
            reason: "16-bit PCM byte length must be even",
        });
    }
    Ok(length)
}

fn float_to_pcm_i16(sample: f32) -> i16 {
    if !sample.is_finite() {
        return 0;
    }
    (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16
}

fn create_dir_all(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|source| io_error("create recording directory", path, source))
}

fn io_error(operation: &'static str, path: &Path, source: io::Error) -> RecordingError {
    RecordingError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    }
}
