//! One paired local connector. Blocking socket work never runs on the UI thread.
use super::connectors::Connector;
use crossbeam_channel::{Receiver, Sender, bounded};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    io::{ErrorKind, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use tungstenite::{Message, WebSocket, accept_hdr_with_config, protocol::WebSocketConfig};

pub(super) enum Event {
    Connected { session: String, captions: bool },
    ResetAck { session: String, reset: u64 },
    SetEnabled { session: String, enabled: bool },
    Disconnected { session: String },
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientFrame {
    Hello {
        protocol: u32,
        connector: String,
        token: String,
        client_id: String,
        #[serde(default)]
        capabilities: Vec<String>,
    },
    ResetAck {
        session: String,
        reset: u64,
        enabled: bool,
    },
    SetEnabled {
        session: String,
        sequence: u64,
        enabled: bool,
    },
    Ping {
        session: String,
    },
}

struct Outbound {
    session: String,
    value: Value,
    caption_epoch: Option<u64>,
}

impl Outbound {
    fn is_current(&self, session: &str, epoch: &AtomicU64) -> bool {
        self.session == session
            && self
                .caption_epoch
                .is_none_or(|value| value == epoch.load(Ordering::Acquire))
    }
}

#[derive(Clone)]
pub(super) struct SenderHandle {
    frames: Sender<Outbound>,
    caption_epoch: Arc<AtomicU64>,
    failed: Arc<AtomicBool>,
}
impl SenderHandle {
    pub fn send(&self, session: &str, mut value: Value) -> Result<(), String> {
        value["session"] = session.into();
        let caption_epoch = match value["type"].as_str() {
            Some("clear") => {
                self.caption_epoch.fetch_add(1, Ordering::AcqRel);
                None
            }
            Some("caption") => Some(self.caption_epoch.load(Ordering::Acquire)),
            _ => None,
        };
        self.frames
            .try_send(Outbound {
                session: session.to_owned(),
                value,
                caption_epoch,
            })
            .map_err(|_| {
                self.failed.store(true, Ordering::Release);
                "Avatar connector output queue is full or disconnected".into()
            })
    }
}

pub(super) struct Runtime {
    pub url: String,
    pub token: String,
    pub sender: SenderHandle,
    events: Receiver<Event>,
    running: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
}
impl Runtime {
    pub fn start(connector: Connector) -> Result<Self, String> {
        let listener =
            TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let url = format!(
            "ws://127.0.0.1:{}/xrt",
            listener.local_addr().map_err(|e| e.to_string())?.port()
        );
        let token = uuid::Uuid::new_v4().simple().to_string();
        let (tx, rx) = bounded(64);
        let (events_tx, events) = bounded(64);
        let running = Arc::new(AtomicBool::new(true));
        let failed = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let caption_epoch = Arc::new(AtomicU64::new(0));
        let sender = SenderHandle {
            frames: tx,
            failed: failed.clone(),
            caption_epoch: caption_epoch.clone(),
        };
        let run = running.clone();
        let worker_error = error.clone();
        let secret = token.clone();
        thread::Builder::new()
            .name("avatar-connector".into())
            .spawn(move || {
                while run.load(Ordering::Acquire) && !failed.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((socket, address)) if address.ip().is_loopback() => {
                            // Old session output is never delivered to a new connection.
                            while rx.try_recv().is_ok() {}
                            if let Some(mut peer) =
                                handshake(socket, connector, &secret, run.clone())
                            {
                                let session = peer.session.clone();
                                if events_tx
                                    .try_send(Event::Connected {
                                        session: session.clone(),
                                        captions: peer.captions,
                                    })
                                    .is_err()
                                {
                                    break;
                                }
                                peer.run(&run, &failed, &caption_epoch, &rx, &events_tx);
                                if events_tx.try_send(Event::Disconnected { session }).is_err() {
                                    break;
                                }
                            }
                        }
                        Ok(_) => {}
                        Err(e) if e.kind() == ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(25))
                        }
                        Err(_) => break,
                    }
                }
                *worker_error.lock() = Some(
                    "Avatar connector disconnected; disable and enable the plugin to reconnect"
                        .into(),
                );
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            url,
            token,
            sender,
            events,
            running,
            error,
        })
    }
    pub fn events(&self) -> impl Iterator<Item = Event> + '_ {
        self.events.try_iter()
    }
    pub fn error(&self) -> Option<String> {
        self.error.lock().clone()
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
    }
}

struct Peer {
    socket: WebSocket<GuardedStream>,
    session: String,
    captions: bool,
}
/// A whole-handshake deadline survives fragmented reads; stop is checked on I/O.
struct GuardedStream {
    socket: TcpStream,
    deadline: Option<Instant>,
    running: Arc<AtomicBool>,
}
impl GuardedStream {
    fn check(&self) -> std::io::Result<()> {
        if !self.running.load(Ordering::Acquire)
            || self
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
        {
            Err(std::io::Error::new(
                ErrorKind::TimedOut,
                "Connector handshake stopped",
            ))
        } else {
            Ok(())
        }
    }
}
impl Read for GuardedStream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.check()?;
        self.socket.read(buffer)
    }
}
impl Write for GuardedStream {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.check()?;
        self.socket.write(buffer)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.check()?;
        self.socket.flush()
    }
}
fn handshake(
    socket: TcpStream,
    connector: Connector,
    secret: &str,
    running: Arc<AtomicBool>,
) -> Option<Peer> {
    socket
        .set_read_timeout(Some(Duration::from_millis(200)))
        .ok()?;
    socket
        .set_write_timeout(Some(Duration::from_millis(200)))
        .ok()?;
    let socket = GuardedStream {
        socket,
        deadline: Some(Instant::now() + Duration::from_secs(2)),
        running,
    };
    let config = WebSocketConfig::default()
        .max_message_size(Some(16 * 1024))
        .max_frame_size(Some(16 * 1024));
    let mut ws = accept_hdr_with_config(
        socket,
        |request: &tungstenite::handshake::server::Request, response| {
            if request.uri().path() != "/xrt" || request.headers().contains_key("origin") {
                return Err(tungstenite::http::Response::builder()
                    .status(403)
                    .body(Some("Connector clients only".into()))
                    .unwrap());
            }
            Ok(response)
        },
        Some(config),
    )
    .ok()?;
    let Message::Text(text) = ws.read().ok()? else {
        return None;
    };
    let ClientFrame::Hello {
        protocol,
        connector: id,
        token,
        client_id,
        capabilities,
    } = serde_json::from_str(&text).ok()?
    else {
        return None;
    };
    if protocol != 1
        || id != connector.id()
        || token != secret
        || client_id.is_empty()
        || client_id.len() > 128
        || capabilities.len() > 8
    {
        return None;
    }
    let session = uuid::Uuid::new_v4().simple().to_string();
    ws.send(Message::Text(
        json!({"type":"welcome", "protocol":1, "session":session})
            .to_string()
            .into(),
    ))
    .ok()?;
    ws.get_mut().deadline = None;
    ws.get_mut()
        .socket
        .set_read_timeout(Some(Duration::from_millis(25)))
        .ok()?;
    Some(Peer {
        socket: ws,
        session,
        captions: capabilities.iter().any(|c| c == "captions"),
    })
}
impl Peer {
    fn run(
        &mut self,
        running: &AtomicBool,
        failed: &AtomicBool,
        caption_epoch: &AtomicU64,
        output: &Receiver<Outbound>,
        events: &Sender<Event>,
    ) {
        let mut last_seen = Instant::now();
        let mut sequence = 0;
        while running.load(Ordering::Acquire)
            && !failed.load(Ordering::Acquire)
            && last_seen.elapsed() < Duration::from_secs(15)
        {
            for frame in output.try_iter().take(64) {
                if !frame.is_current(&self.session, caption_epoch) {
                    continue;
                }
                if self
                    .socket
                    .send(Message::Text(frame.value.to_string().into()))
                    .is_err()
                {
                    return;
                }
            }
            let frame = match self.socket.read() {
                Ok(Message::Text(text)) => match serde_json::from_str::<ClientFrame>(&text) {
                    Ok(frame) => frame,
                    Err(_) => return,
                },
                Ok(Message::Close(_)) => return,
                Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => continue,
                Ok(_) => return,
                Err(tungstenite::Error::Io(e))
                    if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
                {
                    continue;
                }
                Err(_) => return,
            };
            let event = match frame {
                ClientFrame::Ping { session } if session == self.session => {
                    if self
                        .socket
                        .send(Message::Text(
                            json!({"type":"pong", "session":session}).to_string().into(),
                        ))
                        .is_err()
                    {
                        return;
                    }
                    None
                }
                ClientFrame::ResetAck {
                    session,
                    reset,
                    enabled: false,
                } if session == self.session => Some(Event::ResetAck { session, reset }),
                ClientFrame::SetEnabled {
                    session,
                    sequence: seq,
                    enabled,
                } if session == self.session && seq > sequence && seq <= 9_007_199_254_740_991 => {
                    sequence = seq;
                    Some(Event::SetEnabled { session, enabled })
                }
                _ => return,
            };
            last_seen = Instant::now();
            if let Some(event) = event
                && events.try_send(event).is_err()
            {
                failed.store(true, Ordering::Release);
                return;
            }
        }
        let _ = self.socket.send(Message::Text(
            json!({"type":"clear", "session":self.session})
                .to_string()
                .into(),
        ));
        let _ = self.socket.close(None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clear_revokes_queued_captions_but_preserves_control_frames() {
        let (frames, queue) = bounded(8);
        let epoch = Arc::new(AtomicU64::new(0));
        let sender = SenderHandle {
            frames,
            failed: Arc::new(AtomicBool::new(false)),
            caption_epoch: epoch.clone(),
        };
        sender
            .send("old", json!({"type":"caption", "text":"old result"}))
            .unwrap();
        sender
            .send("old", json!({"type":"state", "status":"stopping"}))
            .unwrap();
        sender.send("old", json!({"type":"clear"})).unwrap();
        let caption = queue.recv().unwrap();
        assert!(!caption.is_current("old", &epoch));
        let control = queue.recv().unwrap();
        assert!(control.is_current("old", &epoch));
        assert!(!control.is_current("new", &epoch));
        assert!(queue.recv().unwrap().is_current("old", &epoch));
    }

    #[test]
    fn paired_loopback_validates_identity_sequence_and_reset_messages() {
        let runtime = Runtime::start(Connector::Overte).unwrap();
        let open = || {
            let (mut ws, _) = tungstenite::connect(&runtime.url).unwrap();
            if let tungstenite::stream::MaybeTlsStream::Plain(socket) = ws.get_mut() {
                socket
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
            }
            ws
        };
        let hello = |token: &str| json!({"type":"hello","protocol":1,"connector":"overte","token":token,"client_id":"fixture","capabilities":["captions"]});
        let mut rejected = open();
        rejected
            .send(Message::Text(hello("wrong").to_string().into()))
            .unwrap();
        assert!(rejected.read().is_err());
        let mut ws = open();
        ws.send(Message::Text(hello(&runtime.token).to_string().into()))
            .unwrap();
        let welcome: Value = serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
        let session = welcome["session"].as_str().unwrap();
        assert!(matches!(
            runtime.events.recv_timeout(Duration::from_secs(2)).unwrap(),
            Event::Connected { captions: true, .. }
        ));
        runtime
            .sender
            .send(session, json!({"type":"reset","reset":7}))
            .unwrap();
        let reset: Value = serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
        assert_eq!(reset["reset"], 7);
        ws.send(Message::Text(
            json!({"type":"reset_ack","session":session,"reset":7,"enabled":false})
                .to_string()
                .into(),
        ))
        .unwrap();
        assert!(matches!(
            runtime.events.recv_timeout(Duration::from_secs(2)).unwrap(),
            Event::ResetAck { reset: 7, .. }
        ));
        let request =
            json!({"type":"set_enabled","session":session,"sequence":1,"enabled":true}).to_string();
        ws.send(Message::Text(request.clone().into())).unwrap();
        assert!(matches!(
            runtime.events.recv_timeout(Duration::from_secs(2)).unwrap(),
            Event::SetEnabled { enabled: true, .. }
        ));
        ws.send(Message::Text(request.into())).unwrap();
        assert!(matches!(
            runtime.events.recv_timeout(Duration::from_secs(2)).unwrap(),
            Event::Disconnected { .. }
        ));
    }
}
