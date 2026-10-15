//! Translation producer for the shared text display, scoped to its session owner.
use super::{
    display::{TEXT_LIFETIME, TextDisplay, TextUpdate},
    osc_caption::Layout,
    websocket::SenderHandle,
};
use crate::{
    plugins::osc::transport::OscTransportHandle,
    session_coordinator::{
        SessionEventSubscriber, TranslationEvent, TranslationOutcome, TranslationSessionOwner,
    },
};
use parking_lot::Mutex;
use std::sync::Arc;

#[derive(Default)]
struct State {
    display: TextDisplay,
    owner: Option<TranslationSessionOwner>,
    enabled: bool,
}
#[derive(Clone, Default)]
pub(crate) struct CaptionSink(Arc<Mutex<State>>);
impl CaptionSink {
    pub(super) fn bind(&self, sender: SenderHandle, session: String) {
        self.0.lock().display.bind(sender, session);
    }
    pub(super) fn bind_osc(
        &self,
        transport: OscTransportHandle,
        epoch: u64,
        layout: Option<Layout>,
    ) {
        self.0.lock().display.bind_osc(transport, epoch, layout);
    }
    pub(super) fn available(&self) -> bool {
        self.0.lock().display.available()
    }
    pub(super) fn poll(&self) -> Result<(), String> {
        self.0.lock().display.poll()
    }
    pub(super) fn disconnect(&self) {
        let mut state = self.0.lock();
        state.owner = None;
        state.display.disconnect();
    }
    pub(crate) fn set_owner(&self, owner: TranslationSessionOwner) {
        let mut state = self.0.lock();
        if state.owner.as_ref() != Some(&owner) {
            state.display.clear();
        }
        state.owner = Some(owner);
    }
    pub(super) fn set_enabled(&self, enabled: bool) {
        let mut state = self.0.lock();
        if state.enabled && !enabled {
            state.display.clear();
        }
        state.enabled = enabled;
    }
    pub(super) fn clear_owner(&self) {
        let mut state = self.0.lock();
        state.display.clear();
        state.owner = None;
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
        let completed_only = state.display.prefers_completed_text();
        let (stream, id, text) = match event {
            TranslationEvent::Segment(segment)
                if !completed_only || (!segment.revisable && !segment.live) =>
            {
                let Some(text) = segment
                    .translated
                    .as_deref()
                    .or_else(|| segment.asr_only.then_some(segment.source.as_str()))
                else {
                    return;
                };
                (segment.stream_id, segment.turn_id.as_str(), text.to_owned())
            }
            TranslationEvent::ReplaceSegments(segments) => {
                let Some(last) = segments.last() else {
                    state.display.clear();
                    return;
                };
                let mut candidates = segments
                    .iter()
                    .filter(|s| !completed_only || (!s.revisable && !s.live))
                    .filter_map(|s| {
                        s.translated
                            .as_deref()
                            .or_else(|| s.asr_only.then_some(s.source.as_str()))
                            .map(|text| (s, text))
                    });
                if completed_only {
                    let Some((segment, text)) = candidates.next_back() else {
                        return;
                    };
                    (segment.stream_id, segment.turn_id.as_str(), text.to_owned())
                } else {
                    let texts = candidates.map(|(_, text)| text).collect::<Vec<_>>();
                    if texts.is_empty() {
                        return;
                    }
                    (last.stream_id, last.turn_id.as_str(), texts.join(" "))
                }
            }
            TranslationEvent::Finished {
                outcome: TranslationOutcome::Cancelled | TranslationOutcome::Failed(_),
                ..
            } => {
                state.display.clear();
                return;
            }
            _ => return,
        };
        state.display.replace(TextUpdate {
            stream: &stream.to_string(),
            id,
            text: &text,
            lifetime: TEXT_LIFETIME,
        });
    }
}
