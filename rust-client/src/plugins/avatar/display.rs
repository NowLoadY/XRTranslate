//! Source-neutral text display over the selected connector.
use super::{
    osc_caption::{Board, Layout},
    websocket::SenderHandle,
};
use crate::plugins::osc::transport::OscTransportHandle;
use serde_json::json;
use std::time::Duration;

pub(crate) const TEXT_LIFETIME: Duration = Duration::from_secs(15);

/// Opaque source/result identity, plain text and a renewable display lifetime.
/// Content selection belongs to the host; connectors do not interpret the source.
pub(crate) struct TextUpdate<'a> {
    pub stream: &'a str,
    pub id: &'a str,
    pub text: &'a str,
    pub lifetime: Duration,
}

enum Connection {
    WebSocket(SenderHandle, String),
    Osc(Board),
}

#[derive(Default)]
pub(crate) struct TextDisplay {
    connection: Option<Connection>,
    revision: u64,
}

impl TextDisplay {
    pub(super) fn bind(&mut self, sender: SenderHandle, session: String) {
        self.clear();
        self.connection = Some(Connection::WebSocket(sender, session));
        self.revision = 0;
    }
    pub(super) fn bind_osc(
        &mut self,
        transport: OscTransportHandle,
        epoch: u64,
        layout: Option<Layout>,
    ) {
        if matches!(&self.connection, Some(Connection::Osc(board)) if board.epoch == epoch && Some(board.layout) == layout)
        {
            return;
        }
        self.clear();
        self.connection =
            layout.map(|layout| Connection::Osc(Board::new(transport, epoch, layout)));
        self.revision = 0;
    }
    pub(crate) fn available(&self) -> bool {
        self.connection.is_some()
    }
    pub(crate) fn prefers_completed_text(&self) -> bool {
        matches!(self.connection, Some(Connection::Osc(_)))
    }
    pub(super) fn poll(&mut self) -> Result<(), String> {
        if let Some(Connection::Osc(board)) = &mut self.connection {
            board.poll()?;
        }
        Ok(())
    }
    pub(super) fn disconnect(&mut self) {
        self.clear();
        self.connection = None;
    }
    pub(crate) fn clear(&mut self) {
        match &mut self.connection {
            Some(Connection::WebSocket(sender, session)) => {
                let _ = sender.send(session, json!({"type":"clear"}));
            }
            Some(Connection::Osc(board)) => board.clear(),
            None => {}
        }
    }
    pub(crate) fn replace(&mut self, update: TextUpdate<'_>) {
        if update.text.trim().is_empty() || update.lifetime.is_zero() {
            self.clear();
            return;
        }
        self.revision += 1;
        let lifetime = update.lifetime.min(TEXT_LIFETIME);
        match &mut self.connection {
            Some(Connection::WebSocket(sender, session)) => {
                let text: String = update
                    .text
                    .chars()
                    .filter(|c| !c.is_control() || *c == '\n')
                    .take(256)
                    .collect();
                if text.is_empty() {
                    let _ = sender.send(session, json!({"type":"clear"}));
                } else {
                    let _ = sender.send(session, json!({
                        "type":"caption", "stream":update.stream.chars().take(64).collect::<String>(),
                        "id":update.id.chars().take(64).collect::<String>(), "revision":self.revision,
                        "text":text, "expires_ms":lifetime.as_millis() as u64
                    }));
                }
            }
            Some(Connection::Osc(board)) => board.replace(update.text, lifetime),
            None => {}
        }
    }
}
