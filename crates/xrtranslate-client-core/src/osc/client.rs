//! Native client for the host's shared OSC receiver and paced writer.
use super::*;
use serde_json::json;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use xrtranslate_plugin::{Client, Request};

#[derive(Default)]
struct Shared {
    state: TransportState,
    events: VecDeque<TransportEvent>,
    requests: Vec<Request>,
}
#[derive(Clone)]
pub struct OscTransportHandle {
    client: Client,
    shared: Arc<Mutex<Shared>>,
}
impl OscTransportHandle {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            shared: Default::default(),
        }
    }
    pub fn observe(&self, value: serde_json::Value) -> Result<(), String> {
        let mut shared = self.shared.lock().unwrap();
        shared.state = serde_json::from_value(value["state"].clone()).map_err(|e| e.to_string())?;
        let events: Vec<TransportEvent> =
            serde_json::from_value(value["events"].clone()).map_err(|e| e.to_string())?;
        if shared.events.len() + events.len() > 128 {
            shared.events.clear();
            let epoch = shared.state.epoch;
            shared.events.push_back(TransportEvent::Invalidated {
                epoch,
                reason: "OSC consumer is too slow; reconnect required".into(),
            });
        } else {
            shared.events.extend(events);
        }
        Ok(())
    }
    fn request(&self, value: serde_json::Value) -> Result<(), String> {
        let mut shared = self.shared.lock().unwrap();
        if shared.requests.len() >= 64 {
            return Err("OSC command queue is full".into());
        }
        shared
            .requests
            .push(self.client.request("osc.control", value));
        Ok(())
    }
    pub fn set_control_profile(
        &self,
        profile: OscPeerProfile,
        paths: Option<Vec<String>>,
    ) -> Result<(), String> {
        self.request(json!({"action":"configure", "profile":profile, "paths":paths}))
    }
    pub fn events(&self) -> Vec<TransportEvent> {
        let mut shared = self.shared.lock().unwrap();
        let mut errors = Vec::new();
        shared.requests.retain(|request| match request.poll() {
            None => true,
            Some(Ok(_)) => false,
            Some(Err(error)) => {
                errors.push(error);
                false
            }
        });
        let epoch = shared.state.epoch;
        shared.events.extend(
            errors
                .into_iter()
                .map(|reason| TransportEvent::Invalidated { epoch, reason }),
        );
        shared.events.drain(..).collect()
    }
    pub fn state(&self) -> TransportState {
        self.shared.lock().unwrap().state.clone()
    }
    pub fn write_parameter(&self, epoch: u64, path: &str, value: OscValue) -> Result<(), String> {
        self.request(json!({"action":"write", "epoch":epoch,"path":path,"value":value}))
    }
    pub fn write_and_query(
        &self,
        epoch: u64,
        path: &str,
        value: OscValue,
        query_id: u64,
    ) -> Result<(), String> {
        self.request(
            json!({"action":"write","epoch":epoch,"path":path,"value":value,"query_id":query_id}),
        )
    }
    pub fn write_batch(
        &self,
        epoch: u64,
        values: Vec<(String, OscValue)>,
        lease: &WriteLease,
    ) -> Result<bool, String> {
        let mut state = lease.state.lock().unwrap();
        if state.revoked
            || state.pending.is_some()
            || state
                .sent
                .is_some_and(|sent| sent.elapsed() < lease.interval)
        {
            return Ok(false);
        }
        if let Some(error) = &state.error {
            return Err(error.clone());
        }
        state.pending = Some(self.client.request(
            "osc.control",
            json!({"action":"batch","epoch":epoch,"values":values}),
        ));
        Ok(true)
    }
}
#[derive(Default)]
pub struct WriteLease {
    interval: Duration,
    state: Mutex<Delivery>,
}
#[derive(Default)]
struct Delivery {
    revoked: bool,
    pending: Option<Request>,
    sent: Option<Instant>,
    error: Option<String>,
}
impl WriteLease {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            ..Self::default()
        }
    }
    pub fn revoke(&self) {
        let mut state = self.state.lock().unwrap();
        state.revoked = true;
        state.pending = None;
    }
    pub fn last_sent(&self) -> Option<Instant> {
        let mut state = self.state.lock().unwrap();
        if let Some(result) = state.pending.as_ref().and_then(Request::poll) {
            state.pending = None;
            match result {
                Ok(_) => state.sent = Some(Instant::now()),
                Err(error) => state.error = Some(error),
            }
        }
        (!state.revoked && state.pending.is_none() && state.error.is_none())
            .then_some(state.sent)
            .flatten()
    }
    pub fn error(&self) -> Option<String> {
        self.state.lock().unwrap().error.clone()
    }
}
