//! Read-only, owner-scoped caption consumer for paired game components.
use super::websocket::SenderHandle;
use crate::session_coordinator::{
    SessionEventSubscriber, TranslationEvent, TranslationOutcome, TranslationSessionOwner,
};
use parking_lot::Mutex;
use serde_json::json;
use std::sync::Arc;

#[derive(Default)]
struct State {
    connection: Option<(SenderHandle, String)>,
    owner: Option<TranslationSessionOwner>,
    enabled: bool,
    revision: u64,
}
#[derive(Clone, Default)]
pub(crate) struct CaptionSink(Arc<Mutex<State>>);
impl CaptionSink {
    pub(super) fn bind(&self, sender: SenderHandle, session: String) {
        let mut state = self.0.lock();
        state.connection = Some((sender, session));
        state.revision = 0;
    }
    pub(super) fn disconnect(&self) {
        self.clear_owner();
        self.0.lock().connection = None;
    }
    pub(crate) fn set_owner(&self, owner: TranslationSessionOwner) {
        self.0.lock().owner = Some(owner);
    }
    pub(super) fn set_enabled(&self, enabled: bool) {
        let mut state = self.0.lock();
        if state.enabled && !enabled {
            clear(&state);
        }
        state.enabled = enabled;
    }
    pub(super) fn clear_owner(&self) {
        let mut state = self.0.lock();
        clear(&state);
        state.owner = None;
    }
}
fn clear(state: &State) {
    if let Some((sender, session)) = &state.connection {
        let _ = sender.send(session, json!({"type":"clear"}));
    }
}
impl SessionEventSubscriber for CaptionSink {
    fn accepts_owner(&self, owner: &TranslationSessionOwner) -> bool {
        self.0.lock().owner.as_ref() == Some(owner)
    }
    fn on_translation_event(&self, owner: &TranslationSessionOwner, event: &TranslationEvent) {
        let mut state = self.0.lock();
        if !state.enabled || state.owner.as_ref() != Some(owner) {
            return;
        }
        let (stream, id, text) = match event {
            TranslationEvent::Segment(segment) => {
                let Some(text) = segment
                    .translated
                    .as_deref()
                    .or_else(|| segment.asr_only.then_some(segment.source.as_str()))
                else {
                    return;
                };
                (segment.stream_id, segment.turn_id.clone(), text.to_owned())
            }
            TranslationEvent::ReplaceSegments(segments) => {
                let Some(last) = segments.last() else {
                    return;
                };
                let text = segments
                    .iter()
                    .filter_map(|s| {
                        s.translated
                            .as_deref()
                            .or_else(|| s.asr_only.then_some(s.source.as_str()))
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                if text.is_empty() {
                    return;
                }
                (last.stream_id, last.turn_id.clone(), text)
            }
            TranslationEvent::Finished {
                outcome: TranslationOutcome::Cancelled | TranslationOutcome::Failed(_),
                ..
            } => {
                clear(&state);
                return;
            }
            _ => return,
        };
        // One replaceable caption per stream. Platforms own layout and visibility.
        let text: String = text
            .chars()
            .filter(|c| !c.is_control() || *c == '\n')
            .take(256)
            .collect();
        if text.is_empty() {
            return;
        }
        state.revision += 1;
        if let Some((sender, session)) = &state.connection {
            let _ = sender.send(session, json!({
                "type":"caption", "stream":stream.to_string(), "id":id.chars().take(64).collect::<String>(),
                "revision":state.revision, "text":text, "expires_ms":15000
            }));
        }
    }
}
