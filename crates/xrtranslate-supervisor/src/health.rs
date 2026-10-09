use std::time::Duration;

use crate::LlamaServerEndpoint;

/// An HTTP readiness request for a local llama-server instance.
///
/// llama.cpp exposes `GET /health`; the concrete HTTP client belongs in the
/// backend crate so this crate stays runtime-agnostic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HealthCheckRequest {
    pub endpoint: LlamaServerEndpoint,
    pub path: String,
    pub timeout: Duration,
}

impl HealthCheckRequest {
    #[must_use]
    pub fn llama_server(endpoint: LlamaServerEndpoint, timeout: Duration) -> Self {
        Self {
            endpoint,
            path: "/health".to_owned(),
            timeout,
        }
    }

    #[must_use]
    pub fn url(&self) -> String {
        self.endpoint.url(&self.path)
    }
}

/// The health status returned by a backend-specific health checker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HealthCheckStatus {
    /// The HTTP request reached the server and it reported ready.
    Ready,
    /// The process is reachable but has not completed model initialization.
    Starting,
    /// The process did not respond before the configured timeout.
    TimedOut,
    /// The checker received a response that is not considered ready.
    Unhealthy {
        status_code: u16,
        detail: Option<String>,
    },
}

/// A transport abstraction for readiness checks.
///
/// Implement it with `reqwest`, `hyper`, or a test fake in the backend.  The
/// trait accepts an explicit request so retries, diagnostics, and test timeouts
/// remain under the backend's control.
pub trait LlamaHealthChecker {
    type Error;

    fn check(&self, request: &HealthCheckRequest) -> Result<HealthCheckStatus, Self::Error>;
}
