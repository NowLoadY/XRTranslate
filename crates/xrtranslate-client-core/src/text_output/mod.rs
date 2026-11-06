//! Text composition and sampled presentation data, shared without owning workers.
mod text;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
pub use text::*;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SystemMetrics {
    pub time_str: String,
    pub cpu_name: String,
    pub cpu_usage: u32,
    pub gpu_name: String,
    pub gpu_usage: u32,
}

#[derive(Clone, Default)]
pub struct TextOutputHandle(Arc<parking_lot::Mutex<SystemMetrics>>);
impl TextOutputHandle {
    pub fn snapshot(&self) -> SystemMetrics {
        self.0.lock().clone()
    }
    pub fn update(&self, metrics: SystemMetrics) {
        *self.0.lock() = metrics;
    }
}
