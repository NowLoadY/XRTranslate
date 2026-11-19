//! Bounded IPC mailboxes. State snapshots replace stale snapshots; commands stay ordered.
use crate::Message;
use std::{
    collections::VecDeque,
    sync::{
        Arc, Condvar, Mutex,
        mpsc::{RecvTimeoutError, TrySendError},
    },
    time::Duration,
};

const CAPACITY: usize = 64;

#[derive(Default)]
struct State {
    messages: VecDeque<Message>,
    closed: bool,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    ready: Condvar,
}

pub(crate) struct Sender(Arc<Shared>);
pub(crate) struct Receiver(Arc<Shared>);

pub(crate) fn channel() -> (Sender, Receiver) {
    let shared = Arc::new(Shared::default());
    (Sender(shared.clone()), Receiver(shared))
}

fn snapshot(message: &Message) -> Option<&str> {
    match message {
        Message::Event { name, .. }
            if matches!(
                name.as_str(),
                "companion.presentation" | "voice.state" | "ui.visibility"
            ) =>
        {
            Some(name)
        }
        _ => None,
    }
}

// Position is a snapshot per player. A playback error is a one-shot notification.
fn playback_position(message: &Message) -> Option<u64> {
    if let Message::Event { name, value } = message
        && name == "playback.state"
        && value["snapshot"]["error"].is_null()
    {
        return value["id"].as_u64();
    }
    None
}

impl Sender {
    pub(crate) fn try_send(&self, message: Message) -> Result<(), TrySendError<Message>> {
        let mut state = self.0.state.lock().unwrap();
        if state.closed {
            return Err(TrySendError::Disconnected(message));
        }
        let stale = if let Some(topic) = snapshot(&message) {
            state
                .messages
                .iter()
                .position(|m| snapshot(m) == Some(topic))
        } else {
            playback_position(&message).and_then(|id| {
                state
                    .messages
                    .iter()
                    .position(|m| playback_position(m) == Some(id))
            })
        };
        if let Some(index) = stale {
            state.messages.remove(index);
        }
        // Reserve room for the finite snapshot topics even when commands fill their budget.
        if snapshot(&message).is_none()
            && state
                .messages
                .iter()
                .filter(|pending| snapshot(pending).is_none())
                .count()
                >= CAPACITY
        {
            return Err(TrySendError::Full(message));
        }
        state.messages.push_back(message);
        self.0.ready.notify_one();
        Ok(())
    }
}

impl Receiver {
    pub(crate) fn recv(&self) -> Result<Message, std::sync::mpsc::RecvError> {
        let state = self.0.state.lock().unwrap();
        let mut state = self
            .0
            .ready
            .wait_while(state, |state| state.messages.is_empty() && !state.closed)
            .unwrap();
        state.messages.pop_front().ok_or(std::sync::mpsc::RecvError)
    }

    pub(crate) fn recv_timeout(&self, timeout: Duration) -> Result<Message, RecvTimeoutError> {
        let state = self.0.state.lock().unwrap();
        let (mut state, _) = self
            .0
            .ready
            .wait_timeout_while(state, timeout, |state| {
                state.messages.is_empty() && !state.closed
            })
            .unwrap();
        state.messages.pop_front().ok_or(if state.closed {
            RecvTimeoutError::Disconnected
        } else {
            RecvTimeoutError::Timeout
        })
    }
}

impl Shared {
    fn close(&self) {
        self.state.lock().unwrap().closed = true;
        self.ready.notify_all();
    }
}
impl Drop for Sender {
    fn drop(&mut self) {
        self.0.close();
    }
}
impl Drop for Receiver {
    fn drop(&mut self) {
        self.0.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn playback_updates_coalesce_per_player_without_swallowing_errors_or_calls() {
        let (tx, rx) = channel();
        let position = |id, ms, error: Option<&str>| Message::Event {
            name: "playback.state".into(),
            value: json!({"id":id,"snapshot":{"time_ms":ms,"error":error}}),
        };
        tx.try_send(position(1, 1, Some("decoder failed"))).unwrap();
        tx.try_send(Message::Cancel { id: 7 }).unwrap();
        for ms in 0..1000 {
            for id in [1, 2] {
                tx.try_send(position(id, ms, None)).unwrap();
            }
        }
        assert!(
            matches!(rx.recv().unwrap(), Message::Event { value, .. } if value["snapshot"]["error"] == "decoder failed")
        );
        assert!(matches!(rx.recv().unwrap(), Message::Cancel { id: 7 }));
        for id in [1, 2] {
            assert!(
                matches!(rx.recv().unwrap(), Message::Event { value, .. } if value["id"] == id && value["snapshot"]["time_ms"] == 999)
            );
        }
        assert!(rx.recv_timeout(Duration::ZERO).is_err());
    }

    #[test]
    fn stalled_reader_keeps_latest_state_and_every_command_in_order() {
        let (tx, rx) = channel();
        for index in 0..10_000 {
            tx.try_send(Message::Event {
                name: "companion.presentation".into(),
                value: json!(index),
            })
            .unwrap();
            if index % 1000 == 0 {
                tx.try_send(Message::Call {
                    id: index,
                    name: "action".into(),
                    arguments: json!({}),
                })
                .unwrap();
            }
        }
        drop(tx);
        for index in (0..10_000).step_by(1000) {
            assert!(
                matches!(rx.recv_timeout(Duration::ZERO).unwrap(), Message::Call { id, .. } if id == index)
            );
        }
        assert!(
            matches!(rx.recv_timeout(Duration::ZERO).unwrap(), Message::Event { value, .. } if value == json!(9999))
        );
        assert!(matches!(
            rx.recv_timeout(Duration::ZERO),
            Err(RecvTimeoutError::Disconnected)
        ));
    }

    #[test]
    fn full_mailbox_recovers_without_losing_accepted_messages() {
        let (tx, rx) = channel();
        for id in 0..CAPACITY as u64 {
            tx.try_send(Message::Cancel { id }).unwrap();
        }
        assert!(matches!(
            tx.try_send(Message::Cancel { id: 100 }),
            Err(TrySendError::Full(_))
        ));
        assert!(matches!(
            rx.recv_timeout(Duration::ZERO).unwrap(),
            Message::Cancel { id: 0 }
        ));
        tx.try_send(Message::Cancel { id: 100 }).unwrap();
        for id in 1..CAPACITY as u64 {
            assert!(
                matches!(rx.recv_timeout(Duration::ZERO).unwrap(), Message::Cancel { id: found } if found == id)
            );
        }
        assert!(matches!(
            rx.recv_timeout(Duration::ZERO).unwrap(),
            Message::Cancel { id: 100 }
        ));
        drop(rx);
        assert!(matches!(
            tx.try_send(Message::Cancel { id: 101 }),
            Err(TrySendError::Disconnected(_))
        ));
    }
}
