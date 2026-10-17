//! Authored cues use their timeline neighbors as translation context.
use super::MediaPlugin;
use crate::session_coordinator::{
    PluginSessionBinding, PluginSessionOwner, SessionOutputPolicy, TranslationEvent,
    TranslationOutcome, TranslationTask,
};
use std::collections::BTreeMap;
use xrtranslate_engine::language::{LanguageCapabilities, LanguageSelection};
use xrtranslate_prompt::{ContextFacts, PromptTurn, SurroundingSource, TaskContext};

pub(super) struct SubtitleJob {
    task: String,
    languages: LanguageSelection,
    pending: Option<(String, String)>,
    segments: BTreeMap<(String, u32), String>,
}

impl MediaPlugin {
    pub(crate) fn start_subtitles(
        &mut self,
        languages: LanguageCapabilities,
        restart: bool,
    ) -> Result<(), String> {
        let task = self
            .controller
            .active_task_id
            .as_deref()
            .and_then(|id| self.controller.store.get(id))
            .ok_or("Task not found")?;
        let selection = languages
            .for_text()
            .select(&task.source_language, &task.target_language)?;
        self.subtitle_job = Some(SubtitleJob {
            task: task.id.clone(),
            languages: selection,
            pending: None,
            segments: BTreeMap::new(),
        });
        if restart {
            self.controller.subtitles.clear_translations();
        }
        self.controller.start_task();
        Ok(())
    }

    pub(crate) fn next_subtitle_task(&mut self) -> Option<TranslationTask> {
        let job = self.subtitle_job.as_mut()?;
        if job.pending.is_some() {
            return None;
        }
        if self.controller.active_task_id.as_deref() != Some(job.task.as_str()) {
            self.subtitle_job = None;
            return None;
        }
        let cues = self.controller.subtitles.cues();
        let Some(index) = cues.iter().position(|cue| {
            cue.translated_text
                .as_deref()
                .is_none_or(|text| text.trim().is_empty())
        }) else {
            self.subtitle_job = None;
            self.controller.pause_task();
            return None;
        };
        let cue = &cues[index];
        let context = subtitle_context(cues, index, job.languages);
        let operation = uuid::Uuid::new_v4().to_string();
        let owner = PluginSessionOwner::new(
            super::super::PluginId::MEDIA.as_str(),
            &operation,
            "Media",
            "Media",
            "Translating subtitles",
        );
        job.pending = Some((operation, cue.id.clone()));
        Some(
            TranslationTask::text(
                cue.original_text.clone(),
                job.languages,
                Some(PluginSessionBinding::text(
                    owner,
                    SessionOutputPolicy::PluginOnly,
                )),
            )
            .with_context(context),
        )
    }

    pub(super) fn accept_subtitle_event(
        &mut self,
        operation: &str,
        event: &TranslationEvent,
    ) -> bool {
        let Some(job) = &mut self.subtitle_job else {
            return false;
        };
        if job.pending.as_ref().is_none_or(|(id, _)| id != operation) {
            return false;
        }
        match event {
            TranslationEvent::Segment(segment) => {
                if let Some(text) = &segment.translated {
                    job.segments.insert(
                        (segment.turn_id.clone(), segment.segment_index),
                        text.clone(),
                    );
                }
            }
            TranslationEvent::ReplaceSegments(segments) => {
                if let Some(first) = segments
                    .first()
                    .filter(|segment| segment.translated.is_some())
                {
                    job.segments.retain(|(turn, _), _| turn != &first.turn_id);
                    for segment in segments {
                        if let Some(text) = &segment.translated {
                            job.segments.insert(
                                (segment.turn_id.clone(), segment.segment_index),
                                text.clone(),
                            );
                        }
                    }
                }
            }
            TranslationEvent::Finished { outcome, .. } => {
                let (_, cue) = job.pending.take().unwrap();
                match outcome {
                    TranslationOutcome::Completed if !job.segments.is_empty() => {
                        let mut text = String::new();
                        for segment in job.segments.values() {
                            crate::streaming::append_segment(&mut text, segment);
                        }
                        if text.trim().is_empty() {
                            self.controller.error =
                                Some("No translation was returned. Retry this task.".into());
                            self.subtitle_job = None;
                            self.controller.pause_task();
                        } else {
                            self.controller.subtitles.set_translation(&cue, text);
                            job.segments.clear();
                        }
                    }
                    _ => {
                        if let TranslationOutcome::Failed(error) = outcome {
                            self.controller.error = Some(error.clone());
                        } else if matches!(outcome, TranslationOutcome::Completed) {
                            self.controller.error =
                                Some("No translation was returned. Retry this task.".into());
                        }
                        self.subtitle_job = None;
                        self.controller.pause_task();
                    }
                }
                self.controller.dirty = true;
            }
            TranslationEvent::Preview(_) | TranslationEvent::StreamEnded { .. } => {}
        }
        true
    }
}

fn subtitle_context(
    cues: &[super::subtitles::SubtitleCue],
    index: usize,
    languages: LanguageSelection,
) -> TaskContext {
    let (source_language, target_language) = languages.wire();
    let before = &cues[index.saturating_sub(2)..index];
    let after = &cues[index + 1..(index + 3).min(cues.len())];
    let recent_turns = before
        .iter()
        .filter_map(|cue| {
            let translated = cue.translated_text.as_deref()?.trim();
            (!translated.is_empty()).then(|| PromptTurn {
                turn_id: Some(cue.id.clone()),
                speaker_id: cue.speaker_name.clone().unwrap_or_default(),
                source_language: source_language.clone(),
                target_language: target_language.clone(),
                source_text: cue.original_text.clone(),
                translated_text: translated.to_owned(),
            })
        })
        .collect();
    let join = |neighbors: &[super::subtitles::SubtitleCue]| {
        neighbors
            .iter()
            .map(|cue| cue.original_text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    };
    TaskContext::snapshot(ContextFacts {
        recent_turns,
        surrounding_source: Some(SurroundingSource {
            speaker_id: cues[index].speaker_name.clone().unwrap_or_default(),
            source_language,
            before: join(before),
            after: join(after),
        }),
    })
}
