use super::super::osc::{
    OscPlugin,
    transport::{AvatarSnapshot, OscTransportHandle, OscValue, TransportEvent},
};
use std::time::Duration;
use xrt_avatar_control::{
    Controller, Effect, Generation, HostResult, OperationId, Snapshot, protocol,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HostAction {
    Start(OperationId),
    Cancel(OperationId),
}

struct Binding {
    epoch: u64,
    avatar_id: String,
    generation: Generation,
}

#[derive(Default)]
pub(crate) struct VrcAvatarPlugin {
    controller: Controller,
    enabled: bool,
    binding: Option<Binding>,
    latest_epoch: u64,
    effects: Vec<Effect>,
    host_actions: Vec<HostAction>,
    connection_diagnostic: Option<String>,
}

impl VrcAvatarPlugin {
    /// Runs on the host event loop. The handle only queues socket operations.
    pub(crate) fn poll(&mut self, enabled: bool, osc: &OscPlugin) {
        let transport = osc.transport();
        if self.enabled != enabled {
            self.enabled = enabled;
            self.invalidate("Avatar control is disabled".into());
            let paths = enabled.then(|| {
                vec![
                    protocol::ENABLED_ADDRESS.into(),
                    protocol::STATUS_ADDRESS.into(),
                    protocol::VERSION_ADDRESS.into(),
                ]
            });
            if let Err(error) = transport.set_control_paths(paths) {
                self.invalidate(error);
            }
        }

        for event in transport.events() {
            if !self.enabled {
                continue;
            }
            match event {
                TransportEvent::Snapshot(snapshot) => self.observe_snapshot(snapshot),
                TransportEvent::Invalidated { epoch, reason } if epoch >= self.latest_epoch => {
                    self.latest_epoch = epoch;
                    self.invalidate(reason);
                }
                TransportEvent::Invalidated { .. } => {}
            }
        }
        self.flush_effects(&transport);
        if self.enabled && self.binding.is_none() {
            if let Some(error) = transport.state().diagnostic {
                self.connection_diagnostic = Some(error);
            }
        }
    }

    pub(crate) fn take_host_actions(&mut self) -> Vec<HostAction> {
        let actions = std::mem::take(&mut self.host_actions);
        let superseded: Vec<_> = actions
            .iter()
            .filter_map(|action| match action {
                HostAction::Start(operation)
                    if actions.contains(&HostAction::Cancel(*operation)) =>
                {
                    Some(*operation)
                }
                _ => None,
            })
            .collect();
        // Input can turn off or disconnect in the same poll that requested
        // startup. No host resource was acquired, so do not briefly open audio.
        for operation in &superseded {
            self.report_host_result(*operation, HostResult::Stopped);
        }
        actions
            .into_iter()
            .filter(|action| {
                let operation = match action {
                    HostAction::Start(operation) | HostAction::Cancel(operation) => operation,
                };
                !superseded.contains(operation)
            })
            .collect()
    }

    /// Callbacks enqueue effects for the next poll; they never reenter the host.
    pub(crate) fn report_host_result(&mut self, operation: OperationId, result: HostResult) {
        self.effects
            .extend(self.controller.host_result(operation, result));
    }

    pub(crate) fn request_host_stop(&mut self, operation: OperationId) {
        let snapshot = self.controller.snapshot();
        if snapshot.operation == Some(operation) {
            self.effects
                .extend(self.controller.request_enabled(snapshot.generation, false));
        }
    }

    pub(crate) fn snapshot(&self) -> Snapshot<'_> {
        self.controller.snapshot()
    }

    pub(crate) fn diagnostic(&self) -> Option<&str> {
        self.controller
            .snapshot()
            .diagnostic
            .or(self.connection_diagnostic.as_deref())
    }

    fn invalidate(&mut self, diagnostic: String) {
        let generation = self.controller.snapshot().generation;
        self.effects
            .extend(self.controller.disconnect(generation, diagnostic.clone()));
        self.binding = None;
        self.connection_diagnostic = Some(diagnostic);
    }

    fn observe_snapshot(&mut self, snapshot: AvatarSnapshot) {
        if snapshot.epoch < self.latest_epoch {
            return;
        }
        self.latest_epoch = snapshot.epoch;
        if snapshot.queried_at.elapsed() > Duration::from_secs(5) {
            self.invalidate("Avatar state is stale; waiting for a fresh connection".into());
            return;
        }
        let enabled = match enabled_value(&snapshot) {
            Ok(enabled) => enabled,
            Err(error) => {
                self.invalidate(error);
                return;
            }
        };
        if self.binding.as_ref().is_none_or(|binding| {
            binding.epoch != snapshot.epoch || binding.avatar_id != snapshot.avatar_id
        }) {
            let update = self.controller.bind();
            self.binding = Some(Binding {
                epoch: snapshot.epoch,
                avatar_id: snapshot.avatar_id,
                generation: update.generation,
            });
            self.connection_diagnostic = None;
            self.effects.extend(update.effects);
            // The snapshot that establishes a binding is never a user request.
            return;
        }

        let generation = self.binding.as_ref().expect("validated binding").generation;
        if let Some(token) = self.controller.snapshot().pending_reset {
            if snapshot.query_id == Some(token.sequence()) && !enabled {
                self.effects.extend(self.controller.confirm_reset(token));
            } else if snapshot.query_id.is_none() {
                // Retry at the transport's normal reconciliation cadence, not
                // in an immediate write/query loop when a UDP write is lost.
                self.effects.push(Effect::ResetEnabled { token });
            }
        } else if snapshot.query_id.is_none() {
            self.effects
                .extend(self.controller.request_enabled(generation, enabled));
        }
        // Refresh actual status, including after a lost UDP feedback packet.
        self.effects.push(Effect::WriteStatus {
            generation,
            value: self.controller.snapshot().status,
        });
    }

    fn flush_effects(&mut self, transport: &OscTransportHandle) {
        for effect in std::mem::take(&mut self.effects) {
            let result = match effect {
                Effect::Start(operation) => {
                    self.host_actions.push(HostAction::Start(operation));
                    continue;
                }
                Effect::Cancel(operation) => {
                    self.host_actions.push(HostAction::Cancel(operation));
                    continue;
                }
                Effect::ResetEnabled { token } => match &self.binding {
                    Some(binding)
                        if binding.generation == token.generation()
                            && self.controller.snapshot().pending_reset == Some(token) =>
                    {
                        transport.write_and_query(
                            binding.epoch,
                            protocol::ENABLED_ADDRESS,
                            OscValue::Bool(false),
                            token.sequence(),
                        )
                    }
                    _ => continue,
                },
                // Accepting ON needs no acknowledgement write: a later user OFF
                // may already exist by the time this queued effect reaches VRChat.
                // Echoing the old true value would erase that newer request.
                Effect::WriteEnabled { value: true, .. } => continue,
                Effect::WriteEnabled {
                    generation,
                    value: false,
                } => match &self.binding {
                    Some(binding) if binding.generation == generation => transport.write_parameter(
                        binding.epoch,
                        protocol::ENABLED_ADDRESS,
                        OscValue::Bool(false),
                    ),
                    _ => continue,
                },
                Effect::WriteStatus { generation, value } => match &self.binding {
                    Some(binding) if binding.generation == generation => transport.write_parameter(
                        binding.epoch,
                        protocol::STATUS_ADDRESS,
                        OscValue::Int(value.wire_value()),
                    ),
                    _ => continue,
                },
            };
            if let Err(error) = result {
                self.invalidate(error);
            }
        }
        // A transport write failure must still cancel immediately. Its reset
        // feedback has no valid endpoint and is discarded with the binding.
        for effect in std::mem::take(&mut self.effects) {
            if let Effect::Cancel(operation) = effect {
                self.host_actions.push(HostAction::Cancel(operation));
            }
        }
    }
}

fn enabled_value(snapshot: &AvatarSnapshot) -> Result<bool, String> {
    if snapshot.avatar_id.is_empty() {
        return Err("VRChat has not provided a current Avatar identity".into());
    }
    let node = |path: &str| {
        snapshot
            .nodes
            .get(path)
            .ok_or_else(|| "This Avatar does not have XRT-Avatar-Plugin controls".to_owned())
    };
    let version = node(protocol::VERSION_ADDRESS)?;
    if version.osc_type != "i"
        || version.access & 1 == 0
        || version.value != Some(OscValue::Int(protocol::PROTOCOL_VERSION))
    {
        return Err("The Avatar control protocol version is unsupported".into());
    }
    let status = node(protocol::STATUS_ADDRESS)?;
    if status.osc_type != "i"
        || status.access & 3 != 3
        || !matches!(status.value, Some(OscValue::Int(_)))
    {
        return Err("The Avatar status parameter must be a readable and writable integer".into());
    }
    let enabled = node(protocol::ENABLED_ADDRESS)?;
    if !matches!(enabled.osc_type.as_str(), "T" | "F") || enabled.access & 3 != 3 {
        return Err("The Avatar enable parameter must be a readable and writable boolean".into());
    }
    match enabled.value {
        Some(OscValue::Bool(value)) => Ok(value),
        _ => Err("VRChat returned an invalid Avatar enable value".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::osc::transport::OscQueryNode;
    use std::{collections::BTreeMap, time::Instant};
    use xrt_avatar_control::Status;

    fn snapshot(epoch: u64, enabled: bool, query_id: Option<u64>) -> AvatarSnapshot {
        AvatarSnapshot {
            epoch,
            query_id,
            avatar_id: "avtr_control_fixture".into(),
            queried_at: Instant::now(),
            nodes: BTreeMap::from([
                (
                    protocol::ENABLED_ADDRESS.into(),
                    OscQueryNode {
                        osc_type: if enabled { "T" } else { "F" }.into(),
                        access: 3,
                        value: Some(OscValue::Bool(enabled)),
                    },
                ),
                (
                    protocol::STATUS_ADDRESS.into(),
                    OscQueryNode {
                        osc_type: "i".into(),
                        access: 3,
                        value: Some(OscValue::Int(0)),
                    },
                ),
                (
                    protocol::VERSION_ADDRESS.into(),
                    OscQueryNode {
                        osc_type: "i".into(),
                        access: 3,
                        value: Some(OscValue::Int(protocol::PROTOCOL_VERSION)),
                    },
                ),
            ]),
        }
    }

    #[test]
    fn reset_queries_and_epoch_validation_prevent_stale_startup() {
        let mut plugin = VrcAvatarPlugin::default();
        plugin.observe_snapshot(snapshot(1, true, None));
        assert!(
            !plugin
                .effects
                .iter()
                .any(|effect| matches!(effect, Effect::Start(_)))
        );
        let token = plugin.snapshot().pending_reset.unwrap();
        plugin.observe_snapshot(snapshot(1, false, Some(token.sequence() + 1)));
        assert!(!plugin.snapshot().reset_confirmed);
        plugin.observe_snapshot(snapshot(1, false, Some(token.sequence())));
        assert!(plugin.snapshot().reset_confirmed);
        plugin.observe_snapshot(snapshot(1, true, None));
        let operation = plugin.snapshot().operation.unwrap();
        plugin.report_host_result(operation, HostResult::Started);
        assert_eq!(plugin.snapshot().status, Status::Active);

        plugin.report_host_result(operation, HostResult::Failed("device lost".into()));
        plugin.effects.clear();
        plugin.observe_snapshot(snapshot(1, true, None));
        assert_eq!(plugin.snapshot().status, Status::Error);
        assert!(
            !plugin
                .effects
                .iter()
                .any(|effect| matches!(effect, Effect::Start(_)))
        );
        plugin.observe_snapshot(snapshot(2, true, None));
        let new_token = plugin.snapshot().pending_reset.unwrap();
        plugin.observe_snapshot(snapshot(1, false, Some(new_token.sequence())));
        assert_eq!(plugin.snapshot().pending_reset, Some(new_token));
    }

    #[test]
    fn incompatible_avatar_cancels_only_the_owned_operation() {
        let mut plugin = VrcAvatarPlugin::default();
        plugin.observe_snapshot(snapshot(1, false, None));
        let token = plugin.snapshot().pending_reset.unwrap();
        plugin.observe_snapshot(snapshot(1, false, Some(token.sequence())));
        plugin.observe_snapshot(snapshot(1, true, None));
        let operation = plugin.snapshot().operation.unwrap();
        plugin.effects.clear();
        let mut incompatible = snapshot(2, true, None);
        incompatible.nodes.remove(protocol::VERSION_ADDRESS);
        plugin.observe_snapshot(incompatible);
        assert!(plugin.effects.contains(&Effect::Cancel(operation)));
        assert!(plugin.binding.is_none());
        assert_eq!(plugin.snapshot().status, Status::Unavailable);
    }

    #[test]
    fn cancelled_start_in_same_poll_never_acquires_host_audio() {
        let mut plugin = VrcAvatarPlugin::default();
        plugin.observe_snapshot(snapshot(1, false, None));
        let token = plugin.snapshot().pending_reset.unwrap();
        plugin.observe_snapshot(snapshot(1, false, Some(token.sequence())));
        plugin.observe_snapshot(snapshot(1, true, None));
        let operation = plugin.snapshot().operation.unwrap();
        plugin.request_host_stop(operation);
        plugin.host_actions = vec![HostAction::Start(operation), HostAction::Cancel(operation)];
        assert!(plugin.take_host_actions().is_empty());
        assert!(plugin.snapshot().operation.is_none());
        assert!(!plugin.snapshot().reset_confirmed);
    }
}
