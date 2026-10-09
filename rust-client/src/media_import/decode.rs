use std::{
    fs::File,
    io,
    path::Path,
    sync::atomic::{AtomicBool, AtomicU64},
};

use crossbeam_channel::Sender;
use symphonia::core::{
    audio::SampleBuffer,
    codecs::{CODEC_TYPE_NULL, DecoderOptions},
    errors::Error as SymphoniaError,
    formats::FormatOptions,
    io::MediaSourceStream,
    meta::MetadataOptions,
    probe::Hint,
};

use super::{
    mpv_extract::{TempFileGuard, run_mpv_extract},
    stream::{ChunkSink, StreamingResampler, check_cancelled, duration_from_frames},
    types::{
        AudioFileInfo, AudioImportError, AudioImportEvent, AudioImportOptions, AudioImportProgress,
        AudioImportStage, IMPORT_SAMPLE_RATE,
    },
};

const PROGRESS_AUDIO_INTERVAL_FRAMES: u64 = 5 * IMPORT_SAMPLE_RATE as u64;

pub(super) fn run_import(
    path: &Path,
    audio_tx: Sender<Vec<f32>>,
    options: AudioImportOptions,
    stop_requested: &AtomicBool,
    sent_frames: &AtomicU64,
    event_tx: &Sender<AudioImportEvent>,
) -> Result<u64, AudioImportError> {
    match run_symphonia_import(
        path,
        path,
        &audio_tx,
        &options,
        stop_requested,
        sent_frames,
        event_tx,
    ) {
        Ok(frames) => Ok(frames),
        Err(err @ AudioImportError::Unsupported(_)) | Err(err @ AudioImportError::Decode(_)) => {
            log::info!(
                "Symphonia cannot decode media file ({err}), falling back to MPV audio decoder for {path:?}"
            );
            let temp_id = uuid::Uuid::new_v4();
            let temp_wav =
                std::path::PathBuf::from(format!("runtime/cache/mpv_decode_{temp_id}.wav"));
            let _guard = TempFileGuard(temp_wav.clone());
            run_mpv_extract(
                path,
                &temp_wav,
                &options.recognition_channels,
                stop_requested,
                event_tx,
            )?;
            run_symphonia_import(
                path,
                &temp_wav,
                &audio_tx,
                &options,
                stop_requested,
                sent_frames,
                event_tx,
            )
        }
        Err(err) => Err(err),
    }
}

fn run_symphonia_import(
    original_path: &Path,
    decode_path: &Path,
    audio_tx: &Sender<Vec<f32>>,
    options: &AudioImportOptions,
    stop_requested: &AtomicBool,
    sent_frames: &AtomicU64,
    event_tx: &Sender<AudioImportEvent>,
) -> Result<u64, AudioImportError> {
    let file = File::open(decode_path)?;
    let media_source = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(extension) = decode_path
        .extension()
        .and_then(|extension| extension.to_str())
    {
        hint.with_extension(extension);
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            media_source,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(map_probe_error)?;
    let mut format = probed.format;
    let (track_id, codec_params, mut decoder) = format
        .tracks()
        .iter()
        .find_map(|t| {
            if t.codec_params.codec == CODEC_TYPE_NULL {
                return None;
            }
            let decoder = symphonia::default::get_codecs()
                .make(&t.codec_params, &DecoderOptions::default())
                .ok()?;
            Some((t.id, t.codec_params.clone(), decoder))
        })
        .ok_or_else(|| {
            AudioImportError::Unsupported("no decodable audio track found in media file".into())
        })?;

    let codec_name = format!("{:?}", codec_params.codec);
    let total_source_frames = codec_params.n_frames;

    let mut source_format = None;
    let mut input_gate = None;
    let mut resampler = None;
    let mut sink = ChunkSink::new(
        audio_tx.clone(),
        options.chunk_frames,
        options.output_sample_rate,
        options.pacing,
        stop_requested,
        sent_frames,
    );
    let mut decoded_source_frames = 0_u64;
    let mut next_progress_frame = 0_u64;

    loop {
        check_cancelled(stop_requested)?;
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(error)) if error.kind() == io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(SymphoniaError::ResetRequired) => {
                return Err(AudioImportError::Decode(
                    "mid-stream format reset is not supported".into(),
                ));
            }
            Err(error) => return Err(map_decode_error(error)),
        };
        if packet.track_id() != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            // A damaged packet need not make the rest of a long meeting unusable.
            Err(SymphoniaError::DecodeError(message)) => {
                log::warn!("Skipping damaged imported-audio packet: {message}");
                continue;
            }
            Err(error) => return Err(map_decode_error(error)),
        };
        let spec = *decoded.spec();
        let source_rate = spec.rate;
        let channels = spec.channels.count();
        if channels == 0 || source_rate == 0 {
            return Err(AudioImportError::Decode(
                "invalid decoded audio format".into(),
            ));
        }

        match source_format {
            None => {
                source_format = Some((source_rate, channels));
                input_gate = options
                    .gate_threshold_db
                    .map(|db| crate::audio_processing::NoiseGate::new(db, source_rate));
                resampler = Some(StreamingResampler::new(
                    source_rate,
                    options.output_sample_rate,
                )?);
                let duration = duration_from_frames(total_source_frames, source_rate);
                let _ = event_tx.send(AudioImportEvent::Started(AudioFileInfo {
                    path: original_path.to_path_buf(),
                    codec: codec_name.clone(),
                    source_sample_rate: source_rate,
                    source_channels: channels,
                    total_source_frames,
                    duration,
                    output_sample_rate: options.output_sample_rate,
                }));
            }
            Some((expected_rate, expected_channels))
                if expected_rate != source_rate || expected_channels != channels =>
            {
                return Err(AudioImportError::Decode(format!(
                    "audio format changed from {expected_rate} Hz/{expected_channels} channels to {source_rate} Hz/{channels} channels"
                )));
            }
            _ => {}
        }

        let mut converted = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        converted.copy_interleaved_ref(decoded);
        let mut mono =
            downmix_interleaved(converted.samples(), channels, &options.recognition_channels);
        if let Some(gate) = &mut input_gate {
            gate.process(&mut mono);
        }
        decoded_source_frames += mono.len() as u64;
        resampler
            .as_mut()
            .expect("resampler is initialized with the source format")
            .push(&mono, &mut sink)?;

        if decoded_source_frames >= next_progress_frame {
            let (source_rate, _) = source_format.expect("source format is initialized");
            send_progress(
                event_tx,
                decoded_source_frames,
                total_source_frames,
                source_rate,
            );
            next_progress_frame = decoded_source_frames
                + (PROGRESS_AUDIO_INTERVAL_FRAMES * source_rate as u64 / IMPORT_SAMPLE_RATE as u64)
                    .max(1);
        }
    }

    let Some((source_rate, _)) = source_format else {
        return Err(AudioImportError::Decode(
            "the selected track contained no audio frames".into(),
        ));
    };
    resampler
        .as_mut()
        .expect("resampler exists for a decoded stream")
        .finish(&mut sink)?;
    sink.finish()?;
    send_progress(
        event_tx,
        decoded_source_frames,
        total_source_frames,
        source_rate,
    );
    Ok(sink.sent_frames)
}

fn downmix_interleaved(
    samples: &[f32],
    channels: usize,
    recognition_channels: &[usize],
) -> Vec<f32> {
    if channels == 1 {
        return samples.to_vec();
    }

    let active_channels: Vec<usize> = recognition_channels
        .iter()
        .copied()
        .filter(|&idx| idx < channels)
        .collect();

    if !active_channels.is_empty() {
        if active_channels.len() == 1 {
            let single_idx = active_channels[0];
            return samples
                .chunks_exact(channels)
                .map(|frame| frame[single_idx])
                .collect();
        }

        let scale = 1.0 / active_channels.len() as f32;
        return samples
            .chunks_exact(channels)
            .map(|frame| {
                let sum: f32 = active_channels.iter().map(|&idx| frame[idx]).sum();
                sum * scale
            })
            .collect();
    } else if channels >= 6 {
        // Standard SMPTE / WAVE 5.1/7.1 order: 0:FL, 1:FR, 2:FC, 3:LFE, 4:SL/BL, 5:SR/BR
        // Prioritize Center dialogue while attenuating surround and ignoring LFE
        samples
            .chunks_exact(channels)
            .map(|frame| {
                let l = frame[0];
                let r = frame[1];
                let c = frame[2];
                let ls = frame[4];
                let rs = frame[5];
                c * 0.85 + (l + r) * 0.12 + (ls + rs) * 0.03
            })
            .collect()
    } else if channels == 2 {
        samples
            .chunks_exact(2)
            .map(|frame| (frame[0] + frame[1]) * 0.5)
            .collect()
    } else {
        let scale = 1.0 / channels as f32;
        samples
            .chunks_exact(channels)
            .map(|frame| frame.iter().copied().sum::<f32>() * scale)
            .collect()
    }
}

fn send_progress(
    event_tx: &Sender<AudioImportEvent>,
    decoded_source_frames: u64,
    total_source_frames: Option<u64>,
    source_rate: u32,
) {
    let position =
        duration_from_frames(Some(decoded_source_frames), source_rate).unwrap_or_default();
    let duration = duration_from_frames(total_source_frames, source_rate);
    let fraction = total_source_frames
        .filter(|total| *total > 0)
        .map(|total| (decoded_source_frames as f64 / total as f64).clamp(0.0, 1.0) as f32);
    let _ = event_tx.send(AudioImportEvent::Progress(AudioImportProgress {
        stage: AudioImportStage::Recognizing,
        decoded_source_frames,
        total_source_frames,
        position,
        duration,
        fraction,
    }));
}

fn map_probe_error(error: SymphoniaError) -> AudioImportError {
    match error {
        SymphoniaError::IoError(error) => AudioImportError::Io(error),
        SymphoniaError::Unsupported(message) => AudioImportError::Unsupported(message.into()),
        other => AudioImportError::Unsupported(other.to_string()),
    }
}

fn map_decode_error(error: SymphoniaError) -> AudioImportError {
    match error {
        SymphoniaError::IoError(error) => AudioImportError::Io(error),
        SymphoniaError::Unsupported(message) => AudioImportError::Unsupported(message.into()),
        other => AudioImportError::Decode(other.to_string()),
    }
}
