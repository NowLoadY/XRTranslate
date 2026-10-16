use super::{
    connectors::{Connector, OscContract, Settings},
    output::DisplayOutput,
    websocket,
};
use crate::plugins::osc::transport::{
    AvatarSnapshot, OscPeerProfile, OscTransportHandle, OscValue, TransportEvent,
};
use std::time::Duration;
use xrt_connector_core::{Controller, Effect, Generation, HostResult, OperationId, Snapshot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HostAction {
    Start(OperationId),
    Cancel(OperationId),
}

enum Route {
    Osc { epoch: u64, avatar_id: String },
    WebSocket { session: String },
}
struct Binding {
    route: Route,
    generation: Generation,
}

#[derive(Default)]
pub(crate) struct AvatarPlugin {
    controller: Controller,
    enabled: bool,
    pub settings: Settings,
    applied_connector: Connector,
    binding: Option<Binding>,
    latest_epoch: u64,
    effects: Vec<Effect>,
    host_actions: Vec<HostAction>,
    connection_diagnostic: Option<String>,
    pub(super) websocket: Option<websocket::Runtime>,
    pub output: DisplayOutput,
}
impl AvatarPlugin {
    pub fn new(settings: Settings, text: crate::plugins::text_output::TextOutputHandle) -> Self {
        Self {
            settings,
            output: DisplayOutput::new(text),
            ..Self::default()
        }
    }
    pub(crate) fn poll(&mut self, enabled: bool, transport: &OscTransportHandle) {
        self.output.configure(&self.settings, enabled);
        if self.enabled != enabled || self.applied_connector != self.settings.connector {
            self.invalidate("Avatar control is disconnected".into());
            self.enabled = enabled;
            self.applied_connector = self.settings.connector;
            self.websocket = None;
            let contract = self.settings.connector.osc();
            if let Err(error) = transport.set_control_profile(
                contract
                    .filter(|_| enabled)
                    .map_or_else(OscPeerProfile::default, |c| c.peer),
                contract.filter(|_| enabled).map(OscContract::paths),
            ) {
                self.invalidate(error);
            }
            if enabled && contract.is_none() {
                match websocket::Runtime::start(self.settings.connector) {
                    Ok(runtime) => {
                        self.websocket = Some(runtime);
                        self.connection_diagnostic = None;
                    }
                    Err(error) => self.invalidate(error),
                }
            }
        }
        for event in transport.events() {
            if !self.enabled || self.settings.connector.osc().is_none() {
                continue;
            }
            match event {
                TransportEvent::Snapshot(snapshot) => {
                    let epoch = snapshot.epoch;
                    let layout = self.settings.connector.osc().and_then(|contract| {
                        super::osc_caption::discover(&contract.caption, &snapshot)
                    });
                    self.observe_snapshot(snapshot);
                    if matches!(self.binding.as_ref().map(|b| &b.route), Some(Route::Osc { epoch: current, .. }) if *current == epoch)
                    {
                        self.output.bind_osc(transport.clone(), epoch, layout);
                    }
                }
                TransportEvent::Invalidated { epoch, reason } if epoch >= self.latest_epoch => {
                    self.latest_epoch = epoch;
                    self.invalidate(reason);
                }
                _ => {}
            }
        }
        let events: Vec<_> = self
            .websocket
            .as_ref()
            .map(|runtime| runtime.events().collect())
            .unwrap_or_default();
        for event in events {
            self.observe_websocket(event);
        }
        if let Some(error) = self.websocket.as_ref().and_then(|runtime| runtime.error()) {
            self.invalidate(error);
            self.websocket = None;
        }
        self.flush_effects(transport);
        if let Err(error) = self.output.poll() {
            self.invalidate(error);
        }
        if self.enabled && self.binding.is_none() && self.settings.connector.osc().is_some() {
            if let Some(error) = transport.state().diagnostic {
                self.connection_diagnostic = Some(error);
            }
        }
    }
    pub(crate) fn take_host_actions(&mut self) -> Vec<HostAction> {
        let actions = std::mem::take(&mut self.host_actions);
        let superseded: Vec<_> = actions
            .iter()
            .filter_map(|a| match a {
                HostAction::Start(op) if actions.contains(&HostAction::Cancel(*op)) => Some(*op),
                _ => None,
            })
            .collect();
        for op in &superseded {
            self.report_host_result(*op, HostResult::Stopped);
        }
        actions
            .into_iter()
            .filter(|a| {
                !superseded.contains(match a {
                    HostAction::Start(op) | HostAction::Cancel(op) => op,
                })
            })
            .collect()
    }
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
        self.output.disconnect();
        let generation = self.controller.snapshot().generation;
        self.effects
            .extend(self.controller.disconnect(generation, diagnostic.clone()));
        self.binding = None;
        self.connection_diagnostic = Some(diagnostic);
    }
    fn observe_websocket(&mut self, event: websocket::Event) {
        match event {
            websocket::Event::Connected {
                session,
                captions,
                structured,
            } => {
                let update = self.controller.bind();
                if let Some(runtime) = &self.websocket
                    && captions
                {
                    self.output
                        .bind(runtime.sender.clone(), session.clone(), structured);
                }
                self.binding = Some(Binding {
                    route: Route::WebSocket { session },
                    generation: update.generation,
                });
                self.connection_diagnostic = None;
                self.effects.extend(update.effects);
            }
            websocket::Event::Disconnected { session } if self.has_session(&session) => {
                self.invalidate("Avatar connector disconnected".into())
            }
            websocket::Event::ResetAck { session, reset } if self.has_session(&session) => {
                if let Some(token) = self
                    .controller
                    .snapshot()
                    .pending_reset
                    .filter(|t| t.sequence() == reset)
                {
                    self.effects.extend(self.controller.confirm_reset(token));
                }
            }
            websocket::Event::SetEnabled { session, enabled } if self.has_session(&session) => {
                self.effects.extend(
                    self.controller
                        .request_enabled(self.controller.snapshot().generation, enabled),
                );
            }
            _ => {}
        }
    }
    fn has_session(&self, session: &str) -> bool {
        matches!(self.binding.as_ref().map(|b| &b.route), Some(Route::WebSocket { session: current, .. }) if current == session)
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
        let Some(contract) = self.settings.connector.osc() else {
            return;
        };
        let enabled = match enabled_value(&snapshot, contract) {
            Ok(value) => value,
            Err(error) => {
                self.invalidate(error);
                return;
            }
        };
        let same = matches!(self.binding.as_ref().map(|b| &b.route), Some(Route::Osc { epoch, avatar_id }) if *epoch == snapshot.epoch && *avatar_id == snapshot.avatar_id);
        if !same {
            self.output.disconnect();
            let update = self.controller.bind();
            self.binding = Some(Binding {
                route: Route::Osc {
                    epoch: snapshot.epoch,
                    avatar_id: snapshot.avatar_id,
                },
                generation: update.generation,
            });
            self.connection_diagnostic = None;
            self.effects.extend(update.effects);
            return;
        }
        let generation = self.binding.as_ref().unwrap().generation;
        if let Some(token) = self.controller.snapshot().pending_reset {
            if snapshot.query_id == Some(token.sequence()) && !enabled {
                self.effects.extend(self.controller.confirm_reset(token));
            } else if snapshot.query_id.is_none() {
                self.effects.push(Effect::ResetEnabled { token });
            }
        } else if snapshot.query_id.is_none() {
            self.effects
                .extend(self.controller.request_enabled(generation, enabled));
        }
        self.effects.push(Effect::WriteStatus {
            generation,
            value: self.controller.snapshot().status,
        });
    }
    fn flush_effects(&mut self, transport: &OscTransportHandle) {
        for effect in std::mem::take(&mut self.effects) {
            match effect {
                Effect::Start(op) => {
                    self.host_actions.push(HostAction::Start(op));
                    continue;
                }
                Effect::Cancel(op) => {
                    self.host_actions.push(HostAction::Cancel(op));
                    continue;
                }
                _ => {}
            }
            let Some(binding) = &self.binding else {
                continue;
            };
            let generation = match effect {
                Effect::ResetEnabled { token } => {
                    if self.controller.snapshot().pending_reset != Some(token) {
                        continue;
                    }
                    token.generation()
                }
                Effect::WriteStatus { generation, .. } => generation,
                _ => unreachable!(),
            };
            if binding.generation != generation {
                continue;
            }
            let result = match &binding.route {
                Route::Osc { epoch, .. } => {
                    let contract = self.settings.connector.osc().unwrap();
                    match effect {
                        Effect::ResetEnabled { token } => transport.write_and_query(
                            *epoch,
                            contract.enabled,
                            OscValue::Bool(false),
                            token.sequence(),
                        ),
                        Effect::WriteStatus { value, .. } => transport.write_parameter(
                            *epoch,
                            contract.status,
                            OscValue::Int(value.wire_value()),
                        ),
                        _ => unreachable!(),
                    }
                }
                Route::WebSocket { session, .. } => {
                    let Some(runtime) = &self.websocket else {
                        continue;
                    };
                    let snapshot = self.controller.snapshot();
                    let frame = match effect {
                        Effect::ResetEnabled { token } => {
                            serde_json::json!({"type":"reset", "reset":token.sequence()})
                        }
                        _ => {
                            serde_json::json!({"type":"state", "status":snapshot.status.name(), "enabled":snapshot.requested_enabled})
                        }
                    };
                    runtime.sender.send(session, frame)
                }
            };
            if let Err(error) = result {
                self.invalidate(error);
            }
        }
        for effect in std::mem::take(&mut self.effects) {
            if let Effect::Cancel(op) = effect {
                self.host_actions.push(HostAction::Cancel(op));
            }
        }
    }
}
fn enabled_value(snapshot: &AvatarSnapshot, contract: &OscContract) -> Result<bool, String> {
    if snapshot.avatar_id.is_empty() {
        return Err("The game has not provided a current Avatar identity".into());
    }
    let node = |path: &str| {
        snapshot
            .nodes
            .get(path)
            .ok_or_else(|| "This Avatar does not have XRT-Avatar-Plugin controls".to_owned())
    };
    let version = node(contract.version)?;
    if version.osc_type != "i"
        || version.access & 1 == 0
        || version.value != Some(OscValue::Int(contract.protocol_version))
    {
        return Err("The Avatar control protocol version is unsupported".into());
    }
    let status = node(contract.status)?;
    if status.osc_type != "i"
        || status.access & 3 != 3
        || !matches!(status.value, Some(OscValue::Int(_)))
    {
        return Err("The Avatar status parameter must be a readable and writable integer".into());
    }
    let enabled = node(contract.enabled)?;
    if !matches!(enabled.osc_type.as_str(), "T" | "F") || enabled.access & 3 != 3 {
        return Err("The Avatar enable parameter must be a readable and writable boolean".into());
    }
    match enabled.value {
        Some(OscValue::Bool(value)) => Ok(value),
        _ => Err("The game returned an invalid Avatar enable value".into()),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::osc::transport::OscQueryNode;
    use std::{collections::BTreeMap, time::Instant};
    use xrt_connector_core::Status;

    fn snapshot(epoch: u64, enabled: bool, query_id: Option<u64>) -> AvatarSnapshot {
        let contract = Connector::Vrchat.osc().unwrap();
        AvatarSnapshot {
            epoch,
            query_id,
            avatar_id: "avtr_control_fixture".into(),
            queried_at: Instant::now(),
            nodes: BTreeMap::from([
                (
                    contract.enabled.into(),
                    OscQueryNode {
                        osc_type: if enabled { "T" } else { "F" }.into(),
                        access: 3,
                        value: Some(OscValue::Bool(enabled)),
                    },
                ),
                (
                    contract.status.into(),
                    OscQueryNode {
                        osc_type: "i".into(),
                        access: 3,
                        value: Some(OscValue::Int(0)),
                    },
                ),
                (
                    contract.version.into(),
                    OscQueryNode {
                        osc_type: "i".into(),
                        access: 3,
                        value: Some(OscValue::Int(contract.protocol_version)),
                    },
                ),
            ]),
        }
    }

    #[test]
    fn reset_queries_and_epoch_validation_prevent_stale_startup() {
        let mut plugin = AvatarPlugin::default();
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
        let mut plugin = AvatarPlugin::default();
        plugin.observe_snapshot(snapshot(1, false, None));
        let token = plugin.snapshot().pending_reset.unwrap();
        plugin.observe_snapshot(snapshot(1, false, Some(token.sequence())));
        plugin.observe_snapshot(snapshot(1, true, None));
        let operation = plugin.snapshot().operation.unwrap();
        plugin.effects.clear();
        let mut incompatible = snapshot(2, true, None);
        incompatible
            .nodes
            .remove(Connector::Vrchat.osc().unwrap().version);
        plugin.observe_snapshot(incompatible);
        assert!(plugin.effects.contains(&Effect::Cancel(operation)));
        assert!(plugin.binding.is_none());
        assert_eq!(plugin.snapshot().status, Status::Unavailable);
    }

    #[test]
    fn cancelled_start_in_same_poll_never_acquires_host_audio() {
        let mut plugin = AvatarPlugin::default();
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
