//! Shared receive capability and local OSCQuery transport.
//!
//! OSCQuery discovery and all socket I/O live on this owner's worker. Consumers
//! provide paths, inspect typed snapshots and carry epochs through their writes;
//! this module has no knowledge of a particular Avatar feature's parameters.
use super::{
    microphone::{Edge, MicrophoneSync},
    runtime::{OscSettings, parse_mute_value},
};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use parking_lot::Mutex;
use rosc::{OscBundle, OscMessage, OscPacket, OscType, decoder, encoder};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
pub use xrt_connector_core::osc::Peer as OscPeerProfile;

const QUERY_SERVICE: &str = "_oscjson._tcp.local.";
const OSC_SERVICE: &str = "_osc._udp.local.";
const QUERY_INTERVAL: Duration = Duration::from_secs(1);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(5);
const HTTP_TIMEOUT: Duration = Duration::from_millis(1000);
const MAX_BODY: usize = 4 * 1024 * 1024;
const MAX_EVENTS: usize = 128;
const MAX_PATHS: usize = 32;

#[derive(Clone, Debug, PartialEq)]
pub enum OscValue {
    Bool(bool),
    Int(i32),
    Float(f32),
    String(String),
}

impl OscValue {
    fn osc(&self) -> OscType {
        match self {
            Self::Bool(v) => OscType::Bool(*v),
            Self::Int(v) => OscType::Int(*v),
            Self::Float(v) => OscType::Float(*v),
            Self::String(v) => OscType::String(v.clone()),
        }
    }

    fn from_osc(value: &OscType) -> Option<Self> {
        match value {
            OscType::Bool(v) => Some(Self::Bool(*v)),
            OscType::Int(v) => Some(Self::Int(*v)),
            OscType::Float(v) if v.is_finite() => Some(Self::Float(*v)),
            OscType::String(v) if v.len() <= 1024 => Some(Self::String(v.clone())),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct OscQueryNode {
    pub osc_type: String,
    pub access: u8,
    pub value: Option<OscValue>,
}

#[derive(Clone, Debug)]
pub struct AvatarSnapshot {
    pub epoch: u64,
    /// A caller-provided identity, present only for a query begun after its write.
    pub query_id: Option<u64>,
    pub avatar_id: String,
    pub nodes: BTreeMap<String, OscQueryNode>,
    pub queried_at: Instant,
}

#[derive(Clone, Debug)]
pub enum TransportEvent {
    Snapshot(AvatarSnapshot),
    Invalidated { epoch: u64, reason: String },
}

#[derive(Clone, Debug, Default)]
pub struct TransportState {
    pub epoch: u64,
    pub control_enabled: bool,
    pub listener: String,
    pub diagnostic: Option<String>,
}

#[derive(Default)]
struct Shared {
    state: TransportState,
    events: VecDeque<TransportEvent>,
    microphone: MicrophoneSync,
    last_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
struct Config {
    manual_enabled: bool,
    manual_ip: String,
    manual_port: u16,
    send_port: u16,
    microphone_sync: bool,
    control_paths: Option<BTreeSet<String>>,
    peer_profile: OscPeerProfile,
}

impl Config {
    fn new(settings: &OscSettings) -> Self {
        Self {
            manual_enabled: settings.enabled,
            manual_ip: settings.ip.clone(),
            manual_port: settings.listen_port,
            send_port: settings.send_port,
            microphone_sync: false,
            control_paths: None,
            peer_profile: OscPeerProfile::default(),
        }
    }

    fn query_enabled(&self) -> bool {
        self.control_paths.is_some()
            || (self.microphone_sync && self.peer_profile == OscPeerProfile::default())
    }
}

enum Command {
    Write {
        epoch: u64,
        path: String,
        value: OscValue,
        query_id: Option<u64>,
    },
    Batch {
        epoch: u64,
        values: Vec<(String, OscValue)>,
        lease: WriteLease,
    },
}

/// Revokes queued writes when a consumer replaces or cancels pending output.
#[derive(Clone, Default)]
pub(crate) struct WriteLease(Arc<WriteLeaseState>);

#[derive(Default)]
struct WriteLeaseState {
    revoked: AtomicBool,
    pending: AtomicBool,
    sent: Mutex<Option<Instant>>,
    interval: Duration,
}

impl WriteLease {
    pub(crate) fn new(interval: Duration) -> Self {
        Self(Arc::new(WriteLeaseState {
            interval,
            ..WriteLeaseState::default()
        }))
    }

    pub(crate) fn revoke(&self) {
        self.0.revoked.store(true, Ordering::Release);
    }

    pub(crate) fn last_sent(&self) -> Option<Instant> {
        if self.0.pending.load(Ordering::Acquire) || self.0.revoked.load(Ordering::Acquire) {
            None
        } else {
            *self.0.sent.lock()
        }
    }

    fn claim(&self) -> bool {
        !self.0.revoked.load(Ordering::Acquire)
            && !self
                .0
                .sent
                .lock()
                .is_some_and(|sent| sent.elapsed() < self.0.interval)
            && self
                .0
                .pending
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
    }

    fn completed(&self) {
        *self.0.sent.lock() = Some(Instant::now());
        self.0.pending.store(false, Ordering::Release);
    }
}

#[derive(Clone)]
pub struct OscTransportHandle {
    muted: Arc<AtomicBool>,
    config: Arc<Mutex<Config>>,
    shared: Arc<Mutex<Shared>>,
    tx: mpsc::Sender<Command>,
    stop: Arc<AtomicBool>,
    overflow: Arc<AtomicBool>,
}

impl OscTransportHandle {
    pub(crate) fn update_settings(&self, settings: &OscSettings) {
        let mut config = self.config.lock();
        config.manual_enabled = settings.enabled;
        config.manual_ip.clone_from(&settings.ip);
        config.manual_port = settings.listen_port;
        config.send_port = settings.send_port;
    }

    pub fn set_microphone_sync(&self, enabled: bool) {
        let mut config = self.config.lock();
        if config.microphone_sync != enabled {
            config.microphone_sync = enabled;
            self.shared.lock().microphone.set_enabled(enabled);
        }
    }

    /// An explicit host gesture, not a continuously maintained desired state.
    pub fn request_microphone(&self, enabled: bool) {
        self.shared.lock().microphone.request(enabled);
    }

    pub fn take_microphone_mute(&self) -> bool {
        self.shared.lock().microphone.take_external_mute()
    }

    pub fn microphone_sync_status(&self) -> Option<&'static str> {
        let config = self.config.lock();
        if config.microphone_sync && config.peer_profile != OscPeerProfile::default() {
            return Some("Microphone sync requires VRChat with Toggle Voice enabled");
        }
        self.shared.lock().microphone.status()
    }

    /// This receiver demand is independent of Chatbox output and its plugin tab.
    pub fn set_control_profile(
        &self,
        profile: OscPeerProfile,
        paths: Option<Vec<String>>,
    ) -> Result<(), String> {
        let paths = paths.map(|paths| paths.into_iter().collect::<BTreeSet<_>>());
        if let Some(paths) = &paths {
            if paths.is_empty() || paths.len() > MAX_PATHS || paths.iter().any(|p| !valid_path(p)) {
                return Err("OSC control requires 1–32 concrete Avatar parameter paths".into());
            }
        }
        let mut config = self.config.lock();
        config.control_paths = paths;
        config.peer_profile = profile;
        Ok(())
    }

    pub fn events(&self) -> Vec<TransportEvent> {
        self.shared.lock().events.drain(..).collect()
    }

    pub fn state(&self) -> TransportState {
        self.shared.lock().state.clone()
    }

    pub fn write_parameter(&self, epoch: u64, path: &str, value: OscValue) -> Result<(), String> {
        self.write(epoch, path, value, None)
    }

    /// A bounded group uses one datagram; its consumer owns ordering and pacing.
    pub(crate) fn write_batch(
        &self,
        epoch: u64,
        values: Vec<(String, OscValue)>,
        lease: &WriteLease,
    ) -> Result<bool, String> {
        if values.is_empty()
            || values.len() > 16
            || values.iter().any(|(path, value)| {
                !valid_path(path)
                    || matches!(value, OscValue::String(s) if s.len() > 1024)
                    || matches!(value, OscValue::Float(v) if !v.is_finite())
            })
        {
            return Err("Invalid OSC parameter batch".into());
        }
        if !lease.claim() {
            return Ok(false);
        }
        if let Err(error) = self.enqueue(Command::Batch {
            epoch,
            values,
            lease: lease.clone(),
        }) {
            lease.0.pending.store(false, Ordering::Release);
            return Err(error);
        }
        Ok(true)
    }

    /// The returned snapshot carries query_id only if this epoch was still valid
    /// when the write occurred and when its subsequent query completed.
    pub fn write_and_query(
        &self,
        epoch: u64,
        path: &str,
        value: OscValue,
        query_id: u64,
    ) -> Result<(), String> {
        self.write(epoch, path, value, Some(query_id))
    }

    fn write(
        &self,
        epoch: u64,
        path: &str,
        value: OscValue,
        query_id: Option<u64>,
    ) -> Result<(), String> {
        if !valid_path(path)
            || matches!(&value, OscValue::String(s) if s.len() > 1024)
            || matches!(&value, OscValue::Float(v) if !v.is_finite())
        {
            return Err("Invalid OSC parameter write".into());
        }
        self.enqueue(Command::Write {
            epoch,
            path: path.to_owned(),
            value,
            query_id,
        })
    }

    fn enqueue(&self, command: Command) -> Result<(), String> {
        if self.stop.load(Ordering::Acquire) {
            return Err("OSC transport is stopped".into());
        }
        self.tx.try_send(command).map_err(|error| {
            if matches!(error, mpsc::error::TrySendError::Full(_)) {
                self.overflow.store(true, Ordering::Release);
            }
            "OSC control command queue unavailable; reconnect required".into()
        })
    }
}

pub(crate) struct TransportOwner {
    handle: OscTransportHandle,
    worker: Option<JoinHandle<()>>,
}

impl TransportOwner {
    pub(crate) fn new(settings: &OscSettings) -> Self {
        let config = Arc::new(Mutex::new(Config::new(settings)));
        let shared = Arc::new(Mutex::new(Shared::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let overflow = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel(64);
        let handle = OscTransportHandle {
            muted: Arc::new(AtomicBool::new(false)),
            config,
            shared,
            tx,
            stop,
            overflow,
        };
        let worker_handle = handle.clone();
        let worker = thread::Builder::new()
            .name("osc-shared-transport".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                match runtime {
                    Ok(runtime) => runtime.block_on(Worker::new(worker_handle).run(rx)),
                    Err(error) => {
                        let reason = format!("Cannot start OSC transport runtime: {error}");
                        worker_handle.shared.lock().state.diagnostic = Some(reason.clone());
                        worker_handle.shared.lock().last_error = Some(reason);
                    }
                }
            })
            .expect("failed to start OSC transport owner");
        Self {
            handle,
            worker: Some(worker),
        }
    }

    pub(crate) fn handle(&self) -> OscTransportHandle {
        self.handle.clone()
    }

    pub(crate) fn mute_state(&self) -> Arc<AtomicBool> {
        self.handle.muted.clone()
    }

    pub(crate) fn take_error(&self) -> Option<String> {
        self.handle.shared.lock().last_error.take()
    }

    pub(crate) fn shutdown(&mut self) {
        self.handle.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for TransportOwner {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Endpoint {
    service: String,
    http_port: u16,
    osc: SocketAddr,
}

struct Discovery {
    daemon: ServiceDaemon,
    events: mdns_sd::Receiver<ServiceEvent>,
    registrations: Vec<String>,
    server: tokio::task::JoinHandle<()>,
    candidates: BTreeMap<String, u16>,
}

impl Drop for Discovery {
    fn drop(&mut self) {
        self.server.abort();
        for name in &self.registrations {
            let _ = self.daemon.unregister(name);
        }
        let _ = self.daemon.stop_browse(QUERY_SERVICE);
        if let Ok(done) = self.daemon.shutdown() {
            let _ = done.recv_timeout(Duration::from_millis(500));
        }
    }
}

struct Worker {
    handle: OscTransportHandle,
    config: Option<Config>,
    socket: Option<UdpSocket>,
    discovery: Option<Discovery>,
    endpoint: Option<Endpoint>,
    avatar_id: Option<String>,
    epoch: u64,
    last_query_success: Option<Instant>,
    next_query: Instant,
    retry_configuration: Instant,
    last_udp_values: BTreeMap<String, OscValue>,
    last_mute_datagram: Option<Instant>,
    microphone_pressed_target: Option<String>,
    client: Result<reqwest::Client, String>,
}

impl Worker {
    fn new(handle: OscTransportHandle) -> Self {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(HTTP_TIMEOUT)
            .build()
            .map_err(|e| format!("Cannot create OSCQuery client: {e}"));
        Self {
            handle,
            config: None,
            socket: None,
            discovery: None,
            endpoint: None,
            avatar_id: None,
            epoch: 0,
            last_query_success: None,
            next_query: Instant::now(),
            retry_configuration: Instant::now(),
            last_udp_values: BTreeMap::new(),
            last_mute_datagram: None,
            microphone_pressed_target: None,
            client,
        }
    }

    async fn run(mut self, mut rx: mpsc::Receiver<Command>) {
        while !self.handle.stop.load(Ordering::Acquire) {
            let desired = self.handle.config.lock().clone();
            let needs_retry = self.config.as_ref().is_some_and(|c| {
                c.query_enabled() && (self.socket.is_none() || self.discovery.is_none())
            }) && Instant::now() >= self.retry_configuration;
            if needs_retry {
                self.config = None;
            }
            if self.config.as_ref() != Some(&desired) {
                self.configure(desired).await;
            }
            if self.handle.overflow.swap(false, Ordering::AcqRel) {
                while rx.try_recv().is_ok() {}
                self.invalidate("OSC command queue overflow; binding retired");
            }
            self.discover();
            self.receive();
            self.sync_microphone();
            // A slow OSCQuery must not hold /input/Voice down. Finish its two
            // edges before processing commands that can await an HTTP reply.
            if self.microphone_pressed_target.is_none() {
                for _ in 0..16 {
                    let Ok(command) = rx.try_recv() else { break };
                    self.command(command).await;
                    if self.handle.stop.load(Ordering::Acquire) {
                        break;
                    }
                }
            }
            self.sync_microphone();
            if self.microphone_pressed_target.is_none()
                && self.query_enabled()
                && Instant::now() >= self.next_query
            {
                self.query(None).await;
            }
            if self.endpoint.is_some()
                && self
                    .last_query_success
                    .is_some_and(|t| t.elapsed() >= HEALTH_TIMEOUT)
            {
                self.invalidate("Avatar OSCQuery connection timed out");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        self.invalidate("OSC transport stopped");
        self.discovery = None;
        self.socket = None;
        self.handle.muted.store(false, Ordering::Release);
    }

    fn peer_profile(&self) -> OscPeerProfile {
        self.config
            .as_ref()
            .map(|c| c.peer_profile)
            .unwrap_or_default()
    }

    fn control_config_current(&self) -> bool {
        let desired = self.handle.config.lock();
        self.config.as_ref().is_some_and(|actual| {
            desired.control_paths == actual.control_paths
                && desired.peer_profile == actual.peer_profile
        })
    }

    fn query_config_current(&self) -> bool {
        self.control_config_current() && self.handle.config.lock().query_enabled()
    }

    fn control_enabled(&self) -> bool {
        self.config
            .as_ref()
            .is_some_and(|c| c.control_paths.is_some())
    }

    fn query_enabled(&self) -> bool {
        self.config.as_ref().is_some_and(Config::query_enabled)
    }

    fn observe_microphone(&self, muted: bool) {
        self.handle.muted.store(muted, Ordering::Release);
        self.handle.shared.lock().microphone.observe(muted);
    }

    fn voice_edge(&self, target: &str, pressed: bool) -> Result<(), String> {
        let packet = OscPacket::Message(OscMessage {
            addr: "/input/Voice".into(),
            args: vec![OscType::Int(i32::from(pressed))],
        });
        let bytes = encoder::encode(&packet).map_err(|error| error.to_string())?;
        self.socket
            .as_ref()
            .ok_or_else(|| "OSC receiver unavailable".to_owned())?
            .send_to(&bytes, target)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn release_microphone(&mut self) {
        if let Some(target) = self.microphone_pressed_target.take()
            && let Err(error) = self.voice_edge(&target, false)
        {
            self.error(format!("Cannot release VRChat Voice input: {error}"));
        }
    }

    fn sync_microphone(&mut self) {
        if self.handle.stop.load(Ordering::Acquire) {
            self.release_microphone();
            return;
        }
        let Some(config) = self.config.as_ref() else {
            return;
        };
        if !config.microphone_sync {
            self.release_microphone();
            return;
        }
        if config.peer_profile != OscPeerProfile::default() {
            self.release_microphone();
            self.handle
                .shared
                .lock()
                .microphone
                .fail("Microphone sync requires VRChat with Toggle Voice enabled");
            return;
        }
        let target = self.endpoint.as_ref().map_or_else(
            || format!("{}:{}", config.manual_ip, config.send_port),
            |endpoint| endpoint.osc.to_string(),
        );
        let shared = Arc::clone(&self.handle.shared);
        let mut shared = shared.lock();
        // Hold the short state lock through UDP submission: once disabling sync
        // returns, an already polled opening request cannot still be submitted.
        let edge = shared.microphone.poll(Instant::now());
        let result = match edge {
            Some(Edge::Press) => {
                self.microphone_pressed_target = Some(target.clone());
                self.voice_edge(&target, true)
            }
            Some(Edge::Release) => {
                let result = self
                    .microphone_pressed_target
                    .as_ref()
                    .map_or(Ok(()), |target| self.voice_edge(target, false));
                if result.is_ok() {
                    self.microphone_pressed_target = None;
                }
                result
            }
            None => return,
        };
        if let Err(error) = result {
            shared
                .microphone
                .fail("Could not control the VRChat microphone; check OSC settings");
            drop(shared);
            self.release_microphone();
            self.handle.shared.lock().microphone.disconnected();
            self.error(format!("Cannot control VRChat microphone: {error}"));
        }
    }

    async fn configure(&mut self, config: Config) {
        // Chatbox output and mic-sync toggles do not interrupt an existing
        // OSCQuery receiver when its peer and Avatar subscriptions stay intact.
        let same_control = self.config.as_ref().is_some_and(|old| {
            old.query_enabled()
                && config.query_enabled()
                && old.control_paths == config.control_paths
                && old.peer_profile == config.peer_profile
        });
        self.retry_configuration = Instant::now() + HEALTH_TIMEOUT;
        self.config = Some(config.clone());
        if same_control {
            return;
        }
        self.invalidate("OSC receive configuration changed");
        self.discovery = None;
        self.socket = None;
        self.handle.muted.store(false, Ordering::Release);
        self.handle.shared.lock().state.control_enabled = config.control_paths.is_some();
        if !config.query_enabled() && !config.manual_enabled {
            self.listener("Disabled".into());
            return;
        }
        let address = if config.query_enabled() {
            "127.0.0.1:0".to_owned()
        } else {
            format!("{}:{}", config.manual_ip, config.manual_port)
        };
        let result = UdpSocket::bind(&address).and_then(|socket| {
            socket.set_nonblocking(true)?;
            Ok(socket)
        });
        match result {
            Ok(socket) => {
                let port = socket.local_addr().map(|a| a.port()).unwrap_or(0);
                self.socket = Some(socket);
                self.listener(format!(
                    "Listening on {}",
                    if config.query_enabled() {
                        format!("127.0.0.1:{port} (OSCQuery)")
                    } else {
                        address
                    }
                ));
                if config.query_enabled() {
                    match start_discovery(port).await {
                        Ok(discovery) => {
                            self.discovery = Some(discovery);
                            self.next_query = Instant::now();
                        }
                        Err(error) => self.error(error),
                    }
                }
            }
            Err(error) => self.error(format!("Cannot listen for OSC on {address}: {error}")),
        }
    }

    fn listener(&self, value: String) {
        self.handle.shared.lock().state.listener = value;
    }

    fn error(&self, value: String) {
        self.handle.shared.lock().last_error = Some(value.clone());
        self.handle.shared.lock().state.diagnostic = Some(value);
    }

    fn invalidate(&mut self, reason: &str) {
        self.release_microphone();
        self.handle.shared.lock().microphone.disconnected();
        self.epoch = self
            .epoch
            .checked_add(1)
            .expect("OSC transport epoch exhausted");
        self.endpoint = None;
        self.avatar_id = None;
        self.handle.muted.store(false, Ordering::Release);
        self.last_query_success = None;
        self.last_udp_values.clear();
        self.last_mute_datagram = None;
        self.next_query = Instant::now();
        let mut shared = self.handle.shared.lock();
        shared.state.epoch = self.epoch;
        shared.state.diagnostic = Some(reason.to_owned());
        shared.events.clear();
        shared.events.push_back(TransportEvent::Invalidated {
            epoch: self.epoch,
            reason: reason.to_owned(),
        });
    }

    fn emit(&mut self, event: TransportEvent) {
        if self.handle.shared.lock().events.len() >= MAX_EVENTS {
            self.invalidate("OSC event queue overflow; binding retired");
            return;
        }
        self.handle.shared.lock().events.push_back(event);
    }

    fn discover(&mut self) {
        let profile = self.peer_profile();
        let Some(discovery) = &mut self.discovery else {
            return;
        };
        let mut retired = false;
        for _ in 0..64 {
            let Ok(event) = discovery.events.try_recv() else {
                break;
            };
            match event {
                ServiceEvent::ServiceResolved(service) => {
                    let name = service.get_fullname().to_owned();
                    // Names select a protocol peer, not an authentication identity.
                    // A local-address check happens before any HTTP request.
                    if !profile.accepts_name(name.split("._oscjson.").next().unwrap_or(&name))
                        || service.get_port() == 0
                    {
                        continue;
                    }
                    let local = service
                        .get_addresses()
                        .iter()
                        .any(|ip| local_address(ip.to_ip_addr()));
                    if local
                        && (discovery.candidates.len() < 8
                            || discovery.candidates.contains_key(&name))
                    {
                        let previous = discovery
                            .candidates
                            .insert(name.clone(), service.get_port());
                        if previous.is_some_and(|p| p != service.get_port())
                            && self.endpoint.as_ref().is_some_and(|ep| ep.service == name)
                        {
                            retired = true;
                        }
                    }
                }
                ServiceEvent::ServiceRemoved(_, name) => {
                    discovery.candidates.remove(&name);
                    retired |= self.endpoint.as_ref().is_some_and(|ep| ep.service == name);
                }
                _ => {}
            }
        }
        if retired {
            self.invalidate("Avatar OSCQuery endpoint changed");
        }
    }

    fn receive(&mut self) {
        let mut packets = Vec::new();
        let Some(socket) = &self.socket else { return };
        let mut buffer = [0_u8; 16384];
        for _ in 0..64 {
            match socket.recv_from(&mut buffer) {
                Ok((size, source)) => {
                    if self.query_enabled() && !source.ip().is_loopback() {
                        continue;
                    }
                    if bounded_packet(&buffer[..size], 0, &mut 0) {
                        if let Ok((_, packet)) = decoder::decode_udp(&buffer[..size]) {
                            packets.push(packet);
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    self.error(format!("OSC receiver failed: {e}"));
                    break;
                }
            }
        }
        let mut messages = Vec::new();
        for packet in packets {
            collect_messages(packet, 0, &mut messages);
        }
        // Retire first, regardless of the order of parameter messages in a bundle.
        if self.query_enabled()
            && messages
                .iter()
                .any(|m| self.peer_profile().invalidates_on(&m.addr))
        {
            self.invalidate("Avatar changed or reloaded");
        }
        for message in messages {
            if self.peer_profile().mute_path == Some(message.addr.as_str()) {
                if let Some(value) = message.args.first().and_then(parse_mute_value) {
                    self.last_mute_datagram = Some(Instant::now());
                    self.observe_microphone(value);
                }
            }
            let subscribed = self
                .config
                .as_ref()
                .and_then(|c| c.control_paths.as_ref())
                .is_some_and(|paths| paths.contains(&message.addr));
            if !subscribed || self.endpoint.is_none() || self.avatar_id.is_none() {
                continue;
            }
            let Some(value) = message.args.first().and_then(OscValue::from_osc) else {
                continue;
            };
            if self.last_udp_values.get(&message.addr) == Some(&value) {
                continue;
            }
            self.last_udp_values
                .insert(message.addr.clone(), value.clone());
            self.next_query = self
                .next_query
                .min(Instant::now() + Duration::from_millis(30));
        }
    }

    async fn command(&mut self, command: Command) {
        let (epoch, values, query_id, lease) = match command {
            Command::Write {
                epoch,
                path,
                value,
                query_id,
            } => (epoch, vec![(path, value)], query_id, None),
            Command::Batch {
                epoch,
                values,
                lease,
            } => (epoch, values, None, Some(lease)),
        };
        if lease
            .as_ref()
            .is_some_and(|lease| lease.0.revoked.load(Ordering::Acquire))
        {
            return;
        }
        if self.health_expired() {
            self.invalidate("Avatar OSCQuery connection timed out");
            return;
        }
        // Read config again so revocation does not wait for a slow previous query.
        if !self.control_config_current() || !self.control_enabled() || epoch != self.epoch {
            return;
        }
        let Some(endpoint) = self.endpoint.clone() else {
            return;
        };
        let Some(paths) = self.config.as_ref().and_then(|c| c.control_paths.as_ref()) else {
            return;
        };
        if values.iter().any(|(path, _)| !paths.contains(path)) {
            return;
        }
        let mut packets = values
            .into_iter()
            .map(|(path, value)| {
                OscPacket::Message(OscMessage {
                    addr: path,
                    args: vec![value.osc()],
                })
            })
            .collect::<Vec<_>>();
        let packet = if lease.is_none() {
            packets.pop().unwrap()
        } else {
            OscPacket::Bundle(OscBundle {
                timetag: (0, 1).into(),
                content: packets,
            })
        };
        let encoded = encoder::encode(&packet).map_err(|e| e.to_string());
        let result = encoded.and_then(|bytes| {
            self.socket
                .as_ref()
                .ok_or("OSC receiver unavailable".into())
                .and_then(|socket| {
                    socket
                        .send_to(&bytes, endpoint.osc)
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                })
        });
        if let Some(lease) = lease {
            lease.completed();
        }
        match result {
            Ok(()) if query_id.is_some() => self.query(query_id).await,
            Ok(()) => {}
            Err(e) => self.invalidate(&format!("Cannot send Avatar OSC parameter: {e}")),
        }
    }

    fn health_expired(&self) -> bool {
        self.last_query_success
            .is_some_and(|last| last.elapsed() >= HEALTH_TIMEOUT)
    }

    async fn fetch_peer(
        &self,
        client: &reqwest::Client,
        port: u16,
        path: &str,
    ) -> Result<Value, String> {
        // HOST_INFO and the tree share the existing binding's health deadline;
        // two successive requests cannot extend a failing connection past it.
        let budget = self
            .last_query_success
            .map(|last| HEALTH_TIMEOUT.saturating_sub(last.elapsed()))
            .unwrap_or(HTTP_TIMEOUT)
            .min(HTTP_TIMEOUT);
        tokio::time::timeout(budget, fetch_json(client, port, path))
            .await
            .map_err(|_| "OSCQuery request deadline exceeded".to_owned())?
    }

    async fn query(&mut self, query_id: Option<u64>) {
        self.next_query = Instant::now() + QUERY_INTERVAL;
        if !self.query_enabled() {
            return;
        }
        let candidates = match &self.discovery {
            Some(d) => d.candidates.clone(),
            // A previously verified endpoint remains usable between discovery
            // updates; new peers can only enter through local mDNS discovery.
            None => match &self.endpoint {
                Some(ep) => [(ep.service.clone(), ep.http_port)].into_iter().collect(),
                None => return,
            },
        };
        if candidates.is_empty() {
            return;
        }
        let client = match &self.client {
            Ok(c) => c.clone(),
            Err(e) => {
                self.error(e.clone());
                return;
            }
        };
        // Never pick an arbitrary client when multiple local services match.
        // Bound discovery probing to one peer per query cycle as well.
        let unique_ports = candidates.values().copied().collect::<BTreeSet<_>>();
        if unique_ports.len() != 1 {
            if self.endpoint.is_some() {
                self.invalidate("Multiple local clients found; close the unused client");
            } else {
                self.error("Multiple local clients found; close the unused client".into());
            }
            return;
        }
        let (service, port) = candidates.into_iter().next().unwrap();
        let host = match self.fetch_peer(&client, port, "/?HOST_INFO").await {
            Ok(host) => host,
            Err(error) => {
                self.error(error);
                return;
            }
        };
        let osc = match parse_host_info(&host, self.peer_profile()) {
            Ok(osc) => osc,
            Err(error) => {
                self.invalidate(&error);
                return;
            }
        };
        let endpoint = Endpoint {
            service,
            http_port: port,
            osc,
        };
        // The HTTP service may survive a change to the peer's UDP destination.
        // Retire rather than acknowledge a write delivered to the old endpoint.
        if self.endpoint.as_ref().is_some_and(|old| old != &endpoint) {
            self.invalidate("Avatar OSCQuery destination changed");
            return;
        }
        if self.handle.stop.load(Ordering::Acquire) || !self.query_config_current() {
            return;
        }
        let epoch = self.epoch;
        let queried_at = Instant::now();
        let paths = self
            .config
            .as_ref()
            .and_then(|c| c.control_paths.clone())
            .unwrap_or_default();
        let result = self
            .fetch_peer(&client, endpoint.http_port, "/")
            .await
            .and_then(|tree| parse_avatar_tree(&tree, &paths, self.peer_profile()));
        // A change that arrived during HTTP takes precedence over its snapshot.
        self.discover();
        self.receive();
        if epoch != self.epoch || !self.query_config_current() {
            return;
        }
        match result {
            Ok((avatar_id, nodes)) => {
                if self.health_expired() {
                    self.invalidate("Avatar OSCQuery connection timed out");
                    return;
                }
                if self.avatar_id.as_ref().is_some_and(|id| id != &avatar_id) {
                    self.invalidate("Avatar identity changed");
                    return;
                }
                if self.last_mute_datagram.is_none_or(|at| at <= queried_at)
                    && let Some(OscValue::Bool(value)) = self
                        .peer_profile()
                        .mute_path
                        .and_then(|path| nodes.get(path))
                        .and_then(|node| node.value.as_ref())
                {
                    self.observe_microphone(*value);
                }
                self.endpoint = Some(endpoint);
                self.avatar_id = Some(avatar_id.clone());
                self.last_query_success = Some(Instant::now());
                self.handle.shared.lock().state.diagnostic = None;
                if self.control_enabled() {
                    self.emit(TransportEvent::Snapshot(AvatarSnapshot {
                        epoch,
                        query_id,
                        avatar_id,
                        nodes,
                        queried_at,
                    }));
                }
            }
            Err(error) => {
                self.error(error.clone());
                // Malformed identity/data fails immediately; network timeouts have
                // the bounded health grace period, without publishing stale data.
                if error.starts_with("Invalid OSCQuery") {
                    self.invalidate(&error);
                }
            }
        }
    }
}

fn valid_path(path: &str) -> bool {
    // ChilloutVR's local Animator parameters use a literal '#' prefix.
    path.starts_with("/avatar/parameters/")
        && path.len() > "/avatar/parameters/".len()
        && path.len() <= 256
        && !path
            .chars()
            .any(|c| c.is_control() || matches!(c, '*' | '?' | '[' | ']' | '{' | '}' | ' '))
}

fn local_address(ip: IpAddr) -> bool {
    ip.is_loopback()
        || (!ip.is_unspecified()
            && !ip.is_multicast()
            && UdpSocket::bind(SocketAddr::new(ip, 0)).is_ok())
}

// Check bundle framing before rosc's recursive decoder touches untrusted bytes.
fn bounded_packet(bytes: &[u8], depth: usize, count: &mut usize) -> bool {
    *count += 1;
    if depth > 8 || *count > 128 {
        return false;
    }
    if !bytes.starts_with(b"#bundle\0") {
        return bytes.starts_with(b"/");
    }
    if bytes.len() < 16 {
        return false;
    }
    let mut rest = &bytes[16..];
    while !rest.is_empty() {
        if rest.len() < 4 {
            return false;
        }
        let size = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
        rest = &rest[4..];
        if size == 0 || size > rest.len() || !bounded_packet(&rest[..size], depth + 1, count) {
            return false;
        }
        rest = &rest[size..];
    }
    true
}

fn collect_messages(packet: OscPacket, depth: usize, out: &mut Vec<OscMessage>) {
    if depth > 8 || out.len() >= 128 {
        return;
    }
    match packet {
        OscPacket::Message(message) => out.push(message),
        OscPacket::Bundle(bundle) => {
            for packet in bundle.content {
                collect_messages(packet, depth + 1, out);
            }
        }
    }
}

fn parse_host_info(host: &Value, profile: OscPeerProfile) -> Result<SocketAddr, String> {
    let name = host.get("NAME").and_then(Value::as_str).unwrap_or_default();
    if !profile.accepts_name(name) {
        return Err("Invalid OSCQuery host name".into());
    }
    if host
        .get("OSC_TRANSPORT")
        .and_then(Value::as_str)
        .unwrap_or("UDP")
        != "UDP"
    {
        return Err("Invalid OSCQuery transport; UDP required".into());
    }
    let port = host
        .get("OSC_PORT")
        .and_then(Value::as_u64)
        .filter(|p| (1..=65535).contains(p))
        .ok_or("Invalid OSCQuery UDP port")? as u16;
    let ip: IpAddr = host
        .get("OSC_IP")
        .and_then(Value::as_str)
        .unwrap_or("127.0.0.1")
        .parse()
        .map_err(|_| "Invalid OSCQuery UDP address")?;
    if !local_address(ip) {
        return Err("Invalid OSCQuery remote UDP address".into());
    }
    if !ip.is_ipv4() {
        return Err("Invalid OSCQuery UDP address; this local receiver requires IPv4".into());
    }
    // Honor the advertised address only after proving it belongs to this host.
    Ok(SocketAddr::new(ip, port))
}

async fn fetch_json(client: &reqwest::Client, port: u16, path: &str) -> Result<Value, String> {
    let mut response = client
        .get(format!("http://127.0.0.1:{port}{path}"))
        .header("Cache-Control", "no-cache")
        .send()
        .await
        .map_err(|e| format!("OSCQuery request failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("OSCQuery HTTP error: {e}"))?;
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BODY as u64)
    {
        return Err("Invalid OSCQuery body exceeds limit".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("OSCQuery body failed: {e}"))?
    {
        if bytes.len() + chunk.len() > MAX_BODY {
            return Err("Invalid OSCQuery body exceeds limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|e| format!("Invalid OSCQuery JSON: {e}"))
}

fn parse_avatar_tree(
    tree: &Value,
    paths: &BTreeSet<String>,
    peer: OscPeerProfile,
) -> Result<(String, BTreeMap<String, OscQueryNode>), String> {
    let mut nodes = BTreeMap::new();
    let mut stack = vec![(tree, 0_usize)];
    let mut count = 0;
    while let Some((node, depth)) = stack.pop() {
        count += 1;
        if count > 17000 || depth > 32 {
            return Err("Invalid OSCQuery tree exceeds limits".into());
        }
        let path = node
            .get("FULL_PATH")
            .and_then(Value::as_str)
            .ok_or("Invalid OSCQuery node path")?;
        if path == peer.identity_path || peer.mute_path == Some(path) || paths.contains(path) {
            let osc_type = node
                .get("TYPE")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let value = node
                .get("VALUE")
                .and_then(|v| {
                    if v.is_array() {
                        v.as_array().filter(|values| values.len() == 1)?.first()
                    } else {
                        Some(v)
                    }
                })
                .and_then(|v| match osc_type.as_str() {
                    "T" | "F" => v.as_bool().map(OscValue::Bool),
                    "i" => v
                        .as_i64()
                        .and_then(|i| i32::try_from(i).ok())
                        .map(OscValue::Int),
                    "f" => v
                        .as_f64()
                        .map(|f| f as f32)
                        .filter(|f| f.is_finite())
                        .map(OscValue::Float),
                    "s" => v
                        .as_str()
                        .filter(|s| s.len() <= 1024)
                        .map(|s| OscValue::String(s.to_owned())),
                    _ => None,
                });
            let access = match node.get("ACCESS") {
                Some(access) => access
                    .as_u64()
                    .filter(|a| *a <= 3)
                    .ok_or("Invalid OSCQuery access flags")? as u8,
                None => {
                    if value.is_some() {
                        3
                    } else {
                        2
                    }
                }
            };
            if nodes
                .insert(
                    path.to_owned(),
                    OscQueryNode {
                        osc_type,
                        access,
                        value,
                    },
                )
                .is_some()
            {
                return Err("Invalid OSCQuery duplicate node".into());
            }
        }
        if let Some(children) = node.get("CONTENTS") {
            let children = children.as_object().ok_or("Invalid OSCQuery children")?;
            stack.extend(children.values().map(|child| (child, depth + 1)));
        }
    }
    let avatar_id = match nodes.get(peer.identity_path).and_then(|n| n.value.as_ref()) {
        Some(OscValue::String(id)) if !id.is_empty() && id.len() <= 256 => id.clone(),
        _ => return Err("Invalid OSCQuery snapshot has no current Avatar identity".into()),
    };
    Ok((avatar_id, nodes))
}

async fn start_discovery(udp_port: u16) -> Result<Discovery, String> {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|e| e.to_string())?;
    let http_port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let daemon = ServiceDaemon::new().map_err(|e| format!("Cannot start OSCQuery mDNS: {e}"))?;
    let events = match daemon.browse(QUERY_SERVICE) {
        Ok(events) => events,
        Err(e) => {
            let _ = daemon.shutdown();
            return Err(e.to_string());
        }
    };
    let name = format!("XRTranslate-{}-{udp_port}", std::process::id());
    let hostname = format!("xrtranslate-{}-{udp_port}.local.", std::process::id());
    let mut registrations = Vec::new();
    for (service_type, port) in [(QUERY_SERVICE, http_port), (OSC_SERVICE, udp_port)] {
        let info = match ServiceInfo::new(
            service_type,
            &name,
            &hostname,
            "127.0.0.1",
            port,
            None::<std::collections::HashMap<String, String>>,
        ) {
            Ok(info) => info,
            Err(error) => {
                let _ = daemon.shutdown();
                return Err(error.to_string());
            }
        };
        registrations.push(info.get_fullname().to_owned());
        if let Err(e) = daemon.register(info) {
            let _ = daemon.shutdown();
            return Err(e.to_string());
        }
    }
    let tree = advertised_tree();
    let host_info = json!({ "NAME": name, "OSC_IP": "127.0.0.1", "OSC_PORT": udp_port,
        "OSC_TRANSPORT": "UDP", "EXTENSIONS": { "ACCESS": true } });
    let server = spawn_query_server(listener, host_info, tree);
    Ok(Discovery {
        daemon,
        events,
        registrations,
        server,
        candidates: BTreeMap::new(),
    })
}

fn spawn_query_server(
    listener: tokio::net::TcpListener,
    host_info: Value,
    tree: Value,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Sequential, bounded request handling: this is a tiny read-only local
        // discovery surface, never a general control or file-serving endpoint.
        loop {
            let Ok((mut stream, peer)) = listener.accept().await else {
                return;
            };
            if !peer.ip().is_loopback() {
                continue;
            }
            let _ = tokio::time::timeout(Duration::from_millis(500), async {
                let mut request = Vec::new();
                let mut chunk = [0_u8; 1024];
                while request.len() < 4096 {
                    let count = stream.read(&mut chunk).await?;
                    if count == 0 { break; }
                    request.extend_from_slice(&chunk[..count]);
                    if request.windows(4).any(|v| v == b"\r\n\r\n") { break; }
                }
                let text = String::from_utf8_lossy(&request);
                let mut line = text.lines().next().unwrap_or_default().split_whitespace();
                let method = line.next().unwrap_or_default();
                let target = line.next().unwrap_or_default();
                let (code, body) = if method != "GET" { (405, String::new()) }
                    else if target.ends_with("?HOST_INFO") { (200, host_info.to_string()) }
                    else if target.contains('?') { (204, String::new()) }
                    else if let Some(node) = find_node(&tree, target) { (200, node.to_string()) }
                    else { (404, String::new()) };
                let response = format!("HTTP/1.1 {code} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
                    match code { 200 => "OK", 204 => "No Content", 405 => "Method Not Allowed", _ => "Not Found" }, body.len());
                stream.write_all(response.as_bytes()).await
            }).await;
        }
    })
}

fn advertised_tree() -> Value {
    // The Avatar receiver contract subscribes to the /avatar container. A
    // container has no TYPE or VALUE; leaf types come from the peer's snapshot.
    // Consumer paths are only local filters, never fabricated wire metadata.
    json!({ "FULL_PATH": "/", "ACCESS": 0, "CONTENTS": {
        "avatar": { "FULL_PATH": "/avatar", "ACCESS": 2, "CONTENTS": {} }
    } })
}

fn find_node<'a>(node: &'a Value, path: &str) -> Option<&'a Value> {
    if node.get("FULL_PATH")?.as_str()? == path {
        return Some(node);
    }
    node.get("CONTENTS")?
        .as_object()?
        .values()
        .find_map(|child| find_node(child, path))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONTROL: &str = "/avatar/parameters/ExampleEnabled";

    fn tree(avatar: &str, enabled: Value, osc_type: &str) -> Value {
        json!({"FULL_PATH":"/", "CONTENTS": {
            "avatar": {"FULL_PATH":"/avatar", "CONTENTS": {
                "change": {"FULL_PATH":OscPeerProfile::default().identity_path,"TYPE":"s","ACCESS":1,"VALUE":[avatar]},
                "parameters": {"FULL_PATH":"/avatar/parameters", "CONTENTS": {
                    "ExampleEnabled": {"FULL_PATH":CONTROL,"TYPE":osc_type,"ACCESS":3,"VALUE":[enabled]}
                }}
            }}
        }})
    }

    fn worker(http_port: u16, osc_port: u16) -> Worker {
        let (tx, _) = mpsc::channel(64);
        let mut config = Config::new(&OscSettings::default());
        config.control_paths = Some([CONTROL.to_owned()].into_iter().collect());
        let handle = OscTransportHandle {
            muted: Arc::new(AtomicBool::new(false)),
            config: Arc::new(Mutex::new(config.clone())),
            shared: Arc::new(Mutex::new(Shared::default())),
            tx,
            stop: Arc::new(AtomicBool::new(false)),
            overflow: Arc::new(AtomicBool::new(false)),
        };
        let mut worker = Worker::new(handle);
        worker.config = Some(config);
        worker.socket = Some(UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap());
        worker
            .socket
            .as_ref()
            .unwrap()
            .set_nonblocking(true)
            .unwrap();
        worker.epoch = 7;
        worker.handle.shared.lock().state.epoch = 7;
        worker.endpoint = Some(Endpoint {
            service: "VRChat-test._oscjson._tcp.local.".into(),
            http_port,
            osc: SocketAddr::from((Ipv4Addr::LOCALHOST, osc_port)),
        });
        worker.avatar_id = Some("avtr_a".into());
        worker.last_query_success = Some(Instant::now());
        worker
    }

    /// Real sockets exercise write -> fresh HTTP -> tagged snapshot. The same
    /// production HTTP responder also exposes the receiver's advertised tree.
    #[tokio::test]
    async fn oscquery_loopback_write_query_and_advertisement() {
        let udp = tokio::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let udp_port = udp.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let mut bytes = [0; 2048];
            // The mock does not serve the query until it observes the write.
            let size = tokio::time::timeout(Duration::from_secs(2), udp.recv(&mut bytes))
                .await
                .unwrap()
                .unwrap();
            let (_, packet) = decoder::decode_udp(&bytes[..size]).unwrap();
            assert!(
                matches!(packet, OscPacket::Message(OscMessage { addr, args })
                if addr == CONTROL && args == vec![OscType::Bool(false)])
            );
            for expected in ["/?HOST_INFO", "/"] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let count = stream.read(&mut bytes).await.unwrap();
                assert!(
                    String::from_utf8_lossy(&bytes[..count])
                        .starts_with(&format!("GET {expected} HTTP/1.1"))
                );
                let body = if expected == "/?HOST_INFO" {
                    json!({"NAME":"VRChat-test", "OSC_PORT":udp_port, "OSC_IP":"127.0.0.1", "OSC_TRANSPORT":"UDP"})
                } else { tree("avtr_a", json!(false), "F") }.to_string();
                stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            }
        });
        let mut worker = worker(port, udp_port);
        let before_write = Instant::now();
        worker
            .command(Command::Write {
                epoch: 7,
                path: CONTROL.into(),
                value: OscValue::Bool(false),
                query_id: Some(91),
            })
            .await;
        server.await.unwrap();
        let events = worker.handle.events();
        assert!(
            matches!(events.as_slice(), [TransportEvent::Snapshot(snapshot)]
            if snapshot.epoch == 7 && snapshot.query_id == Some(91)
                && snapshot.queried_at >= before_write
                && snapshot.nodes[CONTROL].value == Some(OscValue::Bool(false)))
        );
        // An obsolete command cannot write or create a token acknowledgement.
        worker
            .command(Command::Write {
                epoch: 6,
                path: CONTROL.into(),
                value: OscValue::Bool(true),
                query_id: Some(92),
            })
            .await;
        assert!(worker.handle.events().is_empty());

        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let host = json!({"NAME":"XRTranslate-test", "OSC_IP":"127.0.0.1", "OSC_PORT":udp_port, "OSC_TRANSPORT":"UDP"});
        let server = spawn_query_server(listener, host.clone(), advertised_tree());
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        assert_eq!(
            fetch_json(&client, port, "/?HOST_INFO").await.unwrap(),
            host
        );
        let avatar = fetch_json(&client, port, "/avatar").await.unwrap();
        assert_eq!(avatar["FULL_PATH"], "/avatar");
        assert_eq!(avatar["ACCESS"], 2);
        assert!(avatar.get("TYPE").is_none());
        server.abort();
    }

    #[tokio::test]
    async fn oscquery_identity_change_and_overflow_retire_old_binding() {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = spawn_query_server(
            listener,
            json!({"NAME":"VRChat-test", "OSC_PORT":9000}),
            tree("avtr_b", json!(true), "T"),
        );
        let mut worker = worker(port, 9000);
        worker.query(Some(99)).await;
        assert!(worker.avatar_id.is_none());
        assert!(matches!(
            worker.handle.events().as_slice(),
            [TransportEvent::Invalidated { epoch: 8, .. }]
        ));
        server.abort();
        // A queue overflow retains only the invalidation, never stale snapshots.
        for _ in 0..=MAX_EVENTS {
            worker.emit(TransportEvent::Snapshot(AvatarSnapshot {
                epoch: worker.epoch,
                query_id: None,
                avatar_id: "avtr_a".into(),
                nodes: BTreeMap::new(),
                queried_at: Instant::now(),
            }));
        }
        assert!(matches!(
            worker.handle.events().as_slice(),
            [TransportEvent::Invalidated { epoch: 9, .. }]
        ));
        assert!(worker.endpoint.is_none());

        // A write sent to the old UDP port must never receive a reset token
        // acknowledgement after HOST_INFO announces a different destination.
        let old_udp = tokio::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let new_udp = tokio::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = spawn_query_server(
            listener,
            json!({"NAME":"VRChat-test", "OSC_PORT":new_udp.local_addr().unwrap().port()}),
            tree("avtr_a", json!(false), "F"),
        );
        let mut changed_peer = self::worker(port, old_udp.local_addr().unwrap().port());
        changed_peer
            .command(Command::Write {
                epoch: 7,
                path: CONTROL.into(),
                value: OscValue::Bool(false),
                query_id: Some(100),
            })
            .await;
        assert!(matches!(
            changed_peer.handle.events().as_slice(),
            [TransportEvent::Invalidated { epoch: 8, .. }]
        ));
        assert!(changed_peer.endpoint.is_none());
        server.abort();
    }

    #[test]
    fn oscquery_peer_profiles_keep_identity_and_writes_separate() {
        let cvr = xrt_connector_core::Connector::Chilloutvr
            .osc()
            .unwrap()
            .peer;
        let vrchat = OscPeerProfile::default();
        let host = json!({"NAME":"ChilloutVR-ABCDEF", "OSC_PORT":9000});
        assert!(parse_host_info(&host, cvr).is_ok());
        assert!(parse_host_info(&host, vrchat).is_err());
        assert!(!cvr.accepts_name("ChilloutVRFake"));
        assert!(cvr.invalidates_on("/avatar/profile/change"));
        assert!(!vrchat.invalidates_on("/avatar/profile/change"));
        assert!(valid_path("/avatar/parameters/#XRT/Enabled"));
        let local_parameter = OscPacket::Message(OscMessage {
            addr: "/avatar/parameters/#XRT/Enabled".into(),
            args: vec![OscType::Bool(false)],
        });
        let bytes = encoder::encode(&local_parameter).unwrap();
        assert_eq!(decoder::decode_udp(&bytes).unwrap().1, local_parameter);
        let worker = worker(9002, 9000);
        assert!(worker.control_config_current());
        worker
            .handle
            .set_control_profile(cvr, Some(vec![CONTROL.to_owned()]))
            .unwrap();
        // Revocation takes effect before a slow HTTP query or worker config pass.
        assert!(!worker.control_config_current());
    }

    #[tokio::test]
    async fn parameter_batches_are_paced_from_delivery_and_revocable() {
        let udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        udp.set_nonblocking(true).unwrap();
        let mut worker = worker(9002, udp.local_addr().unwrap().port());
        let lease = WriteLease::new(Duration::from_millis(200));
        assert!(lease.claim());
        assert!(!lease.claim()); // A slow discovery query cannot accumulate frames.
        let values = vec![(CONTROL.to_owned(), OscValue::Int(7))];
        worker
            .command(Command::Batch {
                epoch: 7,
                values: values.clone(),
                lease: lease.clone(),
            })
            .await;
        let mut bytes = [0; 2048];
        let size = udp.recv(&mut bytes).unwrap();
        assert!(matches!(decoder::decode_udp(&bytes[..size]).unwrap().1,
            OscPacket::Bundle(bundle) if bundle.content.len() == 1));
        assert!(!lease.claim()); // Hold begins after the worker actually sent it.

        let obsolete = WriteLease::default();
        obsolete.revoke();
        worker
            .command(Command::Batch {
                epoch: 7,
                values,
                lease: obsolete,
            })
            .await;
        assert_eq!(
            udp.recv(&mut bytes).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn oscquery_types_identity_and_bundle_bounds() {
        let paths = [CONTROL.to_owned()].into_iter().collect();
        let (_, nodes) = parse_avatar_tree(
            &tree("avtr_a", json!(true), "T"),
            &paths,
            OscPeerProfile::default(),
        )
        .unwrap();
        assert_eq!(nodes[CONTROL].value, Some(OscValue::Bool(true)));
        let (_, nodes) = parse_avatar_tree(
            &tree("avtr_a", json!(true), "i"),
            &paths,
            OscPeerProfile::default(),
        )
        .unwrap();
        assert_eq!(nodes[CONTROL].value, None); // The consumer rejects this schema.
        assert!(
            parse_avatar_tree(
                &tree("", json!(false), "F"),
                &paths,
                OscPeerProfile::default()
            )
            .is_err()
        );
        let mut malformed = tree("avtr_a", json!(false), "F");
        malformed["CONTENTS"]["avatar"]["CONTENTS"]["parameters"]["CONTENTS"]["ExampleEnabled"]["ACCESS"] =
            json!(8);
        assert!(parse_avatar_tree(&malformed, &paths, OscPeerProfile::default()).is_err());
        assert!(
            parse_host_info(
                &json!({"NAME":"VRChat", "OSC_PORT":9000,"OSC_IP":"192.0.2.5"}),
                OscPeerProfile::default()
            )
            .is_err()
        );
        let message = encoder::encode(&OscPacket::Message(OscMessage {
            addr: CONTROL.into(),
            args: vec![OscType::Bool(true)],
        }))
        .unwrap();
        assert!(bounded_packet(&message, 0, &mut 0));
        assert!(!bounded_packet(&message, 9, &mut 0));
        assert!(!bounded_packet(b"#bundle\0", 0, &mut 0));
    }
}
