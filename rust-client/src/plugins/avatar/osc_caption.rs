//! Send plugin-encoded caption frames through the existing OSC transport owner.
use crate::plugins::osc::transport::{AvatarSnapshot, OscTransportHandle, OscValue, WriteLease};
use std::time::{Duration, Instant};
pub(super) use xrt_connector_core::osc::CaptionLayout as Layout;
use xrt_connector_core::{
    DisplaySnapshot,
    osc::{CaptionBuffer, CaptionContract, CaptionFrame},
};

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
    buffer: CaptionBuffer,
    lease: WriteLease,
    inflight: Option<CaptionFrame>,
    last_sent: Option<Instant>,
    ready_at: Instant,
}

impl Board {
    pub fn new(transport: OscTransportHandle, epoch: u64, layout: Layout) -> Self {
        Self {
            transport,
            epoch,
            layout,
            buffer: CaptionBuffer::new(layout),
            lease: WriteLease::new(layout.interval()),
            inflight: None,
            last_sent: None,
            ready_at: Instant::now(),
        }
    }

    pub fn replace(&mut self, snapshot: &DisplaySnapshot) {
        self.confirm_delivery();
        let was_empty = self.buffer.is_empty();
        self.buffer.replace(snapshot);
        if !was_empty && self.buffer.is_empty() {
            self.clear();
            return;
        }
        if self
            .inflight
            .as_ref()
            .is_some_and(|frame| !self.buffer.is_current(frame))
        {
            self.lease.revoke();
            self.buffer.discard(&self.inflight.take().unwrap());
            self.lease = WriteLease::new(self.layout.interval());
            // A send racing cancellation may already be on the wire. Preserve
            // its hold interval and rewrite that unknown chunk from fresh state.
            self.ready_at = Instant::now() + self.layout.interval();
        }
    }

    pub fn clear(&mut self) {
        self.lease.revoke();
        self.lease = WriteLease::new(self.layout.interval());
        self.inflight = None;
        self.buffer.clear();
        self.ready_at = Instant::now();
        // Submit now: the connection may be dropped immediately after clearing.
        let _ = self.poll();
    }

    fn confirm_delivery(&mut self) {
        if let Some(sent) = self.lease.last_sent() {
            if let Some(frame) = self.inflight.take() {
                self.buffer.confirm(frame);
                self.last_sent = Some(sent);
            }
        }
    }

    pub fn poll(&mut self) -> Result<(), String> {
        self.confirm_delivery();
        if self.inflight.is_some() || Instant::now() < self.ready_at {
            return Ok(());
        }
        let idle = self.last_sent.map_or(Duration::ZERO, |sent| sent.elapsed());
        if let Some(frame) = self.buffer.next_frame(idle) {
            let values = frame
                .values
                .iter()
                .map(|(path, value)| (path.clone(), OscValue::Int(*value)))
                .collect();
            if self
                .transport
                .write_batch(self.epoch, values, &self.lease)?
            {
                self.inflight = Some(frame);
            }
        }
        Ok(())
    }
}
