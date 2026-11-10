//! Nonblocking host requests shared by plugin UI and worker tasks.
use crate::{Message, Result};
use crossbeam_channel::{Receiver, Sender};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

#[derive(Clone)]
pub struct Client {
    next: Arc<AtomicU64>,
    output: Sender<Message>,
    cancelled: Arc<Mutex<BTreeSet<u64>>>,
    replies: Arc<Mutex<BTreeMap<u64, Sender<Result>>>>,
    closed: Arc<AtomicBool>,
    wake: Sender<()>,
}
pub struct Request {
    id: u64,
    client: Client,
    reply: Receiver<Result>,
}
impl Client {
    pub(crate) fn channel() -> (Self, Receiver<Message>, Receiver<()>) {
        let (output, receiver) = crossbeam_channel::bounded(64);
        let (wake, wake_rx) = crossbeam_channel::bounded(1);
        (
            Self {
                next: Arc::new(AtomicU64::new(0)),
                output,
                replies: Arc::default(),
                cancelled: Arc::default(),
                closed: Arc::default(),
                wake,
            },
            receiver,
            wake_rx,
        )
    }
    pub(crate) fn cancelled(&self) -> Vec<u64> {
        std::mem::take(&mut *self.cancelled.lock().unwrap())
            .into_iter()
            .collect()
    }
    pub(crate) fn next_id(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed) + 1
    }
    pub fn request(&self, name: &str, arguments: Value) -> Request {
        let id = self.next_id();
        let (sender, reply) = crossbeam_channel::bounded(1);
        let mut pending = self.replies.lock().unwrap();
        if self.closed.load(Ordering::Acquire) || pending.len() >= 128 {
            let _ = sender.send(Err(if self.closed.load(Ordering::Acquire) {
                "Host disconnected"
            } else {
                "Too many pending host requests"
            }
            .into()));
            return Request {
                id,
                client: self.clone(),
                reply,
            };
        }
        pending.insert(id, sender);
        drop(pending);
        if let Err(error) = self.output.try_send(Message::Call {
            id,
            name: name.into(),
            arguments,
        }) {
            self.deliver(
                id,
                Err(format!("Host request queue is unavailable: {error}")),
            );
        }
        Request {
            id,
            client: self.clone(),
            reply,
        }
    }
    pub fn event(&self, name: &str, value: Value) -> std::result::Result<(), String> {
        if self.closed.load(Ordering::Acquire) {
            return Err("Host disconnected".into());
        }
        self.output
            .try_send(Message::Event {
                name: name.into(),
                value,
            })
            .map_err(|e| e.to_string())
    }
    pub(crate) fn deliver(&self, id: u64, result: Result) -> bool {
        if let Some(sender) = self.replies.lock().unwrap().remove(&id) {
            let _ = sender.send(result);
            true
        } else {
            false
        }
    }
    pub(crate) fn disconnect(&self) {
        let mut replies = self.replies.lock().unwrap();
        self.closed.store(true, Ordering::Release);
        let _ = self.wake.try_send(());
        for (_, reply) in std::mem::take(&mut *replies) {
            let _ = reply.send(Err("Host disconnected".into()));
        }
    }
    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
    pub(crate) fn pending(&self, id: u64) -> bool {
        self.replies.lock().unwrap().contains_key(&id)
    }
}
impl Request {
    pub fn poll(&self) -> Option<Result> {
        match self.reply.try_recv() {
            Ok(result) => Some(result),
            Err(crossbeam_channel::TryRecvError::Empty) => None,
            Err(_) => Some(Err("Host disconnected".into())),
        }
    }
    /// Only for plugin workers; the UI/event thread must use `poll`.
    pub fn wait(&self) -> Result {
        self.reply
            .recv()
            .map_err(|_| "Host disconnected".to_owned())?
    }
}
impl Drop for Request {
    fn drop(&mut self) {
        if self
            .client
            .replies
            .lock()
            .unwrap()
            .remove(&self.id)
            .is_some()
        {
            self.client.cancelled.lock().unwrap().insert(self.id);
            let _ = self.client.wake.try_send(());
        }
    }
}
