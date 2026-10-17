//! Media owns subtitle context; inference owns prompt construction and model capabilities.
use super::{
    MediaPlugin,
    subtitles::{SubtitleCue, SubtitleTimeline},
};
use crate::session_coordinator::{
    PluginSessionBinding, PluginSessionOwner, SessionOutputPolicy, TranslationEvent,
    TranslationOutcome, TranslationSegment, TranslationTask,
};
use std::collections::BTreeMap;
use xrtranslate_engine::language::{LanguageCapabilities, LanguageSelection, SupportedLanguage};
use xrtranslate_prompt::{ContextFacts, PromptTurn, SurroundingSource, TaskContext};

const CONTEXT_CUES: usize = 2;

pub(super) struct SubtitleJob {
    task: String,
    languages: LanguageSelection,
    output_policy: SessionOutputPolicy,
    source_open: bool,
    waiting_revision: Option<u64>,
    pending: Option<(SubtitleTranslation, String)>,
}

/// One sentence uses the same request and result handling in a batch or an editor.
pub(super) struct SubtitleTranslation {
    pub(super) operation: String,
    pub(super) cue: String,
    segments: BTreeMap<(String, u32), TranslationSegment>,
}

pub(super) struct SubtitleTranslationResult {
    pub(super) text: String,
    pub(super) source_language: String,
    pub(super) target_language: String,
}

impl SubtitleTranslation {
    pub(super) fn new(cue: &str) -> Self {
        Self {
            operation: uuid::Uuid::new_v4().to_string(),
            cue: cue.to_owned(),
            segments: BTreeMap::new(),
        }
    }

    pub(super) fn task(
        &self,
        cues: &[SubtitleCue],
        index: usize,
        text: String,
        languages: LanguageSelection,
    ) -> TranslationTask {
        let owner = PluginSessionOwner::new(
            super::super::PluginId::MEDIA.as_str(),
            &self.operation,
            "Media",
            "Media",
            "Translating subtitles",
        );
        TranslationTask::text(
            text,
            languages,
            Some(PluginSessionBinding::text(
                owner,
                SessionOutputPolicy::PluginOnly,
            )),
        )
        .with_context(subtitle_context(cues, index))
    }

    pub(super) fn accept(
        &mut self,
        event: &TranslationEvent,
    ) -> Option<Result<SubtitleTranslationResult, String>> {
        match event {
            TranslationEvent::Segment(segment) if segment.translated.is_some() => {
                self.segments.insert(
                    (segment.turn_id.clone(), segment.segment_index),
                    segment.clone(),
                );
            }
            TranslationEvent::ReplaceSegments(segments) => {
                if let Some(first) = segments
                    .first()
                    .filter(|segment| segment.translated.is_some())
                {
                    self.segments.retain(|(turn, _), _| turn != &first.turn_id);
                    for segment in segments
                        .iter()
                        .filter(|segment| segment.translated.is_some())
                    {
                        self.segments.insert(
                            (segment.turn_id.clone(), segment.segment_index),
                            segment.clone(),
                        );
                    }
                }
            }
            TranslationEvent::Finished { outcome, .. } => {
                return Some(match outcome {
                    TranslationOutcome::Completed => {
                        let mut text = String::new();
                        for segment in self.segments.values() {
                            crate::streaming::append_segment(
                                &mut text,
                                segment.translated.as_deref().unwrap_or_default(),
                            );
                        }
                        if text.trim().is_empty() {
                            Err("No translation was returned. Retry this task.".into())
                        } else {
                            let first = self.segments.values().next().unwrap();
                            let (source_language, target_language) =
                                if self.segments.values().all(|segment| {
                                    segment.source_lang == first.source_lang
                                        && segment.target_lang == first.target_lang
                                }) {
                                    (first.source_lang.clone(), first.target_lang.clone())
                                } else {
                                    (String::new(), String::new())
                                };
                            Ok(SubtitleTranslationResult {
                                text,
                                source_language,
                                target_language,
                            })
                        }
                    }
                    TranslationOutcome::Failed(error) => Err(error.clone()),
                    TranslationOutcome::Cancelled => Err("Cancelled".into()),
                });
            }
            _ => {}
        }
        None
    }
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
            output_policy: SessionOutputPolicy::PluginOnly,
            source_open: false,
            waiting_revision: None,
            pending: None,
        });
        self.stop_requested = false;
        if restart {
            self.controller.subtitles.clear_translations();
        }
        self.controller.start_task();
        Ok(())
    }

    /// File recognition feeds the same ordered subtitle translator as authored files.
    pub(crate) fn start_file_subtitles(
        &mut self,
        languages: LanguageSelection,
    ) -> Result<(), String> {
        let task = self
            .controller
            .active_task_id
            .clone()
            .ok_or("Task not found")?;
        self.controller.start_task();
        self.stop_requested = false;
        self.subtitle_job = Some(SubtitleJob {
            task,
            languages,
            output_policy: SessionOutputPolicy::Host,
            source_open: true,
            waiting_revision: None,
            pending: None,
        });
        Ok(())
    }

    pub(crate) fn take_stop_request(&mut self) -> bool {
        std::mem::take(&mut self.stop_requested)
    }

    pub(crate) fn fail_subtitle_startup(&mut self) {
        self.pause_task();
        self.stop_requested = true;
    }

    pub(crate) fn next_subtitle_task(&mut self) -> Option<TranslationTask> {
        let job = self.subtitle_job.as_ref()?;
        if self.controller.active_task_id.as_deref() != Some(job.task.as_str()) {
            self.subtitle_job = None;
            self.stop_requested = true;
            return None;
        }
        if job.pending.is_some() {
            return None;
        }
        let revision = self.controller.subtitles.revision();
        if job.source_open && job.waiting_revision == Some(revision) {
            return None;
        }
        let cues = self.controller.subtitles.cues();
        let Some(index) = cues.iter().position(needs_translation) else {
            if !job.source_open {
                self.subtitle_job = None;
                self.controller.pause_task();
            } else {
                self.subtitle_job.as_mut()?.waiting_revision = Some(revision);
            }
            return None;
        };
        if !cue_ready(&self.controller.subtitles, index, job.source_open) {
            self.subtitle_job.as_mut()?.waiting_revision = Some(revision);
            return None;
        }
        let translation = SubtitleTranslation::new(&cues[index].id);
        let source = cues[index].original_text.clone();
        let mut task = translation.task(cues, index, source.clone(), job.languages);
        let binding = task.plugin.as_mut()?;
        binding.output_policy = job.output_policy;
        if job.output_policy == SessionOutputPolicy::Host {
            binding.owner = binding
                .owner
                .clone()
                .with_audio_source(crate::CaptureSource::SystemAudio);
        }
        self.subtitle_job.as_mut()?.pending = Some((translation, source));
        Some(task)
    }

    pub(super) fn finish_subtitle_source(&mut self, outcome: &TranslationOutcome) -> bool {
        let Some(job) = &mut self.subtitle_job else {
            return false;
        };
        if !job.source_open {
            return false;
        }
        match outcome {
            TranslationOutcome::Completed => job.source_open = false,
            _ => {
                self.subtitle_job = None;
                self.stop_requested = true;
                self.controller.pause_task();
            }
        }
        true
    }

    pub(super) fn accept_subtitle_event(
        &mut self,
        operation: &str,
        event: &TranslationEvent,
    ) -> bool {
        let Some(job) = &mut self.subtitle_job else {
            return false;
        };
        if self.controller.active_task_id.as_deref() != Some(job.task.as_str()) {
            self.subtitle_job = None;
            self.stop_requested = true;
            return false;
        }
        let Some((pending, source)) = &mut job.pending else {
            return false;
        };
        if pending.operation != operation {
            return false;
        }
        self.controller.previews.observe(event);
        let Some(result) = pending.accept(event) else {
            return true;
        };
        let cue = pending.cue.clone();
        let source = source.clone();
        job.pending = None;
        match result {
            Ok(result) => {
                // An ASR revision or an edit may have replaced the input while inference ran.
                if self
                    .controller
                    .subtitles
                    .cues()
                    .iter()
                    .any(|current| current.id == cue && current.original_text == source)
                {
                    self.controller.subtitles.set_translation(
                        &cue,
                        result.text,
                        &result.source_language,
                        &result.target_language,
                    );
                }
            }
            Err(error) => {
                if !matches!(
                    event,
                    TranslationEvent::Finished {
                        outcome: TranslationOutcome::Cancelled,
                        ..
                    }
                ) {
                    self.controller.error = Some(error);
                }
                self.subtitle_job = None;
                self.stop_requested = true;
                self.controller.pause_task();
            }
        }
        self.controller.dirty = true;
        true
    }
}

fn needs_translation(cue: &SubtitleCue) -> bool {
    cue.translated_text
        .as_deref()
        .is_none_or(|text| text.trim().is_empty())
}

fn cue_ready(timeline: &SubtitleTimeline, index: usize, source_open: bool) -> bool {
    let cues = timeline.cues();
    !source_open
        || (index + CONTEXT_CUES < cues.len()
            && cues[index..=index + CONTEXT_CUES]
                .iter()
                .all(|cue| timeline.metadata_for(&cue.id).finalized))
}

pub(super) fn subtitle_context(cues: &[SubtitleCue], index: usize) -> TaskContext {
    let before = &cues[index.saturating_sub(CONTEXT_CUES)..index];
    let after = &cues[index + 1..(index + 1 + CONTEXT_CUES).min(cues.len())];
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
    let join = |neighbors: &[SubtitleCue]| {
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

    fn fixture() -> (MediaPlugin, tempfile::TempDir) {
        use crate::plugins::media::{
            backend::MediaSource,
            task::{MediaTask, SubtitleMode},
        };
        let storage = tempfile::tempdir().unwrap();
        let mut plugin = MediaPlugin::new();
        plugin.controller.backend = None;
        plugin.controller.storage_dir = storage.path().to_owned();
        plugin.controller.store.tasks.clear();
        let task = MediaTask::new(
            "Test media".into(),
            MediaSource::LocalFile("test.mp3".into()),
            "en".into(),
            "zh".into(),
            SubtitleMode::RealtimeTranslation,
            Default::default(),
        );
        plugin.controller.active_task_id = Some(task.id.clone());
        plugin.controller.store.add_or_update(task);
        plugin
            .start_file_subtitles(LanguageSelection::parse("en", "zh").unwrap())
            .unwrap();
        (plugin, storage)
    }

    fn add_source(plugin: &mut MediaPlugin, index: usize, finalized: bool) {
        plugin.controller.subtitles.add_cue_with_metadata(
            SubtitleCue {
                id: format!("stream_7_cue-{index}"),
                start_ms: index as i64 * 1_000,
                end_ms: (index as i64 + 1) * 1_000,
                speaker_name: None,
                original_text: format!("Sentence {index}."),
                translated_text: None,
                source_language: "en".into(),
                target_language: String::new(),
            },
            super::super::subtitles::SubtitleMetadata {
                finalized,
                ..Default::default()
            },
        );
    }

    fn complete(plugin: &mut MediaPlugin, request: &TranslationTask) {
        let operation = request.owner().operation_id().unwrap().to_owned();
        let source = match &request.input {
            crate::session_coordinator::TranslationInput::Text(text) => text.clone(),
            _ => unreachable!(),
        };
        let segment = TranslationSegment {
            stream_id: 90,
            audio_source: crate::CaptureSource::SystemAudio,
            turn_id: operation.clone(),
            segment_index: 1,
            segment_count: 1,
            source,
            translated: Some("译文。".into()),
            source_lang: "en".into(),
            target_lang: "zh".into(),
            additional_translations: Vec::new(),
            asr_only: false,
            speaker_id: String::new(),
            source_start_ms: 0.0,
            source_end_ms: 0.0,
            timing: Default::default(),
            boundary: Default::default(),
            revisable: false,
            live: false,
        };
        assert!(plugin.accept_subtitle_event(&operation, &TranslationEvent::Segment(segment)));
        assert!(plugin.accept_subtitle_event(
            &operation,
            &TranslationEvent::Finished {
                stream_id: 90,
                outcome: TranslationOutcome::Completed,
            }
        ));
    }

    #[test]
    fn file_translation_waits_for_two_stable_sentences_and_drains_the_tail() {
        let (mut plugin, _storage) = fixture();
        add_source(&mut plugin, 0, true);
        add_source(&mut plugin, 1, true);
        assert!(plugin.next_subtitle_task().is_none());
        add_source(&mut plugin, 2, false);
        assert!(plugin.next_subtitle_task().is_none());
        add_source(&mut plugin, 2, true);
        let first = plugin.next_subtitle_task().unwrap();
        assert_eq!(
            first.plugin.as_ref().unwrap().output_policy,
            SessionOutputPolicy::Host
        );
        assert!(!first.plugin.as_ref().unwrap().host_tts);
        assert_eq!(
            first.owner().plugin().unwrap().audio_source(),
            Some(crate::CaptureSource::SystemAudio)
        );
        plugin.controller.current_source = Some(super::super::backend::MediaSource::LocalFile(
            "test.mp3".into(),
        ));
        assert_eq!(
            crate::session_coordinator::TranslationSessionPlugin::translation_session_binding(
                &plugin
            )
            .unwrap()
            .output_policy,
            SessionOutputPolicy::PluginOnly
        );
        assert!(plugin.next_subtitle_task().is_none());
        let TaskContext::Snapshot { facts } = &first.context else {
            panic!("snapshot required")
        };
        assert!(facts.recent_turns.is_empty());
        assert_eq!(
            facts.surrounding_source.as_ref().unwrap().after,
            "Sentence 1.\nSentence 2."
        );
        complete(&mut plugin, &first);
        assert!(plugin.next_subtitle_task().is_none());
        // The host retires its audio transport without ending the subtitle job.
        plugin.stop_import();
        let source_owner =
            crate::session_coordinator::TranslationSessionOwner::Plugin(PluginSessionOwner::new(
                super::super::super::PluginId::MEDIA.as_str(),
                plugin.controller.active_task_id.as_deref().unwrap(),
                "Media",
                "Media",
                "Media",
            ));
        crate::session_coordinator::SessionEventSubscriber::on_translation_event(
            &plugin.event_sink,
            &source_owner,
            &TranslationEvent::Finished {
                stream_id: 7,
                outcome: TranslationOutcome::Completed,
            },
        );
        plugin.poll_translation_events();
        assert!(
            plugin
                .controller
                .store
                .get(plugin.controller.active_task_id.as_deref().unwrap())
                .unwrap()
                .is_task_running
        );
        let second = plugin.next_subtitle_task().unwrap();
        let TaskContext::Snapshot { facts } = &second.context else {
            unreachable!()
        };
        assert_eq!(facts.recent_turns[0].source_text, "Sentence 0.");
        assert_eq!(facts.recent_turns[0].translated_text, "译文。");
        assert_eq!(
            facts.surrounding_source.as_ref().unwrap().after,
            "Sentence 2."
        );
        complete(&mut plugin, &second);
        let third = plugin.next_subtitle_task().unwrap();
        complete(&mut plugin, &third);
        assert!(plugin.next_subtitle_task().is_none());
        assert!(plugin.subtitle_job.is_none());
        assert!(
            !plugin
                .controller
                .store
                .get(plugin.controller.active_task_id.as_deref().unwrap())
                .unwrap()
                .is_task_running
        );
    }

    #[test]
    fn cancelled_failed_and_switched_tasks_do_not_commit_late_results() {
        for outcome in [
            TranslationOutcome::Cancelled,
            TranslationOutcome::Failed("offline".into()),
        ] {
            let (mut plugin, _storage) = fixture();
            for index in 0..3 {
                add_source(&mut plugin, index, true);
            }
            let request = plugin.next_subtitle_task().unwrap();
            plugin
                .controller
                .previews
                .observe(&TranslationEvent::Preview(
                    crate::session_coordinator::TranslationPreview {
                        stream_id: 90,
                        audio_source: crate::CaptureSource::SystemAudio,
                        turn_id: "pending".into(),
                        segment_index: 1,
                        source: "Sentence 0.".into(),
                        translated: Some("正在翻译".into()),
                        revision: 0,
                    },
                ));
            assert!(plugin.finish_subtitle_source(&outcome));
            assert!(plugin.take_stop_request());
            plugin.pause_task();
            assert!(plugin.controller.previews.entries().is_empty());
            assert!(!plugin.accept_subtitle_event(
                request.owner().operation_id().unwrap(),
                &TranslationEvent::Finished {
                    stream_id: 90,
                    outcome: TranslationOutcome::Completed,
                }
            ));
            assert!(
                plugin
                    .controller
                    .subtitles
                    .cues()
                    .iter()
                    .all(needs_translation)
            );
        }
        let (mut plugin, _storage) = fixture();
        for index in 0..3 {
            add_source(&mut plugin, index, true);
        }
        let request = plugin.next_subtitle_task().unwrap();
        plugin.controller.active_task_id = Some("other-task".into());
        assert!(!plugin.accept_subtitle_event(
            request.owner().operation_id().unwrap(),
            &TranslationEvent::Finished {
                stream_id: 90,
                outcome: TranslationOutcome::Completed,
            }
        ));
        assert!(plugin.take_stop_request());
        assert!(
            plugin
                .controller
                .subtitles
                .cues()
                .iter()
                .all(needs_translation)
        );
    }

    #[test]
    fn short_files_drain_without_a_full_lookahead() {
        for count in 0..CONTEXT_CUES + 1 {
            let (mut plugin, _storage) = fixture();
            for index in 0..count {
                add_source(&mut plugin, index, true);
            }
            assert!(plugin.next_subtitle_task().is_none());
            plugin.finish_subtitle_source(&TranslationOutcome::Completed);
            for _ in 0..count {
                let request = plugin.next_subtitle_task().unwrap();
                complete(&mut plugin, &request);
            }
            assert!(plugin.next_subtitle_task().is_none());
            assert!(plugin.subtitle_job.is_none());
        }
    }

    #[test]
    fn finishing_a_stream_rechecks_waiting_sentences_without_new_text() {
        let (mut plugin, _storage) = fixture();
        for index in 0..3 {
            add_source(&mut plugin, index, false);
        }
        assert!(plugin.next_subtitle_task().is_none());
        let before = plugin.controller.subtitles.revision();
        plugin.controller.subtitles.finish_stream(7);
        assert_ne!(plugin.controller.subtitles.revision(), before);
        assert!(plugin.next_subtitle_task().is_some());
    }

    #[test]
    fn revised_source_rejects_old_translation_and_text_failure_stops_recognition() {
        let (mut plugin, _storage) = fixture();
        for index in 0..3 {
            add_source(&mut plugin, index, true);
        }
        let request = plugin.next_subtitle_task().unwrap();
        let mut edited = plugin.controller.subtitles.cues()[0].clone();
        edited.original_text = "Corrected source.".into();
        plugin.controller.subtitles.edit_cue(edited).unwrap();
        complete(&mut plugin, &request);
        assert!(needs_translation(&plugin.controller.subtitles.cues()[0]));
        let request = plugin.next_subtitle_task().unwrap();
        assert!(plugin.accept_subtitle_event(
            request.owner().operation_id().unwrap(),
            &TranslationEvent::Finished {
                stream_id: 90,
                outcome: TranslationOutcome::Failed("offline".into()),
            }
        ));
        assert!(plugin.take_stop_request());
        assert_eq!(plugin.controller.error.as_deref(), Some("offline"));
        assert!(plugin.subtitle_job.is_none());
    }
}
