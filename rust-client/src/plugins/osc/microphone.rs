//! VRChat Toggle Voice synchronization. Only explicit requests create a target;
//! observed game changes never cause an automatic request to reopen its microphone.
use std::time::{Duration, Instant};

const PULSE: Duration = Duration::from_millis(50);
const ACK_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Edge {
    Press,
    Release,
}

struct Awaiting {
    muted: bool,
    since: Instant,
}

#[derive(Default)]
pub(super) struct MicrophoneSync {
    enabled: bool,
    muted: Option<bool>,
    target: Option<bool>,
    pressed: Option<Instant>,
    released: Option<Instant>,
    awaiting: Option<Awaiting>,
    external_mute: bool,
    error: Option<&'static str>,
}

impl MicrophoneSync {
    pub fn set_enabled(&mut self, enabled: bool) {
        if self.enabled == enabled {
            return;
        }
        self.enabled = enabled;
        if self.awaiting.is_some() {
            // A submitted toggle may still arrive after disabling. Its old
            // baseline cannot safely justify another toggle when re-enabled.
            self.muted = None;
        }
        self.target = None;
        self.awaiting = None;
        self.external_mute = false;
        self.error = None;
        // Keep a pressed key until the worker sends its release, including when
        // synchronization is disabled between the two edges.
    }

    pub fn request(&mut self, enabled: bool) {
        if self.enabled {
            self.target = Some(!enabled);
            self.error = None;
        }
    }

    pub fn observe(&mut self, muted: bool) {
        let previous = self.muted.replace(muted);
        if self
            .awaiting
            .as_ref()
            .is_some_and(|pending| pending.muted == muted)
        {
            self.awaiting = None;
            if self.target == Some(muted) {
                self.target = None;
            }
            self.error = None;
        } else if self.enabled
            && muted
            && (previous == Some(false) || (previous.is_none() && self.target.is_none()))
        {
            // The user muted in-game. Retire an older opening request rather
            // than treating its target as a state that must be maintained.
            self.target = None;
            self.awaiting = None;
            self.external_mute = true;
        }
    }

    pub fn take_external_mute(&mut self) -> bool {
        std::mem::take(&mut self.external_mute)
    }

    pub fn poll(&mut self, now: Instant) -> Option<Edge> {
        if self
            .pressed
            .is_some_and(|at| !self.enabled || now.duration_since(at) >= PULSE)
        {
            self.pressed = None;
            self.released = Some(now);
            return Some(Edge::Release);
        }
        if !self.enabled || self.pressed.is_some() {
            return None;
        }
        if self
            .awaiting
            .as_ref()
            .is_some_and(|pending| now.duration_since(pending.since) >= ACK_TIMEOUT)
        {
            self.fail("Microphone sync timed out; check VRChat Toggle Voice and OSC");
            return None;
        }
        if self.awaiting.is_some()
            || self
                .released
                .is_some_and(|at| now.duration_since(at) < PULSE)
        {
            return None;
        }
        let (Some(target), Some(muted)) = (self.target, self.muted) else {
            return None;
        };
        if target == muted {
            self.target = None;
            return None;
        }
        self.pressed = Some(now);
        self.awaiting = Some(Awaiting {
            muted: target,
            since: now,
        });
        Some(Edge::Press)
    }

    /// The transport releases any pressed key before retiring its socket/peer.
    pub fn disconnected(&mut self) {
        if self.awaiting.is_some() || self.pressed.is_some() {
            self.target = None;
        }
        self.muted = None;
        self.awaiting = None;
        self.pressed = None;
        self.released = None;
        self.external_mute = false;
    }

    pub fn fail(&mut self, reason: &'static str) {
        self.target = None;
        self.awaiting = None;
        self.muted = None;
        self.error = Some(reason);
    }

    pub fn status(&self) -> Option<&'static str> {
        if !self.enabled {
            None
        } else if let Some(error) = self.error {
            Some(error)
        } else if self.awaiting.is_some() {
            Some("Waiting for VRChat to confirm the microphone change")
        } else if self.muted.is_none() {
            Some("Waiting for VRChat microphone state (MuteSelf)")
        } else {
            None
        }
    }
}
