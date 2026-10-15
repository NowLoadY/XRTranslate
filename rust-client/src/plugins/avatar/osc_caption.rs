//! Send plugin-encoded caption frames through the existing OSC transport owner.
use crate::plugins::osc::transport::{AvatarSnapshot, OscTransportHandle, OscValue, WriteLease};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};
use xrt_connector_core::osc::CaptionContract;
pub(super) use xrt_connector_core::osc::CaptionLayout as Layout;

pub(super) fn discover(
    contract: &'static CaptionContract,
    snapshot: &AvatarSnapshot,
) -> Option<Layout> {
    Layout::discover(contract, |path, access| {
        let node = snapshot.nodes.get(path)?;
        if node.osc_type != "i" || node.access & access != access {
            return None;
        }
        match node.value {
            Some(OscValue::Int(value)) => Some(value),
            _ => None,
        }
    })
}

pub(super) struct Board {
    transport: OscTransportHandle,
    pub epoch: u64,
    pub layout: Layout,
    lease: WriteLease,
    pending: VecDeque<Vec<(String, OscValue)>>,
    last_text: String,
    observed_at: Instant,
    lifetime: Duration,
}

impl Board {
    pub fn new(transport: OscTransportHandle, epoch: u64, layout: Layout) -> Self {
        Self {
            transport,
            epoch,
            layout,
            lease: WriteLease::new(layout.interval()),
            pending: VecDeque::new(),
            last_text: String::new(),
            observed_at: Instant::now(),
            lifetime: Duration::ZERO,
        }
    }

    pub fn replace(&mut self, text: &str, lifetime: Duration) {
        if text.is_empty() || lifetime.is_zero() {
            self.clear();
            return;
        }
        self.observed_at = Instant::now();
        self.lifetime = lifetime;
        // Refresh repeated content occasionally so receivers can retain their
        // idle-disconnect cleanup without retransmitting every identical update.
        if self.last_text == text
            && !renewal_due(!self.pending.is_empty(), self.lease.last_sent(), lifetime)
        {
            return;
        }
        self.lease.revoke();
        self.lease = WriteLease::new(self.layout.interval());
        self.pending = self
            .layout
            .frames(text)
            .into_iter()
            .map(|frame| {
                frame
                    .into_iter()
                    .map(|(path, value)| (path, OscValue::Int(value)))
                    .collect()
            })
            .collect();
        self.last_text = text.to_owned();
    }

    pub fn clear(&mut self) {
        self.lease.revoke();
        self.pending.clear();
        self.last_text.clear();
        let (path, value) = self.layout.clear_write();
        let _ = self
            .transport
            .write_parameter(self.epoch, &path, OscValue::Int(value));
    }

    pub fn poll(&mut self) -> Result<(), String> {
        if let Some(values) = self.pending.front() {
            if self
                .transport
                .write_batch(self.epoch, values.clone(), &self.lease)?
            {
                self.pending.pop_front();
            }
        } else if !self.last_text.is_empty()
            && self
                .lease
                .last_sent()
                .is_some_and(|sent| sent.max(self.observed_at).elapsed() >= self.lifetime)
        {
            self.clear();
        }
        Ok(())
    }
}

fn renewal_due(pending: bool, sent: Option<Instant>, lifetime: Duration) -> bool {
    !pending && sent.is_some_and(|sent| sent.elapsed() >= lifetime.min(Duration::from_secs(15)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_text_renews_idle_receivers_after_actual_delivery() {
        let sent = Instant::now() - Duration::from_secs(16);
        let lifetime = Duration::from_secs(30);
        assert!(!renewal_due(true, Some(sent), lifetime));
        assert!(!renewal_due(false, None, lifetime)); // Final chunk still in flight.
        assert!(!renewal_due(false, Some(Instant::now()), lifetime));
        assert!(renewal_due(false, Some(sent), lifetime)); // Receiver heartbeat bound.
        assert!(renewal_due(
            false,
            Some(Instant::now() - Duration::from_secs(3)),
            Duration::from_secs(2)
        ));
    }
}
