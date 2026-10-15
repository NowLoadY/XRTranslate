//! Deliver one structured display through the selected connector.
use super::{
    osc_caption::{Board, Layout},
    websocket::SenderHandle,
};
use crate::plugins::osc::transport::OscTransportHandle;
use serde_json::json;
use xrt_connector_core::DisplaySnapshot;

enum Connection {
    WebSocket {
        sender: SenderHandle,
        session: String,
        structured: bool,
    },
    Osc(Board),
}
#[derive(Default)]
pub(crate) struct TextDisplay {
    connection: Option<Connection>,
    revision: u64,
}
impl TextDisplay {
    pub(super) fn bind(&mut self, sender: SenderHandle, session: String, structured: bool) {
        self.clear();
        self.connection = Some(Connection::WebSocket {
            sender,
            session,
            structured,
        });
        self.revision = 0;
    }
    pub(super) fn bind_osc(
        &mut self,
        transport: OscTransportHandle,
        epoch: u64,
        layout: Option<Layout>,
    ) -> bool {
        if matches!(&self.connection, Some(Connection::Osc(board)) if board.epoch == epoch && Some(board.layout) == layout)
        {
            return false;
        }
        self.clear();
        self.connection =
            layout.map(|layout| Connection::Osc(Board::new(transport, epoch, layout)));
        self.revision = 0;
        true
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
            Some(Connection::WebSocket {
                sender, session, ..
            }) => {
                let _ = sender.send(session, json!({"type":"clear"}));
            }
            Some(Connection::Osc(board)) => board.clear(),
            None => {}
        }
    }
    pub(crate) fn replace(&mut self, snapshot: &DisplaySnapshot) {
        self.revision += 1;
        match &mut self.connection {
            Some(Connection::WebSocket {
                sender,
                session,
                structured,
            }) => {
                let value = if *structured {
                    json!({"type":"display", "revision":self.revision,
                        "header":snapshot.header, "messages":snapshot.messages, "footer":snapshot.footer})
                } else {
                    // Legacy projection is maintained with the connector contract.
                    let text = snapshot.legacy_text();
                    if text.is_empty() {
                        json!({"type":"clear"})
                    } else {
                        json!({"type":"caption", "stream":"display", "id":"current", "revision":self.revision,
                            "text":text, "expires_ms":snapshot.messages.iter().map(|message| message.ttl_ms).min().unwrap_or(8000)})
                    }
                };
                let _ = sender.send(session, value);
            }
            Some(Connection::Osc(board)) => board.replace(snapshot),
            None => {}
        }
    }
}
