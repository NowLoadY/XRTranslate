//! Editable clips are durable; the mixed WAV is a rebuildable playback cache.
use super::{SAMPLE_RATE, SpeechTrack};
use crate::media::task::SpeechClip;
use std::{
    collections::HashSet,
    hash::{DefaultHasher, Hash, Hasher},
    io::{Seek, SeekFrom, Write},
    ops::Range,
};

impl SpeechTrack {
    pub(super) fn key(&self, duration: i64) -> u64 {
        let mut hash = DefaultHasher::new();
        (&self.path, &self.clips, duration).hash(&mut hash);
        hash.finish()
    }

    pub(in crate::media) fn clip_for_cue(
        &self,
        cue: &crate::media::subtitles::SubtitleCue,
    ) -> Option<&SpeechClip> {
        self.clips
            .iter()
            .find(|clip| {
                clip.origin
                    .as_ref()
                    .is_some_and(|origin| origin.cue_id == cue.id)
            })
            .or_else(|| {
                self.clips
                    .iter()
                    .find(|clip| clip.origin.is_none() && clip.start_ms == cue.start_ms)
            })
    }

    pub(in crate::media) fn current(
        &self,
        task: &super::MediaTask,
        cue: &crate::media::subtitles::SubtitleCue,
    ) -> bool {
        self.clip_for_cue(cue).is_some_and(|clip| {
            clip.origin
                .as_ref()
                .map_or(self.language.as_str(), |origin| origin.language.as_str())
                == task.target_language
                && clip.path.is_file()
                && Some(clip.text.trim()) == cue.translated_text.as_deref().map(str::trim)
                && clip.origin.as_ref().is_none_or(|origin| {
                    origin.start_ms == cue.start_ms && origin.end_ms == cue.end_ms
                })
        })
    }

    pub(crate) fn overlaps(&self, start_ms: i64, end_ms: i64) -> bool {
        self.clips
            .iter()
            .any(|clip| clip.start_ms < end_ms && clip.end_ms() > start_ms)
    }
}

impl SpeechClip {
    pub(in crate::media) fn files(&self) -> impl Iterator<Item = &std::path::PathBuf> {
        std::iter::once(&self.path).chain(self.previous.as_deref().map(|clip| &clip.path))
    }
    pub(crate) fn end_ms(&self) -> i64 {
        self.start_ms.saturating_add(self.duration_ms)
    }
}

pub(super) struct RenderRequest {
    pub task_id: String,
    pub track: SpeechTrack,
    pub duration: i64,
    retired: Vec<std::path::PathBuf>,
}

pub(super) struct RenderResult {
    pub task_id: String,
    pub key: u64,
    pub path: std::path::PathBuf,
    pub result: Result<(), String>,
}

pub(in crate::media) struct RenderQueue {
    send: crossbeam_channel::Sender<RenderRequest>,
    pub(super) receive: crossbeam_channel::Receiver<RenderResult>,
}

impl RenderQueue {
    pub(super) fn new(ctx: eframe::egui::Context) -> Result<Self, String> {
        let (send, requests) = crossbeam_channel::unbounded::<RenderRequest>();
        let (results, receive) = crossbeam_channel::unbounded();
        std::thread::Builder::new()
            .name("media-speech-mix".into())
            .spawn(move || {
                let mut previous: Option<SpeechTrack> = None;
                for request in requests {
                    let key = request.track.key(request.duration);
                    let result = render(&request.track, request.duration, previous.as_ref());
                    for path in request.retired {
                        if path != request.track.path
                            && path.parent() == request.track.path.parent()
                        {
                            let _ = std::fs::remove_file(path);
                        }
                    }
                    let path = request.track.path.clone();
                    previous = result.is_ok().then_some(request.track);
                    if results
                        .send(RenderResult {
                            task_id: request.task_id,
                            path,
                            key,
                            result,
                        })
                        .is_err()
                    {
                        break;
                    }
                    ctx.request_repaint();
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { send, receive })
    }

    pub(super) fn push(
        &self,
        task: &super::MediaTask,
        retired: Vec<std::path::PathBuf>,
    ) -> Result<(), String> {
        self.send
            .send(RenderRequest {
                task_id: task.id.clone(),
                track: task.speech_track.clone().unwrap(),
                duration: task.duration_ms,
                retired,
            })
            .map_err(|e| e.to_string())
    }
}

/// Rewrite only changed regions. Long pauses are sparse zero-filled file ranges,
/// so adding a clip does not rewrite the preceding film on every synthesis step.
pub(super) fn render(
    track: &SpeechTrack,
    duration: i64,
    previous: Option<&SpeechTrack>,
) -> Result<(), String> {
    let run = || -> Result<(), Box<dyn std::error::Error>> {
        let previous = previous.filter(|old| old.path == track.path && track.path.is_file());
        let duration = track
            .clips
            .iter()
            .map(SpeechClip::end_ms)
            .max()
            .unwrap_or(0)
            .max(duration);
        let samples = duration.max(0) as u64 * u64::from(SAMPLE_RATE) / 1000;
        let bytes = u32::try_from(samples.checked_mul(2).ok_or("Audio track is too long")?)?;
        let size = bytes.checked_add(36).ok_or("Audio track is too long")?;
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(previous.is_none())
            .open(&track.path)?;
        // Standard mono PCM16 RIFF header. Hound reads the immutable clip files.
        output.write_all(b"RIFF")?;
        output.write_all(&size.to_le_bytes())?;
        output.write_all(b"WAVEfmt \x10\0\0\0\x01\0\x01\0")?;
        output.write_all(&SAMPLE_RATE.to_le_bytes())?;
        output.write_all(&(SAMPLE_RATE * 2).to_le_bytes())?;
        output.write_all(b"\x02\0\x10\0data")?;
        output.write_all(&bytes.to_le_bytes())?;
        output.set_len(44 + u64::from(bytes))?;
        let range = |clip: &SpeechClip| -> Range<u64> {
            clip.start_ms.max(0) as u64 * u64::from(SAMPLE_RATE) / 1000
                ..clip.end_ms().max(0) as u64 * u64::from(SAMPLE_RATE) / 1000
        };
        let mut changed = Vec::new();
        if let Some(old) = previous {
            let current: HashSet<_> = track.clips.iter().collect();
            let previous: HashSet<_> = old.clips.iter().collect();
            for clip in &old.clips {
                if !current.contains(clip) {
                    changed.push(range(clip));
                }
            }
            for clip in &track.clips {
                if !previous.contains(clip) {
                    changed.push(range(clip));
                }
            }
        } else {
            changed.extend(track.clips.iter().map(range));
        }
        changed.sort_unstable_by_key(|range| range.start);
        let mut regions: Vec<Range<u64>> = Vec::new();
        for mut region in changed {
            region.end = region.end.min(samples);
            if let Some(last) = regions.last_mut()
                && region.start <= last.end
            {
                last.end = last.end.max(region.end);
            } else {
                regions.push(region);
            }
        }
        for region in regions {
            let mut sources = Vec::new();
            for clip in &track.clips {
                let span = range(clip);
                if span.start < region.end && span.end > region.start {
                    let reader = hound::WavReader::open(&clip.path)?;
                    if reader.spec() != super::SPEC {
                        return Err("Invalid speech clip audio".into());
                    }
                    sources.push((span, reader));
                }
            }
            let mut start = region.start;
            while start < region.end {
                let end = (start + 16_384).min(region.end);
                let mut mixed = vec![0i32; (end - start) as usize];
                for (span, reader) in &mut sources {
                    let from = start.max(span.start);
                    let to = end
                        .min(span.end)
                        .min(span.start + u64::from(reader.duration()));
                    if from >= to {
                        continue;
                    }
                    reader.seek(u32::try_from(from - span.start)?)?;
                    for (slot, sample) in mixed[(from - start) as usize..(to - start) as usize]
                        .iter_mut()
                        .zip(reader.samples::<i16>())
                    {
                        *slot += i32::from(sample?);
                    }
                }
                let bytes: Vec<u8> = mixed
                    .into_iter()
                    .flat_map(|s| (s.clamp(i16::MIN as i32, i16::MAX as i32) as i16).to_le_bytes())
                    .collect();
                output.seek(SeekFrom::Start(44 + start * 2))?;
                output.write_all(&bytes)?;
                start = end;
            }
        }
        output.sync_data()?;
        Ok(())
    };
    run().map_err(|e| e.to_string())
}
