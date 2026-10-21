//! Meeting-owned translation scheduling over stable, durable recognition facts.
use super::{
    controller::ActiveMeetingCapture,
    store::{MeetingStore, Segment, TranslationWindow},
};
use crate::session_coordinator::{
    PluginSessionBinding, PluginSessionOwner, SessionOutputPolicy, TextTranslationCollector,
    TranslationEvent, TranslationTask,
};
use std::time::{Duration, Instant};
use xrtranslate_engine::language::{LanguageMode, LanguageSelection, SupportedLanguage};
use xrtranslate_prompt::{ContextFacts, PromptTurn, SurroundingSource, TaskContext};

const LOOKAHEAD_WAIT: Duration = Duration::from_secs(2);

pub(super) struct MeetingTranslation {
    pub capture: ActiveMeetingCapture,
    languages: LanguageSelection,
    pending: Option<Pending>,
    waiting: Option<(String, Instant)>,
    dirty: bool,
    pub closed: bool,
    pub paused: bool,
    pub error: Option<String>,
}

struct Pending {
    operation: String,
    source: Segment,
    result: TextTranslationCollector,
}

impl MeetingTranslation {
    pub fn new(capture: ActiveMeetingCapture, languages: LanguageSelection) -> Self {
        Self {
            capture,
            languages,
            pending: None,
            waiting: None,
            dirty: true,
            closed: false,
            paused: false,
            error: None,
        }
    }

    pub fn changed(&mut self) {
        self.dirty = true;
    }

    pub fn next(
        &mut self,
        store: &MeetingStore,
        now: Instant,
    ) -> Result<Option<TranslationTask>, String> {
        if self.pending.is_some() || self.error.is_some() || (!self.dirty && self.waiting.is_none())
        {
            return Ok(None);
        }
        self.dirty = false;
        let Some(window) = store
            .translation_window(
                &self.capture.meeting_id,
                &self.capture.recognition_run_id,
                self.capture.imported_audio,
            )
            .map_err(|e| e.to_string())?
        else {
            self.waiting = None;
            return Ok(None);
        };
        let since = match &self.waiting {
            Some((id, since)) if id == &window.current.id => *since,
            _ => {
                self.waiting = Some((window.current.id.clone(), now));
                now
            }
        };
        // Live input must never wait indefinitely for a reply. Files can look
        // two records ahead without an artificial playback-speed restriction.
        let ready = self.closed
            || self.paused
            || if self.capture.imported_audio {
                window.after.len() >= 2
            } else {
                !window.after.is_empty() || now.saturating_duration_since(since) >= LOOKAHEAD_WAIT
            };
        if !ready {
            return Ok(None);
        }
        let operation = format!(
            "{}/translate/{}",
            self.capture.recognition_run_id,
            uuid::Uuid::new_v4()
        );
        let owner = PluginSessionOwner::new(
            super::super::PluginId::MEETING.as_str(),
            &operation,
            "Meeting Notes",
            "Open meeting controls",
            "Translating meeting",
        )
        .with_history(false);
        let task = TranslationTask::text(
            window.current.original_text.clone(),
            languages_for_source(self.languages, &window.current.source_language),
            Some(PluginSessionBinding::text(
                owner,
                SessionOutputPolicy::PluginOnly,
            )),
        )
        .with_context(meeting_context(&window))
        .with_text_workload(xrtranslate_protocol::InferenceWorkload::Offline);
        self.pending = Some(Pending {
            operation,
            source: window.current,
            result: TextTranslationCollector::default(),
        });
        self.waiting = None;
        Ok(Some(task))
    }

    pub fn accept(
        &mut self,
        store: &MeetingStore,
        operation: &str,
        event: &TranslationEvent,
    ) -> Result<(), String> {
        let Some(pending) = &mut self.pending else {
            return Ok(());
        };
        if pending.operation != operation {
            return Ok(());
        }
        let Some(result) = pending.result.accept(event) else {
            return Ok(());
        };
        let source = self.pending.take().unwrap().source;
        let result = result?;
        store
            .complete_translation(
                &source,
                &result.text,
                &result.source_language,
                &result.target_language,
            )
            .map_err(|e| e.to_string())?;
        self.dirty = true;
        Ok(())
    }

    pub fn finished(&self) -> bool {
        self.closed
            && (self.error.is_some()
                || (!self.dirty && self.waiting.is_none() && self.pending.is_none()))
    }
}

fn languages_for_source(mut languages: LanguageSelection, source: &str) -> LanguageSelection {
    let Some(source) = SupportedLanguage::parse(source) else {
        return languages;
    };
    let target = match languages.mode {
        LanguageMode::Fixed { target, .. } | LanguageMode::Detect { target } => target,
        LanguageMode::Bidirectional([first, second]) => {
            if source.base_code() == first.base_code() {
                second
            } else {
                first
            }
        }
        LanguageMode::AsrOnly { .. } => return languages,
    };
    languages.mode = LanguageMode::Fixed { source, target };
    languages
}

fn speaker(segment: &Segment) -> String {
    // Canonical IDs survive user merges; anonymous IDs are scoped to their run
    // and input, never inferred to be the same person after a restart.
    segment.canonical_speaker_id.clone().unwrap_or_else(|| {
        format!(
            "{}/{}",
            segment.recognition_run_id,
            segment
                .speaker_token
                .as_deref()
                .unwrap_or(segment.source.as_str())
        )
    })
}

fn source_reference(
    segment: &Segment,
    speaker: &str,
    budget: &mut usize,
    tail: bool,
) -> Option<String> {
    let label = format!("[{speaker} {}] ", segment.source_language);
    let available = budget.saturating_sub(label.chars().count() + 1);
    if available == 0 {
        return None;
    }
    let text = if tail {
        segment
            .original_text
            .chars()
            .rev()
            .take(available)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<String>()
    } else {
        segment.original_text.chars().take(available).collect()
    };
    let line = format!("{label}{text}");
    *budget = budget.saturating_sub(line.chars().count() + 1);
    Some(line)
}

fn meeting_context(window: &TranslationWindow) -> TaskContext {
    // Short aliases keep identity consistent within this snapshot without
    // spending the small reference budget on database UUIDs.
    let mut speakers = std::collections::HashMap::new();
    for segment in window
        .before
        .iter()
        .chain(std::iter::once(&window.current))
        .chain(&window.after)
    {
        let next = speakers.len() + 1;
        speakers
            .entry(speaker(segment))
            .or_insert_with(|| format!("speaker-{next}"));
    }
    let mut recent_turns = Vec::new();
    let mut before = Vec::new();
    let mut history_left: usize = 480;
    let mut before_left = 160;
    for segment in window.before.iter().rev() {
        let label = &speakers[&speaker(segment)];
        let cost = segment.original_text.chars().count()
            + segment
                .translated_text
                .as_deref()
                .unwrap_or_default()
                .chars()
                .count()
            + label.len() * 2
            + 32;
        if let (Some(text), Some(source), Some(target)) = (
            segment
                .translated_text
                .as_deref()
                .filter(|s| !s.trim().is_empty()),
            SupportedLanguage::parse(&segment.source_language),
            SupportedLanguage::parse(&segment.target_language),
        ) && source != target
            && cost <= history_left
        {
            history_left -= cost;
            recent_turns.push(PromptTurn {
                turn_id: Some(segment.id.clone()),
                speaker_id: label.clone(),
                source_language: source.code().into(),
                target_language: target.code().into(),
                source_text: segment.original_text.clone(),
                translated_text: text.into(),
            });
        } else if let Some(line) = source_reference(segment, label, &mut before_left, true) {
            before.push(line);
        }
    }
    recent_turns.reverse();
    before.reverse();
    let mut after_left = 240;
    let after = window
        .after
        .iter()
        .filter_map(|s| source_reference(s, &speakers[&speaker(s)], &mut after_left, false))
        .collect::<Vec<_>>();
    TaskContext::snapshot(ContextFacts {
        current_speaker_id: speakers[&speaker(&window.current)].clone(),
        recent_turns,
        surrounding_source: Some(SurroundingSource {
            speaker_id: String::new(),
            source_language: String::new(),
            before: before.join("\n"),
            after: after.join("\n"),
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::super::store::{MarkerKind, NewMeeting, NewSegment, SegmentSource};
    use super::*;
    use crate::{
        CaptureSource,
        session_coordinator::{TranslationInput, TranslationOutcome, TranslationSegment},
    };

    fn fixture(imported: bool) -> (MeetingStore, ActiveMeetingCapture, MeetingTranslation) {
        let store = MeetingStore::open_in_memory().unwrap();
        let bundle = store
            .create_meeting(NewMeeting::live("Meeting", None, "auto", "zh"))
            .unwrap();
        let capture = ActiveMeetingCapture {
            meeting_id: bundle.meeting.id,
            topic_id: bundle.topics[0].id.clone(),
            recognition_run_id: "run".into(),
            timeline_offset_ms: 0,
            imported_audio: imported,
        };
        let job = MeetingTranslation::new(
            capture.clone(),
            LanguageSelection::parse_with_options("auto", "zh", None, false).unwrap(),
        );
        (store, capture, job)
    }

    fn source(
        store: &MeetingStore,
        capture: &ActiveMeetingCapture,
        text: &str,
        start: i64,
        system: bool,
        finalized: bool,
    ) -> Segment {
        store
            .upsert_segment(NewSegment {
                meeting_id: capture.meeting_id.clone(),
                external_key: format!("{}:{start}:{system}", capture.recognition_run_id),
                topic_id: capture.topic_id.clone(),
                original_text: text.into(),
                translated_text: None,
                source_language: "en".into(),
                target_language: String::new(),
                start_ms: start,
                end_ms: start + 500,
                source: if system {
                    SegmentSource::SystemAudio
                } else {
                    SegmentSource::Microphone
                },
                recognition_run_id: capture.recognition_run_id.clone(),
                speaker_token: Some(if system { "system:B" } else { "mic:A" }.into()),
                is_final: finalized,
            })
            .unwrap()
    }

    fn translated(
        job: &mut MeetingTranslation,
        store: &MeetingStore,
        task: &TranslationTask,
        text: &str,
    ) {
        let operation = task.owner().operation_id().unwrap().to_owned();
        let segment = TranslationSegment {
            stream_id: 99,
            audio_source: CaptureSource::Microphone,
            turn_id: "text-request".into(),
            segment_index: 1,
            segment_count: 1,
            source: "source".into(),
            translated: Some(text.into()),
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
        job.accept(store, &operation, &TranslationEvent::Segment(segment))
            .unwrap();
        job.accept(
            store,
            &operation,
            &TranslationEvent::Finished {
                stream_id: 99,
                outcome: TranslationOutcome::Completed,
            },
        )
        .unwrap();
    }

    #[test]
    fn live_tail_has_a_deadline_and_uses_the_recognized_language() {
        let (store, capture, mut job) = fixture(false);
        source(&store, &capture, "First.", 0, false, true);
        source(&store, &capture, "Unstable preview", 1000, false, false);
        let now = Instant::now();
        assert!(job.next(&store, now).unwrap().is_none());
        assert!(
            job.next(&store, now + Duration::from_millis(1999))
                .unwrap()
                .is_none()
        );
        let task = job.next(&store, now + LOOKAHEAD_WAIT).unwrap().unwrap();
        assert_eq!(task.languages.wire(), ("en".into(), "zh".into()));
        assert!(matches!(task.input, TranslationInput::Text(ref text) if text == "First."));
        assert!(job.next(&store, now + LOOKAHEAD_WAIT).unwrap().is_none());
        let TaskContext::Snapshot { facts } = &task.context else {
            panic!()
        };
        assert!(
            !facts
                .surrounding_source
                .as_ref()
                .unwrap()
                .after
                .contains("Unstable")
        );
    }

    #[test]
    fn late_earlier_speaker_is_ordered_before_the_reply_and_preserves_annotations() {
        let (store, capture, mut job) = fixture(false);
        source(&store, &capture, "The reply.", 2000, true, true);
        let now = Instant::now();
        assert!(job.next(&store, now).unwrap().is_none());
        let first = source(&store, &capture, "The question?", 0, false, true);
        let marker = store
            .add_marker(&first.id, MarkerKind::Note, "Keep this")
            .unwrap();
        job.changed();
        let task = job.next(&store, now).unwrap().unwrap();
        assert!(matches!(task.input, TranslationInput::Text(ref text) if text == "The question?"));
        let TaskContext::Snapshot { facts } = &task.context else {
            panic!()
        };
        assert_eq!(facts.current_speaker_id, "speaker-1");
        let after = facts.surrounding_source.as_ref().unwrap();
        assert!(after.speaker_id.is_empty());
        assert!(after.after.contains("speaker-2"));
        assert!(after.after.contains("The reply."));
        translated(&mut job, &store, &task, "问题？");
        let saved = store.open_meeting(&capture.meeting_id).unwrap();
        assert_eq!(saved.segments.len(), 2);
        assert_eq!(saved.markers, vec![marker]);
        let saved_first = saved.segments.iter().find(|s| s.id == first.id).unwrap();
        assert_eq!(saved_first.start_ms, first.start_ms);
        assert_eq!(saved_first.topic_id, first.topic_id);
        assert_eq!(saved_first.speaker_token, first.speaker_token);
        assert_eq!(saved_first.translated_text.as_deref(), Some("问题？"));
        job.paused = true;
        let reply = job.next(&store, now).unwrap().unwrap();
        let TaskContext::Snapshot { facts } = reply.context else {
            panic!()
        };
        assert_eq!(facts.recent_turns.len(), 1);
        assert_eq!(facts.recent_turns[0].source_text, "The question?");
        assert_eq!(facts.recent_turns[0].translated_text, "问题？");
    }

    #[test]
    fn file_lookahead_flushes_at_eof_and_finishes_only_after_translation() {
        let (store, capture, mut job) = fixture(true);
        source(&store, &capture, "One.", 0, false, true);
        source(&store, &capture, "Two.", 1000, false, true);
        let now = Instant::now();
        assert!(job.next(&store, now).unwrap().is_none());
        assert!(
            job.next(&store, now + Duration::from_secs(100))
                .unwrap()
                .is_none()
        );
        source(&store, &capture, "Three.", 2000, false, true);
        job.changed();
        let first = job.next(&store, now).unwrap().unwrap();
        job.closed = true;
        job.changed();
        assert!(!job.finished());
        translated(&mut job, &store, &first, "一。");
        for text in ["二。", "三。"] {
            let next = job.next(&store, now).unwrap().unwrap();
            assert!(!job.finished());
            translated(&mut job, &store, &next, text);
        }
        assert!(job.next(&store, now).unwrap().is_none());
        assert!(job.finished());
        assert!(
            store
                .list_segments(&capture.meeting_id)
                .unwrap()
                .iter()
                .all(|s| s.translated_text.is_some())
        );
    }

    #[test]
    fn resumed_history_is_chronological_but_reprocessing_starts_fresh() {
        let (store, capture, _) = fixture(false);
        let previous = ActiveMeetingCapture {
            recognition_run_id: "old-run".into(),
            ..capture.clone()
        };
        let later = source(&store, &previous, "Later.", 1000, true, true);
        let earlier = source(&store, &previous, "Earlier.", 0, false, true);
        store
            .complete_translation(&later, "之后。", "en", "zh")
            .unwrap();
        store
            .complete_translation(&earlier, "之前。", "en", "zh")
            .unwrap();
        source(&store, &previous, "Source only.", 1500, true, true);
        source(&store, &previous, "Preview.", 1800, false, false);
        source(&store, &capture, "Current.", 2000, false, true);
        let window = store
            .translation_window(&capture.meeting_id, "run", false)
            .unwrap()
            .unwrap();
        let TaskContext::Snapshot { facts } = meeting_context(&window) else {
            panic!()
        };
        assert_eq!(
            facts
                .recent_turns
                .iter()
                .map(|s| s.source_text.as_str())
                .collect::<Vec<_>>(),
            ["Earlier.", "Later."]
        );
        assert!(
            facts
                .surrounding_source
                .unwrap()
                .before
                .contains("Source only.")
        );
        let window = store
            .translation_window(&capture.meeting_id, "run", true)
            .unwrap()
            .unwrap();
        assert!(window.before.is_empty());
    }

    #[test]
    fn stale_translation_does_not_overwrite_a_revised_source() {
        let (store, capture, mut job) = fixture(false);
        source(&store, &capture, "Original.", 0, false, true);
        job.paused = true;
        let task = job.next(&store, Instant::now()).unwrap().unwrap();
        source(&store, &capture, "Corrected.", 0, false, true);
        translated(&mut job, &store, &task, "旧的译文。");
        let row = &store.list_segments(&capture.meeting_id).unwrap()[0];
        assert_eq!(row.original_text, "Corrected.");
        assert!(row.translated_text.is_none());
        let next = job.next(&store, Instant::now()).unwrap().unwrap();
        assert!(matches!(next.input, TranslationInput::Text(ref text) if text == "Corrected."));
    }

    #[test]
    fn known_bidirectional_source_selects_the_actual_target() {
        let languages =
            LanguageSelection::parse_with_options("auto", "en,zh", None, false).unwrap();
        assert_eq!(
            languages_for_source(languages, "en").wire(),
            ("en".into(), "zh".into())
        );
        assert_eq!(
            languages_for_source(languages, "zh").wire(),
            ("zh".into(), "en".into())
        );
    }
}
