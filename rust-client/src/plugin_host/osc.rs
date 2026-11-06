//! One owner for shared OSCQuery control; dropped requests revoke queued writes.
use crate::osc_transport::{OscPeerProfile, OscTransportHandle, WriteLease};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct Control {
    owner: Option<String>,
    handle: Option<OscTransportHandle>,
    writes: Vec<(String, u64, WriteLease, Instant)>,
    last_state: Value,
}
impl Control {
    pub fn active(&self) -> bool {
        self.owner.is_some() || !self.writes.is_empty()
    }
    pub fn cancel(&mut self, plugin: &str, request: Option<u64>) {
        self.writes.retain(|(owner, id, lease, _)| {
            if owner == plugin && request.is_none_or(|request| request == *id) {
                lease.revoke();
                false
            } else {
                true
            }
        });
        if request.is_none() && self.owner.as_deref() == Some(plugin) {
            if let Some(handle) = &self.handle {
                let _ = handle.set_control_profile(OscPeerProfile::default(), None);
            }
            self.owner = None;
            self.last_state = Value::Null;
        }
    }
    pub fn call(
        &mut self,
        plugin: &str,
        id: u64,
        args: Value,
        handle: OscTransportHandle,
    ) -> Option<xrtranslate_plugin::Result> {
        let result = (|| {
            match args["action"].as_str() {
                Some("configure") => {
                    if self.owner.as_deref().is_some_and(|owner| owner != plugin) {
                        return Err("OSC control is owned by another plugin".into());
                    }
                    let profile = serde_json::from_value(args["profile"].clone())
                        .map_err(|e| e.to_string())?;
                    let paths: Option<Vec<String>> =
                        serde_json::from_value(args["paths"].clone()).map_err(|e| e.to_string())?;
                    self.cancel(plugin, None);
                    handle.set_control_profile(profile, paths.clone())?;
                    self.owner = paths.map(|_| plugin.to_owned());
                    self.handle = Some(handle);
                }
                Some(action) => {
                    if self.owner.as_deref() != Some(plugin) {
                        return Err("OSC control is not configured".into());
                    }
                    let epoch = args["epoch"].as_u64().ok_or("Missing OSC epoch")?;
                    match action {
                        "batch" => {
                            let values = serde_json::from_value(args["values"].clone())
                                .map_err(|e| e.to_string())?;
                            let lease = WriteLease::new(Duration::ZERO);
                            handle.write_batch(epoch, values, &lease)?;
                            self.writes.push((plugin.into(), id, lease, Instant::now()));
                            return Ok(None);
                        }
                        "write" => {
                            let path = args["path"].as_str().ok_or("Missing OSC path")?;
                            let value = serde_json::from_value(args["value"].clone())
                                .map_err(|e| e.to_string())?;
                            if let Some(query) = args["query_id"].as_u64() {
                                handle.write_and_query(epoch, path, value, query)?;
                            } else {
                                handle.write_parameter(epoch, path, value)?;
                            }
                        }
                        _ => return Err("Unknown OSC action".into()),
                    }
                }
                None => return Err("Missing OSC action".into()),
            }
            Ok(Some(Value::Null))
        })();
        match result {
            Ok(None) => None,
            Ok(Some(value)) => Some(Ok(value)),
            Err(error) => Some(Err(error)),
        }
    }
    pub fn poll(
        &mut self,
    ) -> (
        Option<(String, Value)>,
        Vec<(String, u64, xrtranslate_plugin::Result)>,
    ) {
        let mut replies = Vec::new();
        self.writes.retain(|(plugin, id, lease, start)| {
            let result = if lease.last_sent().is_some() {
                Some(Ok(Value::Null))
            } else if start.elapsed() >= Duration::from_secs(5) {
                lease.revoke();
                Some(Err("OSC write expired".into()))
            } else {
                None
            };
            if let Some(result) = result {
                replies.push((plugin.clone(), *id, result));
                false
            } else {
                true
            }
        });
        let update = self
            .owner
            .as_ref()
            .zip(self.handle.as_ref())
            .and_then(|(owner, handle)| {
                let state = json!(handle.state());
                let events = handle.events();
                if state == self.last_state && events.is_empty() {
                    return None;
                }
                self.last_state = state.clone();
                Some((owner.clone(), json!({"state":state,"events":events})))
            });
        (update, replies)
    }
}
impl Drop for Control {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.clone() {
            self.cancel(&owner, None);
        }
    }
}
