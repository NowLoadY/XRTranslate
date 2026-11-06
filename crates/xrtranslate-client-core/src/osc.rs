//! Shared OSC receiver configuration; chatbox composition belongs to its consumer.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReceiveOptions {
    pub enabled: bool,
    pub ip: String,
    pub listen_port: u16,
    pub send_port: u16,
}
impl Default for ReceiveOptions {
    fn default() -> Self {
        Self {
            enabled: false,
            ip: "127.0.0.1".into(),
            listen_port: 9001,
            send_port: 9000,
        }
    }
}

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct ReceiverStatus {
    pub listener: String,
    pub microphone: Option<String>,
}
