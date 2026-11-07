use std::{
    error::Error,
    ops::ControlFlow,
    time::{Duration, Instant},
};

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
    pub retry_after: Option<Duration>,
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
    /// backpressure; `Break` stops at a protocol completion event without waiting
    /// for HTTP EOF. Dropping this future cancels the request without a worker.
    fn execute_streaming<F, Fut>(
        &self,
        request: HttpRequest,
        mut on_chunk: F,
    ) -> impl Future<Output = Result<HttpResponse, TransportError>> + Send
    where
        F: FnMut(Vec<u8>) -> Fut + Send,
        Fut: Future<Output = Result<ControlFlow<()>, TransportError>> + Send,
    {
        async move {
            let mut response = self.execute(request).await?;
            if (200..300).contains(&response.status) {
                let _ = on_chunk(std::mem::take(&mut response.body).into_bytes()).await?;
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
    /// Account credentials may only be sent to the connector's fixed official endpoint.
    pub(crate) fn for_account() -> Result<Self, TransportError> {
        client_builder()
            .timeout(Duration::from_secs(120))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("XRTranslate/", env!("CARGO_PKG_VERSION")))
            .build()
            .map(|client| Self { client })
            .map_err(|_| TransportError::new("client", "Cannot create account service connection"))
    }
    /// Builds a client with the request timeout used by the legacy backend.
    pub fn new(timeout: Duration) -> Result<Self, TransportError> {
        client_builder()
            .timeout(timeout)
            .pool_idle_timeout(Duration::from_secs(90))
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

    /// The standard 30-second client for online services.
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
            // json() supplies Content-Type only when absent; applying headers
            // first avoids appending a second value that can hide the JSON body.
            for (name, value) in &request.headers {
                let mut header = reqwest::header::HeaderValue::from_str(value)
                    .map_err(|_| TransportError::new("headers", "Invalid request header"))?;
                if name.eq_ignore_ascii_case("authorization") || name.eq_ignore_ascii_case("cookie")
                {
                    header.set_sensitive(true);
                }
                builder = builder.header(name, header);
            }
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
            match builder.send().await {
                Ok(response) => return Ok(response),
                Err(error) => {
                    let kind = request_error_kind(&error);
                    let should_retry =
                        attempt + 1 < MAX_ATTEMPTS && (error.is_connect() || error.is_request());
                    last_error = Some(transport_error(error));
                    if !should_retry {
                        break;
                    }
                    tracing::warn!(
                        attempt = attempt + 1,
                        kind,
                        "Retrying model HTTP connection"
                    );
                    tokio::time::sleep(Duration::from_millis(25 * (1 << attempt))).await;
                }
            }
        }

        Err(last_error.unwrap_or_else(|| TransportError::new("transport", "request failed")))
    }
}

impl AsyncHttpClient for ReqwestClient {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        let started = Instant::now();
        let response = self.send(&request).await?;
        let headers_ms = started.elapsed().as_millis();
        let status = response.status().as_u16();
        let retry_after = retry_after_header(response.headers());
        let body = response.text().await.map_err(transport_error)?;
        let retry_after =
            retry_after.or_else(|| (status >= 400).then(|| retry_after_body(&body)).flatten());
        log_timing(&request, started, headers_ms, None, status);
        Ok(HttpResponse {
            status,
            body,
            retry_after,
        })
    }

    async fn execute_streaming<F, Fut>(
        &self,
        request: HttpRequest,
        mut on_chunk: F,
    ) -> Result<HttpResponse, TransportError>
    where
        F: FnMut(Vec<u8>) -> Fut + Send,
        Fut: Future<Output = Result<ControlFlow<()>, TransportError>> + Send,
    {
        let started = Instant::now();
        let mut response = self.send(&request).await?;
        let headers_ms = started.elapsed().as_millis();
        let status = response.status().as_u16();
        let retry_after = retry_after_header(response.headers());
        if !(200..300).contains(&status) {
            let body = response.text().await.map_err(transport_error)?;
            let retry_after = retry_after.or_else(|| retry_after_body(&body));
            log_timing(&request, started, headers_ms, None, status);
            return Ok(HttpResponse {
                status,
                body,
                retry_after,
            });
        }
        let mut first_chunk_ms = None;
        while let Some(chunk) = response.chunk().await.map_err(|error| {
            let error = transport_error(error);
            tracing::warn!(
                model = request.body["model"].as_str().unwrap_or(""),
                headers_ms, ?first_chunk_ms, status,
                elapsed_ms = started.elapsed().as_millis(),
                error = %error,
                "Model HTTP stream interrupted"
            );
            error
        })? {
            first_chunk_ms.get_or_insert_with(|| started.elapsed().as_millis());
            if on_chunk(chunk.to_vec()).await?.is_break() {
                // Drain a short HTTP/1 tail without delaying the result or losing
                // keep-alive. HTTP/2 can close just this stream and reuse the connection.
                if matches!(
                    response.version(),
                    reqwest::Version::HTTP_10 | reqwest::Version::HTTP_11
                ) {
                    tokio::spawn(async move {
                        let _ = tokio::time::timeout(Duration::from_secs(1), async {
                            while let Ok(Some(_)) = response.chunk().await {}
                        })
                        .await;
                    });
                }
                break;
            }
        }
        log_timing(&request, started, headers_ms, first_chunk_ms, status);
        Ok(HttpResponse {
            status,
            body: String::new(),
            retry_after,
        })
    }
}

pub(crate) fn retry_after_header(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let value = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    value
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
        .or_else(|| {
            Some(
                httpdate::parse_http_date(value)
                    .ok()?
                    .duration_since(std::time::SystemTime::now())
                    .unwrap_or_default(),
            )
        })
}

fn retry_after_body(body: &str) -> Option<Duration> {
    let body: Value = serde_json::from_str(body).ok()?;
    let error = body.get("error").unwrap_or(&body);
    // Google APIs can return RetryInfo instead of the standard HTTP header.
    let delay = error["details"]
        .as_array()
        .and_then(|details| {
            details
                .iter()
                .find_map(|detail| detail["retryDelay"].as_str())
        })
        .or_else(|| {
            // The Interactions API currently gives the delay in its error message.
            error["message"]
                .as_str()?
                .split_once("retry in ")?
                .1
                .split_whitespace()
                .next()
        })?;
    let seconds = delay
        .trim_end_matches('.')
        .strip_suffix('s')?
        .parse()
        .ok()?;
    Duration::try_from_secs_f64(seconds).ok()
}

#[cfg(test)]
mod retry_tests {
    use super::*;

    #[test]
    fn preserves_provider_cooldown() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "28".parse().unwrap());
        assert_eq!(retry_after_header(&headers), Some(Duration::from_secs(28)));
        let until = std::time::SystemTime::now() + Duration::from_secs(60);
        headers.insert(
            reqwest::header::RETRY_AFTER,
            httpdate::fmt_http_date(until).parse().unwrap(),
        );
        assert!((59..=60).contains(&retry_after_header(&headers).unwrap().as_secs()));
        for (body, seconds) in [
            (
                r#"{"error":{"message":"Rate limit exceeded. Please retry in 28s or upgrade your tier.","code":"too_many_requests"}}"#,
                28.0,
            ),
            (
                r#"{"error":{"details":[{"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"2.5s"}]}}"#,
                2.5,
            ),
        ] {
            assert_eq!(
                retry_after_body(body),
                Some(Duration::from_secs_f64(seconds))
            );
        }
        for body in [
            "not json",
            r#"{"error":{"message":"Invalid credentials"}}"#,
            r#"{"error":{"details":[{"retryDelay":"-1s"}]}}"#,
        ] {
            assert_eq!(retry_after_body(body), None);
        }
    }
}

fn log_timing(
    request: &HttpRequest,
    started: Instant,
    headers_ms: u128,
    first_chunk_ms: Option<u128>,
    status: u16,
) {
    tracing::info!(
        model = request.body["model"].as_str().unwrap_or(""),
        headers_ms,
        ?first_chunk_ms,
        elapsed_ms = started.elapsed().as_millis(),
        status,
        "Model HTTP request completed"
    );
}

fn request_error_kind(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connect"
    } else if error.is_request() {
        "request"
    } else if error.is_body() || error.is_decode() {
        "response_body"
    } else {
        "http"
    }
}

fn transport_error(error: reqwest::Error) -> TransportError {
    let kind = request_error_kind(&error);
    let error = error.without_url();
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(&format!(": {cause}"));
        source = cause.source();
    }
    TransportError::new(kind, message)
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

#[cfg(test)]
mod tests {
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
        sync::{Arc, Mutex},
        thread,
    };

    use super::*;
    use crate::{TranslationAdapter, TranslationOptions, TranslationPromptProfile};

    #[tokio::test]
    async fn stalled_response_body_preserves_timeout_and_its_cause() {
        for streaming in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = format!("http://{}", listener.local_addr().unwrap());
            let (release, wait) = std::sync::mpsc::channel::<()>();
            let server = thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut reader = BufReader::new(socket.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    assert_ne!(reader.read_line(&mut line).unwrap(), 0);
                    if line == "\r\n" {
                        break;
                    }
                }
                socket
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{")
                    .unwrap();
                let _ = wait.recv_timeout(Duration::from_secs(2));
            });
            let client = ReqwestClient::new_direct(Duration::from_millis(100)).unwrap();
            let request = HttpRequest::post_json(&endpoint, Value::Null);
            let result = if streaming {
                client
                    .execute_streaming(request, |_| {
                        std::future::ready(Ok(ControlFlow::Continue(())))
                    })
                    .await
            } else {
                client.execute(request).await
            };
            let _ = release.send(());
            server.join().unwrap();
            let source = result.unwrap_err();
            assert_eq!(source.kind, "timeout");
            assert!(source.message.contains("timed out"), "{source}");
            assert!(crate::InferenceError::Transport { endpoint, source }.is_retryable());
        }
    }

    #[tokio::test]
    async fn translation_sends_one_json_content_type_and_selected_model() {
        for streaming in [false, true] {
            let trace = Arc::new(Mutex::new(None));
            let server_trace = trace.clone();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = format!(
                "http://{}/v1/chat/completions",
                listener.local_addr().unwrap()
            );
            let server = thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(socket.try_clone().unwrap());
                let mut headers = Vec::new();
                loop {
                    let mut line = String::new();
                    assert_ne!(reader.read_line(&mut line).unwrap(), 0);
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        headers.push((name.to_ascii_lowercase(), value.trim().to_owned()));
                    }
                }
                let length = headers
                    .iter()
                    .find(|(name, _)| name == "content-length")
                    .unwrap()
                    .1
                    .parse::<usize>()
                    .unwrap();
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                if streaming {
                    assert!(server_trace.lock().unwrap().is_some());
                }
                let (content_type, response) = if streaming {
                    (
                        "text/event-stream",
                        concat!(
                            "data: {\"choices\":[{\"delta\":{\"content\":\"Hello.\"},\"finish_reason\":null}]}\n\n",
                            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                            "data: [DONE]\n\n",
                        ),
                    )
                } else {
                    (
                        "application/json",
                        r#"{"choices":[{"message":{"content":"Hello."},"finish_reason":"stop"}]}"#,
                    )
                };
                write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).unwrap();
                (headers, serde_json::from_slice::<Value>(&body).unwrap())
            });
            let adapter = TranslationAdapter::with_bearer_token(
                ReqwestClient::new_direct(Duration::from_secs(5)).unwrap(),
                endpoint,
                "gpt-4o-mini",
                TranslationPromptProfile::ChatMessages,
                "test-token",
            )
            .unwrap();
            let options = TranslationOptions::new("zh", "en");
            let result = if streaming {
                adapter
                    .translate_streaming(
                        "你好。",
                        options,
                        |_| std::future::ready(()),
                        |current| {
                            assert!(trace.lock().unwrap().replace(current.clone()).is_none());
                        },
                    )
                    .await
            } else {
                adapter.translate("你好。", options).await
            }
            .unwrap();
            let (headers, body) = server.join().unwrap();
            assert_eq!(result.text, "Hello.");
            if streaming {
                assert_eq!(trace.lock().unwrap().as_ref(), Some(&result.prompt_trace));
            }
            let content_types: Vec<_> = headers
                .iter()
                .filter(|(name, _)| name == "content-type")
                .map(|(_, value)| value.as_str())
                .collect();
            assert_eq!(content_types, ["application/json"]);
            assert!(
                headers
                    .iter()
                    .any(|(name, value)| name == "authorization" && value == "Bearer test-token")
            );
            assert_eq!(body["model"], "gpt-4o-mini");
            assert_eq!(body["stream"], streaming);
        }
    }
}
