use super::{BigWalkSettings, chat_text};
#[cfg(test)]
#[path = "tests.rs"]
mod tests;
use crate::session_coordinator::{HostOutputEvent, HostOutputSubscriber};
use parking_lot::{Condvar, Mutex};
use serde::Deserialize;
use std::{
    collections::{BTreeSet, VecDeque},
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpStream},
    sync::Arc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const PORT: u16 = 38647;
const MAX_PENDING: usize = 4;
const MAX_REMEMBERED: usize = 8192;
const MAX_FRAME: usize = 16 * 1024;
const MAX_TEXT: usize = 256;
const MESSAGE_LIFETIME: Duration = Duration::from_secs(8);
const SEND_INTERVAL: Duration = Duration::from_secs(2);
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(4);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConnectionStatus {
    Disabled,
    Connecting,
    WaitingForPlayer,
    Ready,
}

#[derive(Clone)]
pub(crate) struct BigWalkSnapshot {
    pub status: ConnectionStatus,
    pub queued: usize,
    pub skipped: u64,
    pub last_sent: String,
    pub error: Option<String>,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct TranslationId(u64, String, u32);

struct Pending {
    stream: Option<u64>,
    text: String,
    created: Instant,
}

struct State {
    settings: BigWalkSettings,
    enabled: bool,
    shutdown: bool,
    generation: u64,
    snapshot: BigWalkSnapshot,
    limit: usize,
    pending: VecDeque<Pending>,
    remembered: BTreeSet<TranslationId>,
    remembered_order: VecDeque<TranslationId>,
}

impl State {
    fn new(settings: &BigWalkSettings, host_enabled: bool) -> Self {
        Self {
            settings: settings.clone(),
            enabled: host_enabled,
            shutdown: false,
            generation: 0,
            snapshot: BigWalkSnapshot {
                status: if host_enabled {
                    ConnectionStatus::Connecting
                } else {
                    ConnectionStatus::Disabled
                },
                queued: 0,
                skipped: 0,
                last_sent: String::new(),
                error: None,
            },
            limit: MAX_TEXT,
            pending: VecDeque::new(),
            remembered: BTreeSet::new(),
            remembered_order: VecDeque::new(),
        }
    }

    fn remember(&mut self, id: TranslationId) -> bool {
        if !self.remembered.insert(id.clone()) {
            return false;
        }
        self.remembered_order.push_back(id);
        if self.remembered_order.len() > MAX_REMEMBERED {
            self.remembered
                .remove(&self.remembered_order.pop_front().unwrap());
        }
        true
    }

    fn discard_pending(&mut self) {
        self.snapshot.skipped += self.pending.len() as u64;
        self.pending.clear();
    }

    fn expire(&mut self, now: Instant) {
        while self
            .pending
            .front()
            .is_some_and(|item| now.duration_since(item.created) >= MESSAGE_LIFETIME)
        {
            self.pending.pop_front();
            self.snapshot.skipped += 1;
        }
    }

    fn enqueue(&mut self, stream: Option<u64>, text: String) -> bool {
        self.expire(Instant::now());
        if text.is_empty() {
            return false;
        }
        if !self.enabled
            || self.snapshot.status != ConnectionStatus::Ready
            || self.pending.len() >= MAX_PENDING
        {
            if self.enabled {
                self.snapshot.skipped += 1;
            }
            return false;
        }
        self.pending.push_back(Pending {
            stream,
            text,
            created: Instant::now(),
        });
        true
    }
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

#[derive(Clone)]
pub(crate) struct Publisher(Arc<Shared>);

impl HostOutputSubscriber for Publisher {
    fn on_host_output(&self, event: HostOutputEvent<'_>) {
        let mut state = self.0.state.lock();
        match event {
            HostOutputEvent::CommittedTranslation {
                stream_id,
                turn_id,
                segment_index,
                translated,
                source,
                audio_source,
                is_typing,
                additional_translations,
                ..
            } => {
                // Remember even filtered/offline results so a later replacement
                // batch or output toggle cannot replay those same identities.
                if state.remember(TranslationId(stream_id, turn_id.to_owned(), segment_index))
                    && state.settings.audio_sources.allows(audio_source, is_typing)
                {
                    let text = chat_text(
                        source,
                        translated,
                        additional_translations
                            .iter()
                            .map(|item| item.translated_text.as_str()),
                        state.settings.format,
                        state.limit,
                    );
                    state.enqueue(Some(stream_id), text);
                }
            }
            HostOutputEvent::StreamCancelled(stream) => {
                state.pending.retain(|item| item.stream != Some(stream));
            }
            HostOutputEvent::Clear => state.discard_pending(),
            // Previews and caption revisions must never create chat messages.
            HostOutputEvent::Previews { .. }
            | HostOutputEvent::Caption { .. }
            | HostOutputEvent::StreamEnded(_) => {}
        }
        self.0.wake.notify_one();
    }
}

pub(super) struct Runtime {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

impl Runtime {
    pub fn new(settings: &BigWalkSettings, host_enabled: bool) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::new(settings, host_enabled)),
            wake: Condvar::new(),
        });
        let worker_shared = shared.clone();
        let worker = thread::Builder::new()
            .name("bigwalk-chat".into())
            .spawn(move || run(worker_shared));
        if let Err(error) = &worker {
            shared.state.lock().snapshot.error = Some(error.to_string());
        }
        Self {
            shared,
            worker: worker.ok(),
        }
    }

    pub fn publisher(&self) -> Publisher {
        Publisher(self.shared.clone())
    }

    pub fn configure(&self, settings: &BigWalkSettings, host_enabled: bool) {
        let mut state = self.shared.state.lock();
        if state.settings == *settings && state.enabled == host_enabled {
            return;
        }
        state.settings = settings.clone();
        state.enabled = host_enabled;
        state.generation = state.generation.wrapping_add(1);
        state.discard_pending();
        state.snapshot.status = if host_enabled {
            ConnectionStatus::Connecting
        } else {
            ConnectionStatus::Disabled
        };
        state.snapshot.error = None;
        self.shared.wake.notify_one();
    }

    pub fn snapshot(&self) -> BigWalkSnapshot {
        let state = self.shared.state.lock();
        let mut snapshot = state.snapshot.clone();
        snapshot.queued = state.pending.len();
        snapshot
    }

    pub fn send_manual(&self, text: &str) -> bool {
        let mut state = self.shared.state.lock();
        let text = chat_text(
            "",
            text,
            std::iter::empty(),
            super::MessageFormat::TargetOnly,
            state.limit,
        );
        let accepted = state.enqueue(None, text);
        self.shared.wake.notify_one();
        accepted
    }

    pub fn clear(&self) {
        self.shared.state.lock().discard_pending();
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.shared.state.lock().shutdown = true;
        self.shared.wake.notify_one();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Reply {
    State {
        protocol: u8,
        session: String,
        ready: bool,
        max_utf16: usize,
        error: Option<String>,
    },
    Result {
        id: String,
        status: String,
    },
}

struct InFlight {
    id: String,
    text: String,
    sent: Instant,
}

fn run(shared: Arc<Shared>) {
    loop {
        let generation = {
            let mut state = shared.state.lock();
            while !state.enabled && !state.shutdown {
                shared.wake.wait(&mut state);
            }
            if state.shutdown {
                return;
            }
            state.generation
        };
        let endpoint = SocketAddr::from((Ipv4Addr::LOCALHOST, PORT));
        let error = match TcpStream::connect_timeout(&endpoint, Duration::from_millis(300)) {
            Ok(stream) => drive_connection(&shared, generation, stream).err(),
            Err(_) => None,
        };
        let mut state = shared.state.lock();
        if state.generation == generation {
            state.snapshot.status = if state.enabled {
                ConnectionStatus::Connecting
            } else {
                ConnectionStatus::Disabled
            };
            state.discard_pending();
            if error.is_some() {
                state.snapshot.error = error;
            }
        }
        // A burst of output events must not accelerate reconnect attempts.
        let retry = Instant::now() + Duration::from_secs(1);
        while !state.shutdown && state.generation == generation && Instant::now() < retry {
            shared
                .wake
                .wait_for(&mut state, retry.saturating_duration_since(Instant::now()));
        }
        if state.shutdown {
            return;
        }
    }
}

fn drive_connection(shared: &Shared, generation: u64, mut stream: TcpStream) -> Result<(), String> {
    stream.set_nonblocking(true).map_err(|e| e.to_string())?;
    let _ = stream.set_nodelay(true);
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 2048];
    let mut session = String::new();
    let mut last_state = Instant::now();
    let mut next_send = Instant::now();
    let mut in_flight: Option<InFlight> = None;
    loop {
        {
            let state = shared.state.lock();
            if state.shutdown || !state.enabled || state.generation != generation {
                return Ok(());
            }
        }
        match stream.read(&mut chunk) {
            Ok(0) => return Err("Game connection closed.".into()),
            Ok(count) => buffer.extend_from_slice(&chunk[..count]),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => return Err("Game connection closed.".into()),
        }
        if buffer.len() > MAX_FRAME {
            return Err("The game bridge sent an invalid response.".into());
        }
        while let Some(end) = buffer.iter().position(|&byte| byte == b'\n') {
            let reply: Reply = serde_json::from_slice(&buffer[..end])
                .map_err(|_| "The game bridge sent an invalid response.")?;
            buffer.drain(..=end);
            let mut state = shared.state.lock();
            if state.generation != generation || state.shutdown {
                return Ok(());
            }
            match reply {
                Reply::State {
                    protocol,
                    session: current,
                    ready,
                    max_utf16,
                    error,
                } => {
                    if protocol != 1
                        || current.is_empty()
                        || current.len() > 128
                        || !(16..=4096).contains(&max_utf16)
                    {
                        return Err("Update the Big Walk bridge to connect.".into());
                    }
                    last_state = Instant::now();
                    if current != session || !ready {
                        state.discard_pending();
                        in_flight = None;
                    }
                    session = current;
                    state.limit = max_utf16.min(MAX_TEXT);
                    state.snapshot.status = if ready {
                        ConnectionStatus::Ready
                    } else {
                        ConnectionStatus::WaitingForPlayer
                    };
                    state.snapshot.error = error;
                }
                Reply::Result { id, status } => {
                    if in_flight.as_ref().is_some_and(|item| item.id == id) {
                        let item = in_flight.take().unwrap();
                        next_send = Instant::now() + SEND_INTERVAL;
                        if status == "sent" {
                            state.snapshot.last_sent = item.text;
                            state.snapshot.error = None;
                        } else {
                            state.snapshot.skipped += 1;
                            state.snapshot.error =
                                Some("The game did not accept this message.".into());
                        }
                    }
                }
            }
        }
        if in_flight
            .as_ref()
            .is_some_and(|item| item.sent.elapsed() >= CONNECTION_TIMEOUT)
        {
            // A missing acknowledgment is ambiguous. Never resend its text.
            shared.state.lock().snapshot.skipped += 1;
            return Err("Message delivery could not be confirmed.".into());
        }
        if last_state.elapsed() >= CONNECTION_TIMEOUT {
            return Err("The game bridge stopped responding.".into());
        }
        let mut state = shared.state.lock();
        if state.generation != generation || state.shutdown {
            return Ok(());
        }
        state.expire(Instant::now());
        if state.snapshot.status == ConnectionStatus::Ready
            && in_flight.is_none()
            && Instant::now() >= next_send
            && let Some(item) = state.pending.pop_front()
        {
            let id = uuid::Uuid::new_v4().to_string();
            let mut bytes = serde_json::to_vec(
                &serde_json::json!({"type":"chat", "session":session, "id":id, "text":item.text}),
            )
            .map_err(|e| e.to_string())?;
            bytes.push(b'\n');
            // One nonblocking write under the cancellation fence. Partial or
            // ambiguous writes close the connection without replaying the text.
            if stream.write(&bytes).ok() != Some(bytes.len()) {
                state.snapshot.skipped += 1;
                return Err("Message delivery could not be confirmed.".into());
            }
            in_flight = Some(InFlight {
                id,
                text: item.text,
                sent: Instant::now(),
            });
        }
        shared.wake.wait_for(&mut state, Duration::from_millis(50));
    }
}
