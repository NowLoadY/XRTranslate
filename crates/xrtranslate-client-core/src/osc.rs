//! Shared OSC receiver configuration; chatbox composition belongs to its consumer.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OscPeerProfile {
    pub service_name: String,
    pub identity_path: String,
    pub invalidate_paths: Vec<String>,
    pub mute_path: Option<String>,
}
impl Default for OscPeerProfile {
    fn default() -> Self {
        Self {
            service_name: "VRChat".into(),
            identity_path: "/avatar/change".into(),
            invalidate_paths: vec!["/avatar/change".into()],
            mute_path: Some("/avatar/parameters/MuteSelf".into()),
        }
    }
}
impl OscPeerProfile {
    pub fn accepts_name(&self, name: &str) -> bool {
        name.eq_ignore_ascii_case(&self.service_name)
            || (name
                .get(..self.service_name.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(&self.service_name))
                && name.as_bytes().get(self.service_name.len()) == Some(&b'-'))
    }
    pub fn invalidates_on(&self, path: &str) -> bool {
        self.invalidate_paths.iter().any(|p| p == path)
    }
}

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

use std::{collections::BTreeMap, time::SystemTime};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum OscValue {
    Bool(bool),
    Int(i32),
    Float(f32),
    String(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OscQueryNode {
    pub osc_type: String,
    pub access: u8,
    pub value: Option<OscValue>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AvatarSnapshot {
    pub epoch: u64,
    /// A caller-provided identity, present only for a query begun after its write.
    pub query_id: Option<u64>,
    pub avatar_id: String,
    pub nodes: BTreeMap<String, OscQueryNode>,
    pub queried_at: SystemTime,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TransportEvent {
    Snapshot(AvatarSnapshot),
    Invalidated { epoch: u64, reason: String },
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TransportState {
    pub epoch: u64,
    pub control_enabled: bool,
    pub listener: String,
    pub diagnostic: Option<String>,
}

mod client;
pub use client::{OscTransportHandle, WriteLease};
