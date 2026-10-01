use std::time::Duration;

use reqwest::Method;
use serde_json::Value;

use crate::TransportError;

/// A JSON HTTP request made by an OpenAI-compatible adapter.
#[derive(Debug, Clone, PartialEq)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Value,
}

impl HttpRequest {
    pub fn post_json(url: impl Into<String>, body: Value) -> Self {
        Self {
            method: "POST".into(),
            url: url.into(),
            headers: vec![("content-type".into(), "application/json".into())],
            body,
        }
    }
}

/// The owned response returned by an [`AsyncHttpClient`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

/// Minimal async transport seam for production HTTP and deterministic tests.
///
/// Test implementations normally record [`HttpRequest`] and return a fixture;
/// they never need a socket or a model process.
pub trait AsyncHttpClient: Send + Sync {
    fn execute(
        &self,
        request: HttpRequest,
    ) -> impl Future<Output = Result<HttpResponse, TransportError>> + Send;
}

/// `reqwest` implementation used by the native backend.
#[derive(Debug, Clone)]
pub struct ReqwestClient {
    client: reqwest::Client,
}

impl ReqwestClient {
    /// Builds a client with the request timeout used by the legacy backend.
    pub fn new(timeout: Duration) -> Result<Self, TransportError> {
        client_builder()
            .timeout(timeout)
            .pool_idle_timeout(Duration::from_secs(3))
            .tcp_keepalive(Duration::from_secs(5))
            .tcp_nodelay(true)
            .build()
            .map(|client| Self { client })
            .map_err(|error| TransportError::new("client", error.to_string()))
    }

    /// Builds a client that bypasses environment-configured proxies.
    ///
    /// Local llama.cpp endpoints must never be routed through an enterprise
    /// HTTP proxy: proxies commonly reject loopback URLs with a 502, and any
    /// detour would add latency to the real-time audio path. Remote providers
    /// should continue to use [`Self::new`] so their proxy configuration is
    /// respected.
    pub fn new_direct(timeout: Duration) -> Result<Self, TransportError> {
        client_builder()
            .no_proxy()
            .timeout(timeout)
            .pool_idle_timeout(Duration::from_secs(3))
            .tcp_keepalive(Duration::from_secs(5))
            .tcp_nodelay(true)
            .build()
            .map(|client| Self { client })
            .map_err(|error| TransportError::new("client", error.to_string()))
    }

    /// The standard 30-second client for local `llama-server` requests.
    pub fn with_default_timeout() -> Result<Self, TransportError> {
        Self::new(Duration::from_secs(30))
    }

    /// The standard proxy-bypassing client for local `llama-server` requests.
    pub fn with_default_direct_timeout() -> Result<Self, TransportError> {
        Self::new_direct(Duration::from_secs(30))
    }
}

impl Default for ReqwestClient {
    fn default() -> Self {
        Self::with_default_timeout().expect("a default reqwest client must be constructible")
    }
}

impl AsyncHttpClient for ReqwestClient {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        let method = Method::from_bytes(request.method.as_bytes())
            .map_err(|error| TransportError::new("method", error.to_string()))?;

        const MAX_ATTEMPTS: usize = 3;
        let mut last_error = None;

        for attempt in 0..MAX_ATTEMPTS {
            let mut builder = self.client.request(method.clone(), &request.url);
            if !request.body.is_null() {
                builder = builder.json(&request.body);
            }
            for (name, value) in &request.headers {
                builder = builder.header(name, value);
            }

            match builder.send().await {
                Ok(response) => {
                    let status = response.status().as_u16();
                    let body = response
                        .text()
                        .await
                        .map_err(|error| TransportError::new("response_body", error.to_string()))?;
                    return Ok(HttpResponse { status, body });
                }
                Err(error) => {
                    let kind = request_error_kind(&error);
                    let should_retry =
                        attempt + 1 < MAX_ATTEMPTS && (error.is_connect() || error.is_request());
                    last_error = Some(TransportError::new(kind, error.to_string()));
                    if !should_retry {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(25 * (1 << attempt))).await;
                }
            }
        }

        Err(last_error.unwrap_or_else(|| TransportError::new("transport", "request failed")))
    }
}

fn request_error_kind(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connect"
    } else if error.is_request() {
        "request"
    } else {
        "http"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn reqwest_retries_on_connection_reset_or_drop() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{port}/test");

        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = socket.read(&mut buf).await;
                drop(socket);
            }
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = socket.read(&mut buf).await;
                let response = "HTTP/1.1 200 OK\r\nContent-Length: 15\r\nContent-Type: application/json\r\n\r\n{\"status\":\"ok\"}";
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });

        let client = ReqwestClient::new_direct(Duration::from_secs(5)).unwrap();
        let response = client
            .execute(HttpRequest::post_json(url, serde_json::json!({"test": 1})))
            .await
            .unwrap();

        assert_eq!(response.status, 200);
        assert_eq!(response.body, "{\"status\":\"ok\"}");
    }
}

fn client_builder() -> reqwest::ClientBuilder {
    let builder = reqwest::Client::builder();
    #[cfg(target_os = "android")]
    {
        // The managed service is a native process and has no Java VM.
        let roots = rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        let config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        builder.use_preconfigured_tls(config)
    }
    #[cfg(not(target_os = "android"))]
    builder
}
