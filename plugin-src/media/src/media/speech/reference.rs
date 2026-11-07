use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};

/// Read only this cue's separated voice; never borrow an unknown neighboring speaker.
pub(super) fn audio(task: &MediaTask, start_ms: i64, end_ms: i64) -> Result<String, String> {
    let [start_ms, end_ms] = task.speech_reference_range.unwrap_or([start_ms, end_ms]);
    if end_ms - start_ms < 5000 {
        return Err("Voice cloning needs at least 5 seconds of original speech. Use the selected voice for this short segment.".into());
    }
    let path = &task
        .separated_audio
        .as_ref()
        .ok_or("Separate the original vocals before cloning a voice.")?
        .vocals;
    let mut reader = hound::WavReader::open(path).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    if spec.channels == 0 || spec.sample_rate < 16_000 {
        return Err("The original vocal track is not suitable for voice cloning.".into());
    }
    let start = (start_ms.max(0) as u64 * u64::from(spec.sample_rate) / 1000)
        .min(u64::from(u32::MAX)) as u32;
    let frames =
        ((end_ms - start_ms).min(20_000) as u64 * u64::from(spec.sample_rate) / 1000) as usize;
    reader.seek(start).map_err(|e| e.to_string())?;
    let count = frames * usize::from(spec.channels);
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .take(count)
            .collect::<Result<_, _>>(),
        hound::SampleFormat::Int => {
            let scale = 2.0_f32.powi(i32::from(spec.bits_per_sample) - 1);
            reader
                .samples::<i32>()
                .take(count)
                .map(|sample| sample.map(|value| value as f32 / scale))
                .collect()
        }
    }
    .map_err(|e| e.to_string())?;
    let mono: Vec<_> = samples
        .chunks_exact(usize::from(spec.channels))
        .map(|frame| frame.iter().sum::<f32>() / f32::from(spec.channels))
        .collect();
    if mono.len() < spec.sample_rate as usize * 5
        || mono.iter().any(|sample| !sample.is_finite())
        || mono.iter().map(|sample| sample * sample).sum::<f32>() / (mono.len().max(1) as f32)
            < 0.00001
    {
        return Err("The reference needs at least 5 seconds of clear original speech.".into());
    }
    let mut resampler =
        xrtranslate_engine::audio::StreamingResampler::new(spec.sample_rate, 24_000, 1)?;
    let mut pcm = resampler.push(&mono)?;
    pcm.extend(resampler.finish()?);
    let mut wav = Cursor::new(Vec::new());
    let mut writer = hound::WavWriter::new(
        &mut wav,
        hound::WavSpec {
            sample_rate: 24_000,
            ..SPEC
        },
    )
    .map_err(|e| e.to_string())?;
    for sample in pcm {
        writer
            .write_sample((sample * 32768.0).round().clamp(-32768.0, 32767.0) as i16)
            .map_err(|e| e.to_string())?;
    }
    writer.finalize().map_err(|e| e.to_string())?;
    Ok(STANDARD.encode(wav.into_inner()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_reads_only_the_selected_stereo_region_and_resamples_it() {
        let dir = tempfile::tempdir().unwrap();
        let vocals = dir.path().join("vocals.wav");
        let mut writer = hound::WavWriter::create(
            &vocals,
            hound::WavSpec {
                channels: 2,
                sample_rate: 48_000,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .unwrap();
        for frame in 0..48_000 * 12 {
            let value = if frame < 48_000 * 6 {
                0.0
            } else {
                (frame as f32 * std::f32::consts::TAU * 400.0 / 48_000.0).sin() * 0.2
            };
            writer.write_sample(value).unwrap();
            writer.write_sample(value).unwrap();
        }
        writer.finalize().unwrap();
        let mut task = MediaTask::new(
            "reference".into(),
            crate::media::backend::MediaSource::LocalFile(vocals.clone()),
            "en".into(),
            "zh".into(),
            crate::media::task::SubtitleMode::None,
            Default::default(),
        );
        task.separated_audio = Some(xrtranslate_protocol::SeparatedAudio {
            vocals,
            background: dir.path().join("unused.wav"),
        });
        assert!(audio(&task, 0, 4000).is_err());
        assert!(audio(&task, 0, 6000).is_err());
        let reference = STANDARD.decode(audio(&task, 6000, 12000).unwrap()).unwrap();
        let mut reader = hound::WavReader::new(Cursor::new(reference)).unwrap();
        assert_eq!(reader.spec().sample_rate, 24000);
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(reader.duration(), 6 * 24000);
        assert!(
            reader
                .samples::<i16>()
                .map(|s| s.unwrap().unsigned_abs())
                .max()
                .unwrap()
                > 6000
        );
        task.speech_reference_range = Some([6000, 12000]);
        assert!(
            audio(&task, 0, 1000).is_ok(),
            "a longer chosen reference can voice a short subtitle"
        );
    }
}
