use std::time::Duration;

use reqwest::Method;
use serde_json::Value;

use crate::TransportError;

/// An HTTP request made by an inference adapter.
#[derive(Debug, Clone, PartialEq)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Value,
    pub multipart: Option<MultipartBody>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MultipartBody {
    pub fields: Vec<(String, String)>,
    pub file_name: String,
    pub file: Vec<u8>,
    pub mime_type: String,
}

impl HttpRequest {
    pub fn post_json(url: impl Into<String>, body: Value) -> Self {
        Self {
            method: "POST".into(),
            url: url.into(),
            headers: vec![("content-type".into(), "application/json".into())],
            body,
            multipart: None,
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

    /// Reads a successful response incrementally. The awaited consumer provides
    /// backpressure; dropping this future cancels the request without a worker.
    fn execute_streaming<F, Fut>(
        &self,
        request: HttpRequest,
        mut on_chunk: F,
    ) -> impl Future<Output = Result<HttpResponse, TransportError>> + Send
    where
        F: FnMut(Vec<u8>) -> Fut + Send,
        Fut: Future<Output = Result<(), TransportError>> + Send,
    {
        async move {
            let mut response = self.execute(request).await?;
            if (200..300).contains(&response.status) {
                on_chunk(std::mem::take(&mut response.body).into_bytes()).await?;
            }
            Ok(response)
        }
    }
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

impl ReqwestClient {
    async fn send(&self, request: &HttpRequest) -> Result<reqwest::Response, TransportError> {
        let method = Method::from_bytes(request.method.as_bytes())
            .map_err(|error| TransportError::new("method", error.to_string()))?;

        const MAX_ATTEMPTS: usize = 3;
        let mut last_error = None;

        for attempt in 0..MAX_ATTEMPTS {
            let mut builder = self.client.request(method.clone(), &request.url);
            if let Some(body) = &request.multipart {
                let file = reqwest::multipart::Part::bytes(body.file.clone())
                    .file_name(body.file_name.clone())
                    .mime_str(&body.mime_type)
                    .map_err(|error| TransportError::new("multipart", error.to_string()))?;
                let mut form = reqwest::multipart::Form::new().part("file", file);
                for (name, value) in &body.fields {
                    form = form.text(name.clone(), value.clone());
                }
                builder = builder.multipart(form);
            } else if !request.body.is_null() {
                builder = builder.json(&request.body);
            }
            for (name, value) in &request.headers {
                builder = builder.header(name, value);
            }

            match builder.send().await {
                Ok(response) => return Ok(response),
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

impl AsyncHttpClient for ReqwestClient {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        let response = self.send(&request).await?;
        let status = response.status().as_u16();
        let body = response
            .text()
            .await
            .map_err(|error| TransportError::new("response_body", error.to_string()))?;
        Ok(HttpResponse { status, body })
    }

    async fn execute_streaming<F, Fut>(
        &self,
        request: HttpRequest,
        mut on_chunk: F,
    ) -> Result<HttpResponse, TransportError>
    where
        F: FnMut(Vec<u8>) -> Fut + Send,
        Fut: Future<Output = Result<(), TransportError>> + Send,
    {
        let mut response = self.send(&request).await?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            let body = response
                .text()
                .await
                .map_err(|error| TransportError::new("response_body", error.to_string()))?;
            return Ok(HttpResponse { status, body });
        }
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| TransportError::new("response_body", error.to_string()))?
        {
            on_chunk(chunk.to_vec()).await?;
        }
        Ok(HttpResponse {
            status,
            body: String::new(),
        })
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

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!(
            "http://{}/v1/chat/completions",
            listener.local_addr().unwrap()
        );
        let (first_seen, wait_for_first) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            assert!(socket.read(&mut request).await.unwrap() > 0);
            let first = "data: {\"choices\":[{\"delta\":{\"content\":\"你\"}}]}\n\n";
            let final_chunk = "data: {\"choices\":[{\"delta\":{\"content\":\"好。\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
            socket.write_all(format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{first}",
                first.len() + final_chunk.len(),
            ).as_bytes()).await.unwrap();
            // The server cannot finish until the caller sees its first token.
            wait_for_first.await.unwrap();
            socket.write_all(final_chunk.as_bytes()).await.unwrap();
        });
        let chat = crate::OpenAiCompatibleClient::new(client, endpoint).unwrap();
        let mut first_seen = Some(first_seen);
        let result = chat
            .chat_completion_streaming(serde_json::json!({}), false, |text| {
                if text == "你" {
                    first_seen.take().unwrap().send(()).unwrap();
                }
                std::future::ready(())
            })
            .await
            .unwrap();
        assert_eq!(result.text, "你好。");
        server.await.unwrap();
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
