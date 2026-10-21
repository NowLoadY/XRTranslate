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
use xrtranslate_engine::language::{
    LanguageCapabilities, LanguageMode, LanguageSelection, SupportedLanguage,
};
use xrtranslate_prompt::{ContextFacts, PromptTurn, SurroundingSource, TaskContext};

const CONTEXT_CUES: usize = 2;
const PREVIOUS_CUES: usize = 8;
const FOLLOWING_CUES: usize = 4;
pub(super) const CONTEXT_GAP_MS: i64 = 30_000;

pub(super) struct SubtitleJob {
    task: String,
    languages: LanguageSelection,
    output_policy: SessionOutputPolicy,
    source_open: bool,
    waiting_revision: Option<(u64, usize)>,
    pending: Option<PendingSubtitle>,
    refinement: Option<super::subtitle_refinement::SubtitleRefinement>,
}

struct PendingSubtitle {
    translation: SubtitleTranslation,
    // The current cue and its bounded neighbors. Appended ASR does not
    // invalidate a request, but edits to facts already used by it do.
    inputs: Vec<SubtitleCue>,
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
        mut languages: LanguageSelection,
    ) -> TranslationTask {
        // Short recognized sentences may be too ambiguous to detect again.
        // Reuse their known language until the user changes the actual source.
        if text.trim() == cues[index].original_text.trim()
            && let LanguageMode::Detect { target } = languages.mode
            && let Some(source) = SupportedLanguage::parse(&cues[index].source_language)
        {
            languages.mode = LanguageMode::Fixed { source, target };
        }
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
        .with_text_workload(xrtranslate_protocol::InferenceWorkload::Offline)
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
            refinement: None,
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
        refine: bool,
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
            refinement: refine.then(Default::default),
        });
        Ok(())
    }

    pub(crate) fn take_stop_request(&mut self) -> bool {
        std::mem::take(&mut self.stop_requested)
    }

    pub(super) fn subtitle_source_open(&self) -> bool {
        self.subtitle_job
            .as_ref()
            .is_some_and(|job| job.source_open)
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
        let timeline = &self.controller.subtitles;
        let ready = job
            .refinement
            .as_ref()
            .map_or(timeline.count(), |refinement| {
                refinement.ready().min(timeline.count())
            });
        let revision = (timeline.revision(), ready);
        let source_open = job.source_open || ready < timeline.count();
        if source_open && job.waiting_revision == Some(revision) {
            return None;
        }
        let cues = &timeline.cues()[..ready];
        let Some(index) = cues.iter().position(needs_translation) else {
            if !source_open {
                self.subtitle_job = None;
                self.controller.pause_task();
            } else {
                self.subtitle_job.as_mut()?.waiting_revision = Some(revision);
            }
            return None;
        };
        if !cue_ready(timeline, index, ready, source_open) {
            self.subtitle_job.as_mut()?.waiting_revision = Some(revision);
            return None;
        }
        let translation = SubtitleTranslation::new(&cues[index].id);
        let source = cues[index].original_text.clone();
        let mut task = translation.task(cues, index, source, job.languages);
        let binding = task.plugin.as_mut()?;
        binding.output_policy = job.output_policy;
        if job.output_policy == SessionOutputPolicy::Host {
            binding.owner = binding
                .owner
                .clone()
                .with_audio_source(crate::CaptureSource::SystemAudio);
        }
        let inputs = cues[context_range(cues, index)].to_vec();
        self.subtitle_job.as_mut()?.pending = Some(PendingSubtitle {
            translation,
            inputs,
        });
        Some(task)
    }

    pub(crate) fn poll_subtitle_refinement(
        &mut self,
        ask: &mut crate::ask::Ask,
        ctx: &eframe::egui::Context,
    ) {
        let Some(job) = &mut self.subtitle_job else {
            return;
        };
        if self.controller.active_task_id.as_deref() != Some(&job.task) {
            return;
        }
        if let Some(refinement) = &mut job.refinement {
            let revision = self.controller.subtitles.revision();
            if refinement.poll(
                &mut self.controller.subtitles,
                job.source_open,
                |prompt| ask.request(prompt, ctx),
                ctx,
            ) {
                job.refinement = None;
            }
            self.controller.dirty |= self.controller.subtitles.revision() != revision;
        }
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
        let Some(pending) = &mut job.pending else {
            return false;
        };
        if pending.translation.operation != operation {
            return false;
        }
        self.controller.previews.observe(event);
        let Some(result) = pending.translation.accept(event) else {
            return true;
        };
        let pending = job.pending.take().unwrap();
        match result {
            Ok(result) => {
                let cues = self.controller.subtitles.cues();
                let start = cues.iter().position(|cue| cue.id == pending.inputs[0].id);
                if start.is_some_and(|start| {
                    cues.get(start..start + pending.inputs.len()) == Some(pending.inputs.as_slice())
                }) {
                    self.controller.subtitles.set_translation(
                        &pending.translation.cue,
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

fn cue_ready(timeline: &SubtitleTimeline, index: usize, ready: usize, source_open: bool) -> bool {
    if !source_open {
        return true;
    }
    let cues = timeline.cues();
    for position in index..=index + CONTEXT_CUES {
        let Some(cue) = cues.get(position) else {
            return false;
        };
        // A known scene break closes lookahead even if the next scene has
        // not been refined yet. That scene is not reference for this cue.
        if position > index
            && cue.start_ms.saturating_sub(cues[position - 1].end_ms) > CONTEXT_GAP_MS
        {
            return true;
        }
        let metadata = timeline.metadata_for(&cue.id);
        if position >= ready || !metadata.finalized || metadata.revisable {
            return false;
        }
    }
    true
}

pub(super) fn subtitle_context(cues: &[SubtitleCue], index: usize) -> TaskContext {
    // Keep both bilingual history and source lookahead useful within the shared
    // prompt budget. Do not let a long neighboring cue crowd out every turn.
    const BEFORE_CHARS: usize = 160;
    const AFTER_CHARS: usize = 240;
    const HISTORY_CHARS: usize = 480;
    let window = context_range(cues, index);
    let mut recent_turns = Vec::new();
    let mut source_before = Vec::new();
    let mut history_left = HISTORY_CHARS;
    let mut before_left = BEFORE_CHARS;
    for cue in cues[window.start..index].iter().rev() {
        if let (Some(translated), Some(source), Some(target)) = (
            cue.translated_text
                .as_deref()
                .map(str::trim)
                .filter(|text| !text.is_empty()),
            SupportedLanguage::parse(&cue.source_language),
            SupportedLanguage::parse(&cue.target_language),
        ) && let cost = cue
            .original_text
            .chars()
            .chain(translated.chars())
            .take(HISTORY_CHARS + 1)
            .count()
            + 64
            && cost <= history_left
        {
            history_left -= cost;
            recent_turns.push(PromptTurn {
                turn_id: Some(cue.id.clone()),
                speaker_id: cue.speaker_name.clone().unwrap_or_default(),
                source_language: source.code().to_owned(),
                target_language: target.code().to_owned(),
                source_text: cue.original_text.clone(),
                translated_text: translated.to_owned(),
            });
        } else if before_left > 0 {
            let text = context_excerpt(&cue.original_text, before_left, true);
            before_left = before_left.saturating_sub(text.chars().count() + 1);
            source_before.push(text);
        }
    }
    recent_turns.reverse();
    source_before.reverse();
    let mut after = String::new();
    let mut after_left = AFTER_CHARS;
    for cue in &cues[index + 1..window.end] {
        if after_left == 0 {
            break;
        }
        let text = context_excerpt(&cue.original_text, after_left, false);
        after_left = after_left.saturating_sub(text.chars().count() + 1);
        if !after.is_empty() {
            after.push('\n');
        }
        after.push_str(&text);
    }
    TaskContext::snapshot(ContextFacts {
        recent_turns,
        surrounding_source: Some(SurroundingSource {
            speaker_id: cues[index].speaker_name.clone().unwrap_or_default(),
            source_language: SupportedLanguage::parse(&cues[index].source_language)
                .map_or_else(String::new, |language| language.code().to_owned()),
            before: source_before.join("\n"),
            after,
        }),
    })
}

fn context_range(cues: &[SubtitleCue], index: usize) -> std::ops::Range<usize> {
    let mut start = index;
    while start > index.saturating_sub(PREVIOUS_CUES)
        && cues[start].start_ms.saturating_sub(cues[start - 1].end_ms) <= CONTEXT_GAP_MS
    {
        start -= 1;
    }
    let mut end = index + 1;
    while end < (index + 1 + FOLLOWING_CUES).min(cues.len())
        && cues[end].start_ms.saturating_sub(cues[end - 1].end_ms) <= CONTEXT_GAP_MS
    {
        end += 1;
    }
    start..end
}

fn context_excerpt(text: &str, limit: usize, tail: bool) -> String {
    if tail {
        text.chars()
            .rev()
            .take(limit)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    } else {
        text.chars().take(limit).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::media::subtitles::SubtitleTimeline;

    #[test]
    fn unchanged_subtitles_reuse_known_language_without_overriding_language_intent() {
        let timeline =
            SubtitleTimeline::parse("1\n00:00:00,000 --> 00:00:01,000\nRead aloud.").unwrap();
        let mut cue = timeline.cues()[0].clone();
        cue.source_language = "en".into();
        let selected =
            LanguageSelection::parse_with_options("auto", "zh", Some("ja"), false).unwrap();
        let request = |cue: &SubtitleCue, text: &str, languages| {
            SubtitleTranslation::new(&cue.id)
                .task(std::slice::from_ref(cue), 0, text.into(), languages)
                .languages
        };
        let recognized = request(&cue, "Read aloud.", selected);
        assert_eq!(recognized.wire(), ("en".into(), "zh".into()));
        assert_eq!(recognized.additional_target, selected.additional_target);
        assert_eq!(request(&cue, "新的原文。", selected), selected);
        for languages in [
            LanguageSelection::parse("ja", "zh").unwrap(),
            LanguageSelection::parse("auto", "en,zh").unwrap(),
        ] {
            assert_eq!(request(&cue, "Read aloud.", languages), languages);
        }
        for unknown in ["", "auto", "und"] {
            cue.source_language = unknown.into();
            assert_eq!(request(&cue, "Read aloud.", selected), selected);
        }
    }

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
        assert!(facts.recent_turns[0].source_text.contains("left"));
        assert!(facts.surrounding_source.unwrap().before.is_empty());

        timeline.clear_translations();
        timeline.set_translation("srt_1", "Old translation".into(), "", "");
        let TaskContext::Snapshot { facts } = subtitle_context(timeline.cues(), 2) else {
            unreachable!();
        };
        assert!(facts.recent_turns.is_empty());
        assert_eq!(
            facts.surrounding_source.unwrap().before,
            "The entrance is on the left.\n我们会等你。"
        );
        assert!(timeline.cues()[0].source_language.is_empty());
        assert!(timeline.cues()[0].target_language.is_empty());

        timeline.set_translation("srt_0", "入口在左边。".into(), "en", "zh");
        let TaskContext::Snapshot { facts } = subtitle_context(timeline.cues(), 2) else {
            unreachable!();
        };
        assert_eq!(facts.recent_turns.len(), 1);
        assert_eq!(facts.surrounding_source.unwrap().before, "我们会等你。");
    }

    #[test]
    fn identical_neighboring_cues_remain_distinct_facts() {
        let mut timeline = SubtitleTimeline::parse(
            "1\n00:00:00,000 --> 00:00:01,000\nAgain.\n\n\
             2\n00:00:01,000 --> 00:00:02,000\nAgain.\n\n\
             3\n00:00:02,000 --> 00:00:03,000\nNow.\n\n\
             4\n00:00:03,000 --> 00:00:04,000\nLater.",
        )
        .unwrap();
        for id in ["srt_0", "srt_1"] {
            timeline.set_translation(id, "再一次。".into(), "en", "zh");
        }
        let TaskContext::Snapshot { facts } = subtitle_context(timeline.cues(), 2) else {
            unreachable!();
        };
        assert_eq!(facts.recent_turns.len(), 2);
        assert_ne!(facts.recent_turns[0].turn_id, facts.recent_turns[1].turn_id);
        let neighbors = facts.surrounding_source.unwrap();
        assert!(neighbors.before.is_empty());
        assert_eq!(neighbors.after, "Later.");
    }

    #[test]
    fn long_neighbors_leave_room_for_bilingual_history_and_scene_breaks_cut_context() {
        let mut timeline = SubtitleTimeline::parse(
            "1\n00:00:00,000 --> 00:00:01,000\nHer name is Mira.\n\n\
             2\n00:00:01,000 --> 00:00:02,000\nShe is here.\n\n\
             3\n00:00:02,000 --> 00:00:03,000\nNext.",
        )
        .unwrap();
        timeline.set_translation("srt_0", "她叫米拉。".into(), "en", "zh");
        let mut long = timeline.cues()[2].clone();
        long.original_text = "后文".repeat(2_000);
        timeline.edit_cue(long.clone()).unwrap();
        let TaskContext::Snapshot { facts } = subtitle_context(timeline.cues(), 1) else {
            unreachable!()
        };
        assert_eq!(facts.recent_turns.len(), 1);
        assert_eq!(facts.recent_turns[0].translated_text, "她叫米拉。");
        assert_eq!(facts.surrounding_source.unwrap().after.chars().count(), 240);

        long.start_ms = 60_000;
        long.end_ms = 61_000;
        timeline.edit_cue(long).unwrap();
        let TaskContext::Snapshot { facts } = subtitle_context(timeline.cues(), 2) else {
            unreachable!()
        };
        assert!(facts.recent_turns.is_empty());
        assert!(facts.surrounding_source.unwrap().before.is_empty());
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
            .start_file_subtitles(LanguageSelection::parse("en", "zh").unwrap(), false)
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

    // Deterministic stand-in for Ask: echo unchanged source, which must still
    // advance readiness without relying on a timeline revision changing.
    fn refine(plugin: &mut MediaPlugin) {
        let job = plugin.subtitle_job.as_mut().unwrap();
        let refinement = job.refinement.as_mut().unwrap();
        if refinement.poll(
            &mut plugin.controller.subtitles,
            job.source_open,
            |prompt| {
                let (tx, rx) = crossbeam_channel::bounded(1);
                tx.send(Ok(prompt.rsplit("\n\n").next().unwrap().to_owned()))
                    .unwrap();
                rx
            },
            &eframe::egui::Context::default(),
        ) {
            job.refinement = None;
        }
    }

    #[test]
    fn refined_file_starts_translation_before_eof_and_flushes_its_unmodified_tail() {
        let (mut plugin, _storage) = fixture();
        plugin.subtitle_job.as_mut().unwrap().refinement = Some(Default::default());
        for index in 0..6 {
            add_source(&mut plugin, index, true);
        }
        assert!(matches!(
            plugin.translation_context(),
            TaskContext::Snapshot { .. }
        ));
        assert!(plugin.next_subtitle_task().is_none());
        let revision = plugin.controller.subtitles.revision();
        refine(&mut plugin);
        assert!(plugin.next_subtitle_task().is_none());
        refine(&mut plugin);
        assert_eq!(revision, plugin.controller.subtitles.revision());
        assert_eq!(
            plugin
                .subtitle_job
                .as_ref()
                .unwrap()
                .refinement
                .as_ref()
                .unwrap()
                .ready(),
            5
        );
        let first = plugin.next_subtitle_task().unwrap();
        assert!(plugin.subtitle_source_open());
        assert_eq!(
            first.text_workload,
            Some(xrtranslate_protocol::InferenceWorkload::Offline)
        );
        assert!(plugin.next_subtitle_task().is_none());
        complete(&mut plugin, &first);
        plugin.finish_subtitle_source(&TranslationOutcome::Completed);
        refine(&mut plugin);
        refine(&mut plugin);
        assert!(plugin.subtitle_job.as_ref().unwrap().refinement.is_none());
        while let Some(request) = plugin.next_subtitle_task() {
            complete(&mut plugin, &request);
        }
        assert!(plugin.subtitle_job.is_none());
        assert_eq!(plugin.controller.subtitles.count(), 6);
        assert!(
            plugin
                .controller
                .subtitles
                .cues()
                .iter()
                .all(|cue| !needs_translation(cue))
        );
    }

    #[test]
    fn authored_subtitle_translation_keeps_cue_boundaries_and_reads_fresh_history() {
        let (mut plugin, _storage) = fixture();
        let id = plugin.controller.active_task_id.clone().unwrap();
        plugin.controller.store.get_mut(&id).unwrap().media_type =
            super::super::task::MediaType::Subtitles;
        plugin.controller.subtitles = SubtitleTimeline::parse(
            "1\n00:00:00,125 --> 00:00:01,900\nI think\n\n\
             2\n00:00:01,900 --> 00:00:03,200\nwe should leave.",
        )
        .unwrap();
        let original = plugin.controller.subtitles.cues().to_vec();
        plugin
            .start_subtitles(LanguageCapabilities::default(), true)
            .unwrap();
        assert!(plugin.subtitle_job.as_ref().unwrap().refinement.is_none());
        let first = plugin.next_subtitle_task().unwrap();
        let TaskContext::Snapshot { facts } = &first.context else {
            unreachable!()
        };
        assert_eq!(
            facts.surrounding_source.as_ref().unwrap().after,
            "we should leave."
        );
        complete(&mut plugin, &first);
        let second = plugin.next_subtitle_task().unwrap();
        let TaskContext::Snapshot { facts } = &second.context else {
            unreachable!()
        };
        assert_eq!(facts.recent_turns[0].source_text, "I think");
        complete(&mut plugin, &second);
        assert!(plugin.next_subtitle_task().is_none());
        for (before, after) in original.iter().zip(plugin.controller.subtitles.cues()) {
            assert_eq!(
                (
                    &before.id,
                    before.start_ms,
                    before.end_ms,
                    &before.original_text
                ),
                (
                    &after.id,
                    after.start_ms,
                    after.end_ms,
                    &after.original_text
                )
            );
        }
    }

    #[test]
    fn file_source_waits_for_atomic_final_turns_before_refinement() {
        use crate::session_coordinator::{SessionEventSubscriber, TranslationSessionOwner};
        let (mut plugin, _storage) = fixture();
        let operation = plugin.controller.active_task_id.clone().unwrap();
        let owner = TranslationSessionOwner::Plugin(PluginSessionOwner::new(
            super::super::super::PluginId::MEDIA.as_str(),
            &operation,
            "Media",
            "Media",
            "Media",
        ));
        let mut segment = TranslationSegment {
            stream_id: 7,
            audio_source: crate::CaptureSource::SystemAudio,
            turn_id: "turn".into(),
            segment_index: 1,
            segment_count: 1,
            source: "Unfinished".into(),
            translated: Some("Unfinished".into()),
            source_lang: "en".into(),
            target_lang: "en".into(),
            additional_translations: Vec::new(),
            asr_only: true,
            speaker_id: String::new(),
            source_start_ms: 0.0,
            source_end_ms: 1_000.0,
            timing: Default::default(),
            boundary: Default::default(),
            revisable: true,
            live: true,
        };
        plugin.event_sink.on_translation_event(
            &owner,
            &TranslationEvent::ReplaceSegments(vec![segment.clone()]),
        );
        plugin.poll_translation_events();
        assert!(plugin.controller.subtitles.cues().is_empty());
        segment.source = "Finished sentence.".into();
        segment.translated = Some(segment.source.clone());
        segment.revisable = false;
        segment.live = false;
        plugin
            .event_sink
            .on_translation_event(&owner, &TranslationEvent::ReplaceSegments(vec![segment]));
        plugin.poll_translation_events();
        assert_eq!(
            plugin.controller.subtitles.cues()[0].original_text,
            "Finished sentence."
        );
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
    fn appended_source_keeps_pending_work_but_edits_to_its_context_require_retry() {
        let (mut plugin, _storage) = fixture();
        for index in 0..3 {
            add_source(&mut plugin, index, true);
        }
        let first = plugin.next_subtitle_task().unwrap();
        add_source(&mut plugin, 3, true);
        complete(&mut plugin, &first);
        assert!(!needs_translation(&plugin.controller.subtitles.cues()[0]));

        let second = plugin.next_subtitle_task().unwrap();
        let mut edited = plugin.controller.subtitles.cues()[0].clone();
        edited.translated_text = Some("更正后的译文。".into());
        plugin.controller.subtitles.edit_cue(edited).unwrap();
        complete(&mut plugin, &second);
        assert!(needs_translation(&plugin.controller.subtitles.cues()[1]));
        let retry = plugin.next_subtitle_task().unwrap();
        let TaskContext::Snapshot { facts } = &retry.context else {
            unreachable!()
        };
        assert_eq!(facts.recent_turns[0].translated_text, "更正后的译文。");
        complete(&mut plugin, &retry);
        assert!(!needs_translation(&plugin.controller.subtitles.cues()[1]));
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
