use std::{
    fs::File,
    io,
    path::Path,
    sync::atomic::{AtomicBool, AtomicU64},
};

use crossbeam_channel::Sender;
use symphonia::core::{
    audio::{Channels, SampleBuffer},
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
                options.output_sample_rate,
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
        options.output_channels,
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
                resampler = Some(
                    StreamingResampler::new(
                        source_rate,
                        options.output_sample_rate,
                        options.output_channels,
                    )
                    .map_err(AudioImportError::Resample)?,
                );
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
        let mut mono = if options.output_channels == 2 {
            stereo_interleaved(
                converted.samples(),
                spec.channels,
                &options.recognition_channels,
            )
        } else {
            downmix_interleaved(converted.samples(), channels, &options.recognition_channels)
        };
        if let Some(gate) = &mut input_gate {
            gate.process(&mut mono);
        }
        decoded_source_frames += (mono.len() / options.output_channels) as u64;
        let output = resampler
            .as_mut()
            .expect("resampler is initialized with the source format")
            .push(&mono)
            .map_err(AudioImportError::Resample)?;
        sink.push(&output)?;

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
    let tail = resampler
        .as_mut()
        .expect("resampler exists for a decoded stream")
        .finish()
        .map_err(AudioImportError::Resample)?;
    sink.push(&tail)?;
    sink.finish()?;
    send_progress(
        event_tx,
        decoded_source_frames,
        total_source_frames,
        source_rate,
    );
    Ok(sink.sent_frames)
}

/// Keep stereo cues until source separation. Selected mono/center tracks are
/// duplicated; multi-channel tracks retain their left/right balance.
fn stereo_interleaved(samples: &[f32], layout: Channels, selected: &[usize]) -> Vec<f32> {
    let channels = layout.count();
    if channels == 0 {
        return Vec::new();
    }
    let active = layout
        .iter()
        .enumerate()
        .filter(|(index, _)| selected.is_empty() || selected.contains(index))
        .collect::<Vec<_>>();
    if let [(index, _)] = active.as_slice() {
        return samples
            .chunks_exact(channels)
            .flat_map(|frame| [frame[*index]; 2])
            .collect();
    }
    let left_channels = Channels::FRONT_LEFT
        | Channels::REAR_LEFT
        | Channels::SIDE_LEFT
        | Channels::FRONT_LEFT_CENTRE
        | Channels::TOP_FRONT_LEFT
        | Channels::TOP_REAR_LEFT
        | Channels::REAR_LEFT_CENTRE
        | Channels::FRONT_LEFT_WIDE
        | Channels::FRONT_LEFT_HIGH;
    let right_channels = Channels::FRONT_RIGHT
        | Channels::REAR_RIGHT
        | Channels::SIDE_RIGHT
        | Channels::FRONT_RIGHT_CENTRE
        | Channels::TOP_FRONT_RIGHT
        | Channels::TOP_REAR_RIGHT
        | Channels::REAR_RIGHT_CENTRE
        | Channels::FRONT_RIGHT_WIDE
        | Channels::FRONT_RIGHT_HIGH;
    let weights = active
        .into_iter()
        .filter(|(_, channel)| !channel.intersects(Channels::LFE1 | Channels::LFE2))
        .map(|(index, channel)| {
            let (left, right) = if channel.intersects(left_channels) {
                (1.0, 0.0)
            } else if channel.intersects(right_channels) {
                (0.0, 1.0)
            } else {
                (
                    std::f32::consts::FRAC_1_SQRT_2,
                    std::f32::consts::FRAC_1_SQRT_2,
                )
            };
            (index, left, right)
        })
        .collect::<Vec<_>>();
    // One gain for both sides preserves their balance with asymmetric selections.
    let scale = weights
        .iter()
        .map(|(_, left, _)| left)
        .sum::<f32>()
        .max(weights.iter().map(|(_, _, right)| right).sum::<f32>())
        .max(1.0);
    samples
        .chunks_exact(channels)
        .flat_map(|frame| {
            let left = weights
                .iter()
                .map(|&(index, weight, _)| frame[index] * weight)
                .sum::<f32>();
            let right = weights
                .iter()
                .map(|&(index, _, weight)| frame[index] * weight)
                .sum::<f32>();
            [left / scale, right / scale]
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_import::AudioImportPacing;
    use std::{io::Write, sync::atomic::Ordering, time::Duration};

    #[test]
    fn surround_layout_keeps_centres_and_left_right_positions() {
        let stereo = Channels::FRONT_LEFT | Channels::FRONT_RIGHT;
        let rear = Channels::REAR_LEFT | Channels::REAR_RIGHT;
        let quad = stereo | rear;
        assert_eq!(
            stereo_interleaved(&[0.0, 0.0, 1.0, 0.0], quad, &[]),
            [0.5, 0.0]
        );
        assert_eq!(
            stereo_interleaved(&[0.0, 0.0, 0.0, 1.0], quad, &[]),
            [0.0, 0.5]
        );
        for layout in [
            stereo | Channels::FRONT_CENTRE | Channels::REAR_CENTRE,
            stereo | Channels::FRONT_CENTRE | rear,
            stereo | Channels::FRONT_CENTRE | Channels::LFE1 | rear,
        ] {
            let mut samples = vec![0.0; layout.count()];
            let centre = layout
                .iter()
                .position(|channel| channel == Channels::FRONT_CENTRE)
                .unwrap();
            samples[centre] = 0.6;
            let output = stereo_interleaved(&samples, layout, &[]);
            assert!(output[0] > 0.0);
            assert_eq!(output[0], output[1]);
            assert_eq!(stereo_interleaved(&samples, layout, &[centre]), [0.6, 0.6]);
            if layout.contains(rear) {
                samples[centre] = 0.0;
                let left = layout
                    .iter()
                    .position(|channel| channel == Channels::REAR_LEFT)
                    .unwrap();
                let right = layout
                    .iter()
                    .position(|channel| channel == Channels::REAR_RIGHT)
                    .unwrap();
                samples[left] = 0.4;
                samples[right] = -0.2;
                assert_eq!(
                    stereo_interleaved(&samples, layout, &[left, right]),
                    [0.4, -0.2]
                );
                assert_eq!(stereo_interleaved(&samples, layout, &[left]), [0.4, 0.4]);
                assert_eq!(stereo_interleaved(&samples, layout, &[0, left]), [0.2, 0.0]);
            }
        }
        assert_eq!(
            stereo_interleaved(&[0.2, -0.4], Channels::FRONT_LEFT, &[]),
            [0.2, 0.2, -0.4, -0.4]
        );
        let surround = stereo | Channels::FRONT_CENTRE | Channels::LFE1 | rear;
        let lfe = [0.0, 0.0, 0.0, 0.75, 0.0, 0.0];
        assert_eq!(stereo_interleaved(&lfe, surround, &[]), [0.0, 0.0]);
        assert_eq!(stereo_interleaved(&lfe, surround, &[3]), [0.75, 0.75]);
    }

    #[test]
    fn stereo_file_import_preserves_channels_duration_and_frame_progress() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("stereo.wav");
        let source_rate = 48_000_u32;
        let frames = 4_097_u32;
        write_stereo_test_wav(&path, source_rate, frames);

        let (audio_tx, audio_rx) = crossbeam_channel::bounded(64);
        let (event_tx, event_rx) = crossbeam_channel::unbounded();
        let sent = AtomicU64::new(0);
        let output_frames = run_import(
            &path,
            audio_tx,
            AudioImportOptions {
                chunk_frames: 127,
                output_sample_rate: 44_100,
                output_channels: 2,
                pacing: AudioImportPacing::AsFastAsPossible,
                recognition_channels: Vec::new(),
                gate_threshold_db: None,
            },
            &AtomicBool::new(false),
            &sent,
            &event_tx,
        )
        .unwrap();
        let expected_frames = (u64::from(frames) * 44_100).div_ceil(u64::from(source_rate));
        let samples = audio_rx.try_iter().flatten().collect::<Vec<_>>();
        assert_eq!(output_frames, expected_frames);
        assert_eq!(sent.load(Ordering::Acquire), expected_frames);
        assert_eq!(samples.len() as u64, expected_frames * 2);
        assert!(samples.iter().any(|sample| sample.abs() > 0.1));
        assert!(
            samples
                .chunks_exact(2)
                .all(|frame| (frame[0] + frame[1]).abs() < 1e-5)
        );
        let events = event_rx.try_iter().collect::<Vec<_>>();
        assert!(matches!(&events[0], AudioImportEvent::Started(info)
            if info.source_channels == 2 && info.source_sample_rate == source_rate
                && info.output_sample_rate == 44_100));
        let progress = events
            .iter()
            .rev()
            .find_map(|event| match event {
                AudioImportEvent::Progress(progress) => Some(progress),
                _ => None,
            })
            .unwrap();
        assert_eq!(progress.decoded_source_frames, u64::from(frames));
        assert_eq!(
            progress.position,
            Duration::from_secs_f64(f64::from(frames) / f64::from(source_rate))
        );
        assert_eq!(progress.duration, Some(progress.position));
        assert_eq!(progress.fraction, Some(1.0));
    }

    fn write_stereo_test_wav(path: &Path, source_rate: u32, frames: u32) {
        let data_bytes = frames * 4;
        let mut file = File::create(path).unwrap();
        file.write_all(b"RIFF").unwrap();
        file.write_all(&(36 + data_bytes).to_le_bytes()).unwrap();
        file.write_all(b"WAVEfmt ").unwrap();
        file.write_all(&16_u32.to_le_bytes()).unwrap();
        file.write_all(&1_u16.to_le_bytes()).unwrap();
        file.write_all(&2_u16.to_le_bytes()).unwrap();
        file.write_all(&source_rate.to_le_bytes()).unwrap();
        file.write_all(&(source_rate * 4).to_le_bytes()).unwrap();
        file.write_all(&4_u16.to_le_bytes()).unwrap();
        file.write_all(&16_u16.to_le_bytes()).unwrap();
        file.write_all(b"data").unwrap();
        file.write_all(&data_bytes.to_le_bytes()).unwrap();
        for index in 0..frames {
            let sample = ((index as f32 * 0.05).sin() * 10_000.0) as i16;
            file.write_all(&sample.to_le_bytes()).unwrap();
            file.write_all(&(-sample).to_le_bytes()).unwrap();
        }
    }

    #[cfg(feature = "mpv")]
    #[test]
    #[ignore = "requires the installed native MPV playback component"]
    fn mpv_extraction_preserves_stereo_and_media_timing() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("stereo.wav");
        let extracted = directory.path().join("extracted.wav");
        write_stereo_test_wav(&source, 48_000, 4_800);

        let (audio_tx, audio_rx) = crossbeam_channel::bounded(64);
        let (event_tx, event_rx) = crossbeam_channel::unbounded();
        let sent = AtomicU64::new(0);
        let cancelled = AtomicBool::new(false);
        let (output_frames, samples) = std::thread::scope(|scope| {
            let (done_tx, done_rx) = std::sync::mpsc::channel();
            let stop = &cancelled;
            scope.spawn(move || {
                if done_rx.recv_timeout(Duration::from_secs(10)).is_err() {
                    stop.store(true, Ordering::Release);
                }
            });
            let reader = scope.spawn(move || audio_rx.iter().flatten().collect::<Vec<_>>());
            let result = run_mpv_extract(&source, &extracted, 44_100, &cancelled, &event_tx)
                .and_then(|()| {
                    run_symphonia_import(
                        &source,
                        &extracted,
                        &audio_tx,
                        &AudioImportOptions {
                            chunk_frames: 127,
                            output_sample_rate: 44_100,
                            output_channels: 2,
                            pacing: AudioImportPacing::AsFastAsPossible,
                            recognition_channels: Vec::new(),
                            gate_threshold_db: None,
                        },
                        &cancelled,
                        &sent,
                        &event_tx,
                    )
                });
            drop(audio_tx);
            let _ = done_tx.send(());
            (result.unwrap(), reader.join().unwrap())
        });
        // MPV's PCM encoder can pad its last 16,384-frame block with silence.
        // The original signal must retain its full 100 ms and stereo phase.
        assert!((4_410..4_410 + 16_384).contains(&output_frames));
        assert_eq!(sent.load(Ordering::Acquire), output_frames);
        assert_eq!(samples.len() as u64, output_frames * 2);
        assert!(
            samples[8_600..8_820]
                .iter()
                .any(|sample| sample.abs() > 0.1)
        );
        assert!(samples[8_820..].iter().all(|sample| sample.abs() < 1e-4));
        assert!(
            samples
                .chunks_exact(2)
                .all(|frame| (frame[0] + frame[1]).abs() < 1e-4)
        );
        let events = event_rx.try_iter().collect::<Vec<_>>();
        assert!(
            events
                .iter()
                .any(|event| matches!(event, AudioImportEvent::Started(info)
            if info.source_channels == 2 && info.source_sample_rate == 44_100
                && info.output_sample_rate == 44_100 && info.path == source))
        );
        let progress = events
            .iter()
            .rev()
            .find_map(|event| match event {
                AudioImportEvent::Progress(progress) => Some(progress),
                _ => None,
            })
            .unwrap();
        assert_eq!(progress.decoded_source_frames, output_frames);
        assert_eq!(
            progress.position,
            Duration::from_secs_f64(output_frames as f64 / 44_100.0)
        );
        assert_eq!(progress.duration, Some(progress.position));
        assert_eq!(progress.fraction, Some(1.0));
    }
}
