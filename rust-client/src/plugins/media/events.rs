use crossbeam_channel::{Receiver, Sender, unbounded};

use crate::session_coordinator::{
    SessionEventSubscriber, TranslationEvent, TranslationOutcome, TranslationSegment,
    TranslationSessionOwner,
};

use super::{
    MediaPlugin,
    subtitles::{SubtitleCue, SubtitleMetadata, TranslationCueInput, cue_from_translation},
};

#[derive(Clone)]
pub struct MediaTranslationSink {
    tx: Sender<(String, TranslationEvent)>,
    rx: Receiver<(String, TranslationEvent)>,
}

impl Default for MediaTranslationSink {
    fn default() -> Self {
        let (tx, rx) = unbounded();
        Self { tx, rx }
    }
}

impl SessionEventSubscriber for MediaTranslationSink {
    fn accepts_owner(&self, owner: &TranslationSessionOwner) -> bool {
        owner.is_plugin(super::super::PluginId::MEDIA.as_str())
    }

    fn on_translation_event(&self, owner: &TranslationSessionOwner, event: &TranslationEvent) {
        if self.accepts_owner(owner)
            && let Some(operation) = owner.operation_id()
        {
            let _ = self.tx.send((operation.to_owned(), event.clone()));
        }
    }
}

impl MediaPlugin {
    pub fn poll_translation_events(&mut self) {
        while let Ok((operation, event)) = self.event_sink.rx.try_recv() {
            if self.accept_retranslation_event(&operation, &event) {
                continue;
            }
            if self.accept_subtitle_event(&operation, &event) {
                continue;
            }
            if self.controller.active_task_id.as_deref() != Some(operation.as_str()) {
                continue;
            }
            self.controller.previews.observe(&event);
            self.controller.dirty = true;
            match event {
                TranslationEvent::Preview(_) => {}
                TranslationEvent::Segment(segment) => {
                    let stream_id = segment.stream_id;
                    if let Some((cue, metadata)) =
                        translation_cue(segment, &self.controller.subtitles)
                    {
                        self.controller.subtitles.finalize_live_cues(stream_id);
                        self.on_translation_cue(cue, metadata);
                    }
                }
                TranslationEvent::ReplaceSegments(segments) => {
                    if let Some(segment) = segments.first() {
                        if segment.asr_only {
                            apply_recognition_snapshot(&mut self.controller.subtitles, segments);
                        } else if segment.translated.is_some() {
                            let stream_id = segment.stream_id;
                            let cues = segments
                                .into_iter()
                                .filter_map(|segment| {
                                    translation_cue(segment, &self.controller.subtitles)
                                })
                                .collect::<Vec<_>>();
                            self.controller
                                .subtitles
                                .replace_stream_cues(stream_id, cues);
                        } else {
                            self.controller.subtitles.retain_source_snapshot(
                                segment.stream_id,
                                &segment.turn_id,
                                segments.iter().map(|segment| {
                                    (segment.segment_index, segment.source.as_str())
                                }),
                            );
                        }
                    }
                }
                TranslationEvent::StreamEnded { stream_id } => {
                    self.controller.subtitles.finish_stream(stream_id);
                }
                TranslationEvent::Finished { stream_id, outcome } => {
                    self.controller.subtitles.finish_stream(stream_id);
                    if !self.finish_subtitle_source(&outcome) {
                        self.controller.pause_task();
                    }
                    if let TranslationOutcome::Failed(error) = outcome {
                        self.set_error(error);
                    }
                }
            }
        }
    }
}

fn apply_recognition_snapshot(
    timeline: &mut super::subtitles::SubtitleTimeline,
    segments: Vec<TranslationSegment>,
) {
    let Some(first) = segments.first() else {
        return;
    };
    timeline.retain_source_snapshot(
        first.stream_id,
        &first.turn_id,
        segments
            .iter()
            .map(|segment| (segment.segment_index, segment.source.as_str())),
    );
    timeline.finalize_live_cues(first.stream_id);
    for segment in segments {
        if let Some((cue, metadata)) = translation_cue(segment, timeline) {
            timeline.add_cue_with_metadata(cue, metadata);
        }
    }
}

fn translation_cue(
    segment: TranslationSegment,
    timeline: &super::subtitles::SubtitleTimeline,
) -> Option<(SubtitleCue, SubtitleMetadata)> {
    let asr_only = segment.asr_only;
    let (mut cue, metadata) = cue_from_translation(TranslationCueInput {
        turn_id: segment.turn_id,
        segment_index: segment.segment_index,
        stream_id: Some(segment.stream_id),
        start_ms: segment.source_start_ms.round() as i64,
        end_ms: segment.source_end_ms.round() as i64,
        speaker_id: segment.speaker_id,
        source: segment.source,
        translated: segment.translated?,
        source_language: segment.source_lang,
        target_language: segment.target_lang,
        timing: segment.timing,
        boundary: segment.boundary,
        revisable: segment.revisable,
        finalized: !segment.live,
    });
    if asr_only {
        cue.translated_text = None;
        cue.target_language.clear();
        // Stable sentences recur in cumulative ASR snapshots. Their translation
        // remains valid until their actual source changes.
        if let Some(previous) = timeline
            .cues()
            .iter()
            .find(|previous| previous.id == cue.id && previous.original_text == cue.original_text)
        {
            cue.translated_text = previous.translated_text.clone();
            if previous.translated_text.is_some() {
                cue.source_language = previous.source_language.clone();
            }
            cue.target_language = previous.target_language.clone();
        }
    }
    Some((cue, metadata))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::media::subtitles::SubtitleTimeline;

    fn recognized(source: &str) -> TranslationSegment {
        TranslationSegment {
            stream_id: 7,
            audio_source: crate::CaptureSource::SystemAudio,
            turn_id: "utterance".into(),
            segment_index: 1,
            segment_count: 1,
            source: source.into(),
            translated: Some(source.into()),
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
            live: false,
        }
    }

    #[test]
    fn recognition_snapshots_keep_matching_translations_and_invalidate_revised_source() {
        let mut timeline = SubtitleTimeline::new();
        let (cue, metadata) = translation_cue(recognized("Hello."), &timeline).unwrap();
        assert!(cue.translated_text.is_none());
        assert_eq!(cue.source_language, "en");
        assert!(cue.target_language.is_empty());
        let id = cue.id.clone();
        timeline.replace_stream_cues(7, [(cue, metadata)]);
        timeline.set_translation(&id, "你好。".into(), "en", "zh");
        let mut repeated = recognized("Hello.");
        repeated.source_lang = "und".into();
        let (cue, metadata) = translation_cue(repeated, &timeline).unwrap();
        assert_eq!(cue.translated_text.as_deref(), Some("你好。"));
        assert_eq!((&*cue.source_language, &*cue.target_language), ("en", "zh"));
        timeline.replace_stream_cues(7, [(cue, metadata)]);
        assert_eq!(timeline.count(), 1);
        let (cue, metadata) = translation_cue(recognized("Hello there."), &timeline).unwrap();
        assert!(cue.translated_text.is_none());
        assert!(cue.target_language.is_empty());
        timeline.replace_stream_cues(7, [(cue, metadata)]);
        assert_eq!(timeline.count(), 1);
        assert_eq!(timeline.cues()[0].original_text, "Hello there.");
    }

    #[test]
    fn recognition_snapshots_replace_their_turn_without_losing_previous_sentences() {
        let mut timeline = SubtitleTimeline::new();
        let first = recognized("First turn.");
        apply_recognition_snapshot(&mut timeline, vec![first.clone()]);
        let first_id = timeline.cues()[0].id.clone();
        timeline.set_translation(&first_id, "第一句。".into(), "en", "zh");
        let mut second = recognized("Second turn.");
        second.turn_id = "second".into();
        second.source_start_ms = 2_000.0;
        second.source_end_ms = 3_000.0;
        apply_recognition_snapshot(&mut timeline, vec![second.clone()]);
        assert_eq!(timeline.count(), 2);
        assert_eq!(
            timeline.cues()[0].translated_text.as_deref(),
            Some("第一句。")
        );
        let mut third = second.clone();
        third.segment_index = 2;
        third.source = "Removed sentence.".into();
        third.translated = Some(third.source.clone());
        third.source_start_ms = 3_000.0;
        third.source_end_ms = 4_000.0;
        apply_recognition_snapshot(&mut timeline, vec![second.clone(), third]);
        assert_eq!(timeline.count(), 3);
        second.source = "Corrected second turn.".into();
        second.translated = Some(second.source.clone());
        apply_recognition_snapshot(&mut timeline, vec![second]);
        assert_eq!(timeline.count(), 2);
        assert_eq!(timeline.cues()[0].original_text, "First turn.");
        assert_eq!(timeline.cues()[1].original_text, "Corrected second turn.");
        assert!(timeline.cues()[1].translated_text.is_none());
    }
}
