//! Authored cues use their timeline neighbors as translation context.
use super::MediaPlugin;
use crate::session_coordinator::{
    PluginSessionBinding, PluginSessionOwner, SessionOutputPolicy, TranslationEvent,
    TranslationOutcome, TranslationSegment, TranslationTask,
};
use std::collections::BTreeMap;
use xrtranslate_engine::language::{LanguageCapabilities, LanguageSelection, SupportedLanguage};
use xrtranslate_prompt::{ContextFacts, PromptTurn, SurroundingSource, TaskContext};

pub(super) struct SubtitleJob {
    task: String,
    languages: LanguageSelection,
    pending: Option<(String, String)>,
    segments: BTreeMap<(String, u32), TranslationSegment>,
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
        let context = subtitle_context(cues, index);
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
                if segment.translated.is_some() {
                    job.segments.insert(
                        (segment.turn_id.clone(), segment.segment_index),
                        segment.clone(),
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
                        if segment.translated.is_some() {
                            job.segments.insert(
                                (segment.turn_id.clone(), segment.segment_index),
                                segment.clone(),
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
                            crate::streaming::append_segment(
                                &mut text,
                                segment.translated.as_deref().unwrap_or_default(),
                            );
                        }
                        if text.trim().is_empty() {
                            self.controller.error =
                                Some("No translation was returned. Retry this task.".into());
                            self.subtitle_job = None;
                            self.controller.pause_task();
                        } else {
                            let first = job.segments.values().next().unwrap();
                            let (source, target) = if job.segments.values().all(|segment| {
                                segment.source_lang == first.source_lang
                                    && segment.target_lang == first.target_lang
                            }) {
                                (first.source_lang.as_str(), first.target_lang.as_str())
                            } else {
                                ("", "")
                            };
                            self.controller
                                .subtitles
                                .set_translation(&cue, text, source, target);
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

fn subtitle_context(cues: &[super::subtitles::SubtitleCue], index: usize) -> TaskContext {
    let before = &cues[index.saturating_sub(2)..index];
    let after = &cues[index + 1..(index + 3).min(cues.len())];
    let recent_turns = before
        .iter()
        .filter_map(|cue| {
            let translated = cue.translated_text.as_deref()?.trim();
            let source = SupportedLanguage::parse(&cue.source_language)?;
            let target = SupportedLanguage::parse(&cue.target_language)?;
            (!translated.is_empty()).then(|| PromptTurn {
                turn_id: Some(cue.id.clone()),
                speaker_id: cue.speaker_name.clone().unwrap_or_default(),
                source_language: source.code().to_owned(),
                target_language: target.code().to_owned(),
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
            source_language: String::new(),
            before: join(before),
            after: join(after),
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::media::subtitles::SubtitleTimeline;

    #[test]
    fn subtitle_context_uses_result_languages_and_current_cues() {
        let mut timeline = SubtitleTimeline::parse(
            "1\n00:00:00,000 --> 00:00:01,000\nThe entrance is on the right.\n\n\
             2\n00:00:01,000 --> 00:00:02,000\n我们会等你。\n\n\
             3\n00:00:02,000 --> 00:00:03,000\nThank you.",
        )
        .unwrap();
        timeline.set_translation("srt_0", "入口在右边。".into(), "en", "zh");
        timeline.set_translation("srt_1", "We will wait for you.".into(), "zh", "en");
        let mut edited = timeline.cues()[0].clone();
        edited.original_text = "The entrance is on the left.".into();
        edited.translated_text = Some("入口在左边。".into());
        timeline.edit_cue(edited).unwrap();

        let TaskContext::Snapshot { facts } = subtitle_context(timeline.cues(), 2) else {
            panic!("Subtitles must use a snapshot");
        };
        assert_eq!(facts.recent_turns.len(), 2);
        assert_eq!(facts.recent_turns[0].source_language, "en");
        assert_eq!(facts.recent_turns[0].target_language, "zh");
        assert_eq!(facts.recent_turns[0].translated_text, "入口在左边。");
        assert_eq!(facts.recent_turns[1].source_language, "zh");
        assert_eq!(facts.recent_turns[1].target_language, "en");
        assert!(facts.surrounding_source.unwrap().before.contains("left"));

        timeline.clear_translations();
        timeline.set_translation("srt_1", "Old translation".into(), "", "");
        let TaskContext::Snapshot { facts } = subtitle_context(timeline.cues(), 2) else {
            unreachable!();
        };
        assert!(facts.recent_turns.is_empty());
        assert!(timeline.cues()[0].source_language.is_empty());
        assert!(timeline.cues()[0].target_language.is_empty());
    }
}
