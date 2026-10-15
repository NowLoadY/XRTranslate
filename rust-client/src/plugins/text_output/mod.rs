//! Shared text-output capability, composed by output plugins through a host-owned handle.
mod system;
mod text;

pub use system::{SystemMetrics, TextOutputHandle};
pub use text::{BannerConfig, BannerContentType, MessageFormat};
pub(crate) use text::{format_message, message_parts, render_banner_selector};

pub(crate) struct TextOutputPlugin {
    monitor: system::SystemMonitor,
}

impl TextOutputPlugin {
    pub(crate) fn new() -> Self {
        Self {
            monitor: system::SystemMonitor::new(),
        }
    }

    pub(crate) fn handle(&self) -> TextOutputHandle {
        self.monitor.handle()
    }

    pub(crate) fn shutdown(&mut self) {
        self.monitor.shutdown();
    }
}
