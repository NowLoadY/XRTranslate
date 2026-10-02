use crossbeam_channel::{Receiver, Sender, unbounded};

use crate::session_coordinator::{
    SessionEventSubscriber, TranslationEvent, TranslationOutcome, TranslationSegment,
    TranslationSessionOwner,
};

use super::{
    VideoPlayerPlugin,
    subtitles::{SubtitleCue, SubtitleMetadata, TranslationCueInput, cue_from_translation},
};

#[derive(Clone)]
pub struct PlayerTranslationSink {
    tx: Sender<(String, TranslationEvent)>,
    rx: Receiver<(String, TranslationEvent)>,
}

impl Default for PlayerTranslationSink {
    fn default() -> Self {
        let (tx, rx) = unbounded();
        Self { tx, rx }
    }
}

impl SessionEventSubscriber for PlayerTranslationSink {
    fn accepts_owner(&self, owner: &TranslationSessionOwner) -> bool {
        owner.is_plugin(super::super::PluginId::VIDEO_PLAYER.as_str())
    }

    fn on_translation_event(&self, owner: &TranslationSessionOwner, event: &TranslationEvent) {
        if matches!(event, TranslationEvent::Segment(segment) if segment.translated.is_none()) {
            return;
        }
        if self.accepts_owner(owner)
            && let Some(operation) = owner.operation_id()
        {
            let _ = self.tx.send((operation.to_owned(), event.clone()));
        }
    }
}

impl VideoPlayerPlugin {
    pub fn poll_translation_events(&mut self) {
        while let Ok((operation, event)) = self.event_sink.rx.try_recv() {
            if self.controller.active_task_id.as_deref() != Some(operation.as_str()) {
                continue;
            }
            match event {
                TranslationEvent::Segment(segment) => {
                    let stream_id = segment.stream_id;
                    if let Some((cue, metadata)) = translation_cue(segment) {
                        self.controller.subtitles.finalize_live_cues(stream_id);
                        self.on_translation_cue(cue, metadata);
                    }
                }
                TranslationEvent::ReplaceSegments(segments) => {
                    if let Some(segment) = segments.first() {
                        if segment.translated.is_some() {
                            self.controller.subtitles.replace_stream_cues(
                                segment.stream_id,
                                segments.into_iter().filter_map(translation_cue),
                            );
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
                    if let TranslationOutcome::Failed(error) = outcome {
                        self.set_error(error);
                    }
                }
            }
        }
    }
}

fn translation_cue(segment: TranslationSegment) -> Option<(SubtitleCue, SubtitleMetadata)> {
    Some(cue_from_translation(TranslationCueInput {
        turn_id: segment.turn_id,
        segment_index: segment.segment_index,
        stream_id: Some(segment.stream_id),
        start_ms: segment.source_start_ms.round() as i64,
        end_ms: segment.source_end_ms.round() as i64,
        speaker_id: segment.speaker_id,
        source: segment.source,
        translated: segment.translated?,
        timing: segment.timing,
        boundary: segment.boundary,
        revisable: segment.revisable,
        finalized: !segment.live,
    }))
}
