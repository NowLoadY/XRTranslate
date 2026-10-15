//! Owner-scoped captions, delivered through the selected game connector.
use super::{
    vrchat_caption::{Board, Layout},
    websocket::SenderHandle,
};
use crate::{
    plugins::osc::transport::OscTransportHandle,
    session_coordinator::{
        SessionEventSubscriber, TranslationEvent, TranslationOutcome, TranslationSessionOwner,
    },
};
use parking_lot::Mutex;
use serde_json::json;
use std::sync::Arc;

enum Connection {
    WebSocket(SenderHandle, String),
    Vrchat(Board),
}

#[derive(Default)]
struct State {
    connection: Option<Connection>,
    owner: Option<TranslationSessionOwner>,
    enabled: bool,
    revision: u64,
}
#[derive(Clone, Default)]
pub(crate) struct CaptionSink(Arc<Mutex<State>>);
impl CaptionSink {
    pub(super) fn bind(&self, sender: SenderHandle, session: String) {
        let mut state = self.0.lock();
        clear(&mut state);
        state.connection = Some(Connection::WebSocket(sender, session));
        state.revision = 0;
    }
    pub(super) fn bind_osc(
        &self,
        transport: OscTransportHandle,
        epoch: u64,
        layout: Option<Layout>,
    ) {
        let mut state = self.0.lock();
        if matches!(&state.connection, Some(Connection::Vrchat(board)) if board.epoch == epoch && Some(board.layout) == layout)
        {
            return;
        }
        clear(&mut state);
        state.connection =
            layout.map(|layout| Connection::Vrchat(Board::new(transport, epoch, layout)));
        state.revision = 0;
    }
    pub(super) fn available(&self) -> bool {
        self.0.lock().connection.is_some()
    }
    pub(super) fn poll(&self) -> Result<(), String> {
        if let Some(Connection::Vrchat(board)) = &mut self.0.lock().connection {
            board.poll()?;
        }
        Ok(())
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
            clear(&mut state);
        }
        state.enabled = enabled;
    }
    pub(super) fn clear_owner(&self) {
        let mut state = self.0.lock();
        clear(&mut state);
        state.owner = None;
    }
}
fn clear(state: &mut State) {
    match &mut state.connection {
        Some(Connection::WebSocket(sender, session)) => {
            let _ = sender.send(session, json!({"type":"clear"}));
        }
        Some(Connection::Vrchat(board)) => board.clear(),
        None => {}
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
        let vrchat = matches!(state.connection, Some(Connection::Vrchat(_)));
        let (stream, id, text) = match event {
            TranslationEvent::Segment(segment)
                if !vrchat || (!segment.revisable && !segment.live) =>
            {
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
                    if vrchat {
                        clear(&mut state);
                    }
                    return;
                };
                // The bus displays the latest completed sentence, not an older
                // prefix of a turn's growing authoritative replacement batch.
                let mut texts = segments
                    .iter()
                    .filter(|s| !vrchat || (!s.revisable && !s.live))
                    .filter_map(|s| {
                        s.translated
                            .as_deref()
                            .or_else(|| s.asr_only.then_some(s.source.as_str()))
                    });
                let text = if vrchat {
                    texts.next_back().unwrap_or_default().to_owned()
                } else {
                    texts.collect::<Vec<_>>().join(" ")
                };
                if text.is_empty() {
                    return;
                }
                (last.stream_id, last.turn_id.clone(), text)
            }
            TranslationEvent::Finished {
                outcome: TranslationOutcome::Cancelled | TranslationOutcome::Failed(_),
                ..
            } => {
                clear(&mut state);
                return;
            }
            _ => return,
        };
        let text = if vrchat {
            // The glyph formatter normalizes before applying its slot bound.
            text
        } else {
            text.chars()
                .filter(|c| !c.is_control() || *c == '\n')
                .take(256)
                .collect()
        };
        if text.is_empty() {
            return;
        }
        state.revision += 1;
        let revision = state.revision;
        match &mut state.connection {
            Some(Connection::WebSocket(sender, session)) => {
                let _ = sender.send(session, json!({
                    "type":"caption", "stream":stream.to_string(), "id":id.chars().take(64).collect::<String>(),
                    "revision":revision, "text":text, "expires_ms":15000
                }));
            }
            Some(Connection::Vrchat(board)) => board.replace(&text),
            None => {}
        }
    }
}
