//! One authored cue at a time reuses the shared translator and conversation.
use super::MediaPlugin;
use crate::session_coordinator::{
    PluginSessionBinding, PluginSessionOwner, SessionOutputPolicy, TranslationEvent,
    TranslationOutcome, TranslationTask,
};
use std::collections::BTreeMap;
use xrtranslate_engine::language::{LanguageCapabilities, LanguageSelection};

pub(super) struct SubtitleJob {
    task: String,
    conversation: String,
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
            conversation: uuid::Uuid::new_v4().to_string(),
            languages: selection,
            pending: None,
            segments: BTreeMap::new(),
        });
        if restart {
            self.controller.subtitles.clear_translations();
        }
        self.controller.error = None;
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
        let Some(cue) = self.controller.subtitles.cues().iter().find(|cue| {
            cue.translated_text
                .as_deref()
                .is_none_or(|text| text.trim().is_empty())
        }) else {
            self.subtitle_job = None;
            self.controller.pause_task();
            return None;
        };
        let operation = uuid::Uuid::new_v4().to_string();
        let owner = PluginSessionOwner::new(
            super::super::PluginId::MEDIA.as_str(),
            &operation,
            "Media",
            "Media",
            "Translating subtitles",
        )
        .in_conversation(&job.conversation);
        job.pending = Some((operation, cue.id.clone()));
        Some(TranslationTask::text(
            cue.original_text.clone(),
            job.languages,
            Some(PluginSessionBinding::text(
                owner,
                SessionOutputPolicy::PluginOnly,
            )),
        ))
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
            TranslationEvent::StreamEnded { .. } => {}
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_coordinator::{SessionEventSubscriber, TranslationInput, test_segment};
    #[test]
    fn subtitle_task_without_playback_resumes_and_exports_original_timestamps() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("captions.srt");
        std::fs::write(&path, "7\n00:00:01,015 --> 00:00:01,100\nFirst\nline\n \n8\n00:00:02,001 --> 00:00:03,009\nSecond\n").unwrap();
        let mut plugin = MediaPlugin::new();
        plugin.controller.backend = None;
        plugin.controller.storage_dir = root.path().into();
        plugin.controller.store = Default::default();
        plugin.controller.draft_source = path.to_string_lossy().into_owned();
        let id = plugin.controller.start_draft_task().unwrap();
        assert!(!plugin.controller.can_play());
        assert!(plugin.controller.is_subtitle_task());
        let caps = LanguageCapabilities::default();
        plugin.start_subtitles(caps, false).unwrap();
        let first = plugin.next_subtitle_task().unwrap();
        assert!(matches!(&first.input, TranslationInput::Text(text) if text == "First\nline"));
        assert!(!first.plugin.as_ref().unwrap().publish_to_host_outputs());
        plugin.event_sink.on_translation_event(
            &first.owner(),
            &TranslationEvent::Segment(test_segment("First\nline", "第一行")),
        );
        plugin.event_sink.on_translation_event(
            &first.owner(),
            &TranslationEvent::Finished {
                stream_id: 1,
                outcome: TranslationOutcome::Completed,
            },
        );
        plugin.poll_translation_events();
        let interrupted = plugin.next_subtitle_task().unwrap();
        plugin.pause_task();
        plugin.controller.open_library();
        plugin.controller.store = super::super::task::MediaTaskStore::load_from_dir(root.path());
        plugin.controller.open_task(&id).unwrap();
        assert!(!plugin.controller.store.get(&id).unwrap().is_task_running);
        plugin.start_subtitles(caps, false).unwrap();
        let second = plugin.next_subtitle_task().unwrap();
        assert!(matches!(&second.input, TranslationInput::Text(text) if text == "Second"));
        plugin.event_sink.on_translation_event(
            &interrupted.owner(),
            &TranslationEvent::Segment(test_segment("Second", "过时结果")),
        );
        plugin.event_sink.on_translation_event(
            &second.owner(),
            &TranslationEvent::Segment(test_segment("Second", "第二行")),
        );
        plugin.event_sink.on_translation_event(
            &second.owner(),
            &TranslationEvent::Finished {
                stream_id: 2,
                outcome: TranslationOutcome::Completed,
            },
        );
        plugin.poll_translation_events();
        assert!(plugin.next_subtitle_task().is_none());
        let exported = plugin.controller.subtitles.export_srt();
        assert!(exported.contains("7\n00:00:01,015 --> 00:00:01,100\nFirst\nline\n第一行"));
        assert!(exported.contains("8\n00:00:02,001 --> 00:00:03,009\nSecond\n第二行"));
        assert!(!exported.contains("过时结果"));
        plugin.start_subtitles(caps, true).unwrap();
        assert!(
            plugin
                .controller
                .subtitles
                .cues()
                .iter()
                .all(|cue| cue.translated_text.is_none())
        );
        assert_eq!(plugin.controller.subtitles.count(), 2);
    }
}
