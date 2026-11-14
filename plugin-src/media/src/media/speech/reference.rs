use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};

use crate::media::subtitles::SubtitleCue;
use std::collections::BTreeMap;

const MIN_REFERENCE_MS: i64 = 5_000;
const MAX_REFERENCE_MS: i64 = 20_000;

#[derive(Clone)]
pub(super) struct Prepared {
    pub id: String,
    pub audio: String,
    pub range: [i64; 2],
}

#[derive(Default)]
pub(super) struct References {
    successful: BTreeMap<String, Prepared>,
}

impl References {
    pub fn previous(&self, task: &MediaTask, index: usize) -> Option<Prepared> {
        self.successful
            .get(&speaker_key(task.subtitles.cues(), index))
            .cloned()
    }

    pub fn remember(&mut self, task: &MediaTask, index: usize, reference: Prepared) {
        self.successful
            .insert(speaker_key(task.subtitles.cues(), index), reference);
    }

    pub fn choose(&self, task: &MediaTask, index: usize) -> Result<Prepared, String> {
        let cue = &task.subtitles.cues()[index];
        if task.speech_reference_range.is_none()
            && let Some(speaker) = speaker(cue)
        {
            let id = format!("speaker:{speaker}");
            if task.speech_voices.iter().any(|voice| voice.id == id) {
                return voices::saved(task, &id);
            }
        }
        if let Some([start, end]) = task.speech_reference_range {
            let id = format!("selected:{start}:{end}");
            if let Some(previous) = self
                .successful
                .values()
                .find(|reference| reference.id == id)
            {
                return Ok(previous.clone());
            }
            return Ok(Prepared {
                id,
                audio: audio(task, start, end)?,
                range: [start, end],
            });
        }
        // A known speaker keeps one voice. With no speaker labels, a fresh long
        // passage can update the voice, while short replies retain the last success.
        if (speaker(cue).is_some() || cue.end_ms - cue.start_ms < MIN_REFERENCE_MS)
            && let Some(previous) = self.previous(task, index)
        {
            return Ok(previous);
        }
        // Without an identity, extending a long passage could mix different voices.
        if speaker(cue).is_none()
            && cue.end_ms - cue.start_ms >= MIN_REFERENCE_MS
            && let Ok(audio) = audio(task, cue.start_ms, cue.end_ms)
        {
            return Ok(Prepared {
                id: format!(
                    "passage:{}:{}",
                    cue.start_ms,
                    cue.end_ms.min(cue.start_ms + MAX_REFERENCE_MS)
                ),
                audio,
                range: [
                    cue.start_ms,
                    cue.end_ms.min(cue.start_ms + MAX_REFERENCE_MS),
                ],
            });
        }
        for [start, end] in candidates(task.subtitles.cues(), index) {
            if let Ok(audio) = audio(task, start, end) {
                let id = speaker(cue).map_or_else(
                    || format!("passage:{start}:{end}"),
                    |speaker| format!("speaker:{speaker}"),
                );
                return Ok(Prepared {
                    id,
                    audio,
                    range: [start, end],
                });
            }
        }
        self.previous(task, index).ok_or_else(|| "No suitable original voice reference was found. Select at least 5 seconds of clear speech on the vocal timeline.".into())
    }
}

fn speaker(cue: &SubtitleCue) -> Option<&str> {
    cue.speaker_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty() && *name != "speaker-unknown")
}

fn unlabeled_start(cues: &[SubtitleCue], index: usize) -> usize {
    cues[..index]
        .iter()
        .rposition(|cue| speaker(cue).is_some())
        .map_or(0, |previous| previous + 1)
}

fn speaker_key(cues: &[SubtitleCue], index: usize) -> String {
    speaker(&cues[index]).map_or_else(
        || format!("unlabeled:{}", unlabeled_start(cues, index)),
        |speaker| format!("speaker:{speaker}"),
    )
}

fn candidates(cues: &[SubtitleCue], index: usize) -> Vec<[i64; 2]> {
    let cue = &cues[index];
    let same = |other: &SubtitleCue| speaker(other) == speaker(cue);
    let mut spans = Vec::new();
    let (lower, upper) = if speaker(cue).is_none() {
        (
            unlabeled_start(cues, index),
            cues[index..]
                .iter()
                .position(|cue| speaker(cue).is_some())
                .map_or(cues.len(), |next| index + next),
        )
    } else {
        (0, cues.len())
    };
    for (first, start) in cues
        .iter()
        .enumerate()
        .take(upper)
        .skip(lower)
        .filter(|(_, other)| same(other))
    {
        let mut spoken = 0;
        let mut previous_end = start.start_ms;
        let limit = start.start_ms + MAX_REFERENCE_MS;
        for other in &cues[first..] {
            if !same(other) || other.start_ms - previous_end > 2000 || other.start_ms >= limit {
                break;
            }
            let end = other.end_ms.min(limit);
            spoken += (end - other.start_ms.max(previous_end)).max(0);
            previous_end = previous_end.max(end);
            if previous_end == limit || (speaker(cue).is_none() && spoken >= MIN_REFERENCE_MS) {
                break;
            }
        }
        if spoken >= MIN_REFERENCE_MS {
            spans.push([start.start_ms, previous_end]);
        }
    }
    spans.sort_by_key(|[start, end]| {
        let distance = (cue.start_ms - end).max(start - cue.start_ms).max(0);
        (
            distance,
            std::cmp::Reverse(end - start),
            (*start - cue.start_ms).abs(),
        )
    });
    spans.dedup();
    spans
}

/// Read a bounded passage from the separated vocal track.
pub(super) fn audio(task: &MediaTask, start_ms: i64, end_ms: i64) -> Result<String, String> {
    let [start_ms, end_ms] = task.speech_reference_range.unwrap_or([start_ms, end_ms]);
    if end_ms - start_ms < MIN_REFERENCE_MS {
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
    let frames = ((end_ms - start_ms).min(MAX_REFERENCE_MS) as u64 * u64::from(spec.sample_rate)
        / 1000) as usize;
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

    fn cue(start: i64, end: i64, speaker: Option<&str>) -> SubtitleCue {
        SubtitleCue {
            id: start.to_string(),
            start_ms: start,
            end_ms: end,
            speaker_name: speaker.map(str::to_owned),
            original_text: "Speech".into(),
            translated_text: Some("Translation".into()),
            source_language: "en".into(),
            target_language: "zh".into(),
        }
    }

    #[test]
    fn automatic_references_cover_short_openings_without_crossing_known_speakers() {
        let cues = vec![
            cue(0, 1000, Some("A")),
            cue(1000, 8000, Some("B")),
            cue(9000, 16000, Some("A")),
        ];
        assert_eq!(candidates(&cues, 0), [[9000, 16000]]);
        assert_eq!(candidates(&cues, 1), [[1000, 8000]]);
        let short = vec![
            cue(0, 2000, None),
            cue(2200, 4000, None),
            cue(4200, 6000, None),
        ];
        assert_eq!(candidates(&short, 0), [[0, 6000]]);
        let unknown = vec![
            cue(0, 1000, None),
            cue(1000, 8000, Some("B")),
            cue(9000, 16000, None),
        ];
        assert!(candidates(&unknown, 0).is_empty());
        let gap = vec![cue(0, 2000, None), cue(6000, 9000, None)];
        assert!(candidates(&gap, 0).is_empty());
    }

    #[test]
    fn known_speakers_prefer_long_nearby_passages_with_bounded_duration() {
        let cues = vec![
            cue(0, 6000, Some("A")),
            cue(6200, 12000, Some("A")),
            cue(12000, 18000, Some("A")),
            cue(18000, 25000, Some("A")),
            cue(25000, 31000, Some("B")),
            cue(31000, 32000, Some("B")),
            cue(40000, 47000, Some("A")),
            cue(48000, 54000, Some("A")),
        ];
        assert_eq!(candidates(&cues, 0)[0], [0, 20000]);
        assert_eq!(candidates(&cues, 5)[0], [25000, 32000]);
        assert_eq!(candidates(&cues, 7)[0], [40000, 54000]);
        assert!(candidates(&cues, 0).iter().all(|[start, end]| {
            end - start <= MAX_REFERENCE_MS && (*end <= 25000 || *start >= 40000)
        }));
        let overlapping = vec![cue(0, 3000, Some("A")), cue(1000, 4000, Some("A"))];
        assert!(candidates(&overlapping, 0).is_empty());
    }

    #[test]
    fn unknown_speakers_refresh_long_passages_and_reuse_short_replies() {
        let dir = tempfile::tempdir().unwrap();
        let vocals = dir.path().join("vocals.wav");
        let mut writer = hound::WavWriter::create(&vocals, SPEC).unwrap();
        for frame in 0..SAMPLE_RATE * 11 {
            writer
                .write_sample(if frame < SAMPLE_RATE * 5 {
                    1000_i16
                } else {
                    2000_i16
                })
                .unwrap();
        }
        writer.finalize().unwrap();
        let mut task = MediaTask::new(
            "voices".into(),
            crate::media::backend::MediaSource::LocalFile("source.mp4".into()),
            "en".into(),
            "zh".into(),
            crate::media::task::SubtitleMode::None,
            Default::default(),
        );
        task.separated_audio = Some(xrtranslate_protocol::SeparatedAudio {
            vocals,
            background: dir.path().join("unused.wav"),
        });
        for (start, end) in [(0, 5000), (5000, 10000), (10000, 11000)] {
            task.subtitles.add_cue_with_metadata(
                cue(start, end, Some("speaker-unknown")),
                Default::default(),
            );
        }
        let mut references = References::default();
        let first = references.choose(&task, 0).unwrap();
        references.remember(&task, 0, first.clone());
        let second = references.choose(&task, 1).unwrap();
        assert_ne!(
            first.id, second.id,
            "unknown is not a shared speaker identity"
        );
        assert_ne!(first.audio, second.audio);
        references.remember(&task, 1, second.clone());
        assert_eq!(references.choose(&task, 2).unwrap().id, second.id);
        task.speech_reference_range = Some([0, 5000]);
        let selected = references.choose(&task, 2).unwrap();
        assert_eq!(selected.id, "selected:0:5000");
        assert_eq!(selected.audio, first.audio);
    }

    #[test]
    fn speaker_identity_keeps_known_and_unlabeled_runs_separate() {
        let cues = vec![
            cue(0, 2000, None),
            cue(3000, 9000, Some("A")),
            cue(10000, 12000, None),
            cue(13000, 18000, Some("A")),
        ];
        assert_eq!(speaker_key(&cues, 1), speaker_key(&cues, 3));
        assert_ne!(speaker_key(&cues, 0), speaker_key(&cues, 2));
        assert_ne!(speaker_key(&cues, 0), speaker_key(&cues, 1));
    }

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
        for frame in 0..48_000 * 24 {
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
        task.speech_reference_range = None;
        for (start, end, speaker) in [(6000, 12000, "A"), (12000, 18000, "A"), (18000, 24000, "B")]
        {
            task.subtitles
                .add_cue_with_metadata(cue(start, end, Some(speaker)), Default::default());
        }
        let mut references = References::default();
        let first = references.choose(&task, 0).unwrap();
        let wav = STANDARD.decode(&first.audio).unwrap();
        assert_eq!(
            hound::WavReader::new(Cursor::new(wav)).unwrap().duration(),
            12 * 24000
        );
        references.remember(&task, 0, first.clone());
        assert_eq!(references.choose(&task, 1).unwrap().audio, first.audio);
        assert_ne!(references.choose(&task, 2).unwrap().id, first.id);
    }
}
