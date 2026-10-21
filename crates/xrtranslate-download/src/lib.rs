//! Resumable artifact downloads with retries, source routing, and size checks.

#![forbid(unsafe_code)]

mod source;

pub use source::DownloadSource;

use reqwest::{
    StatusCode,
    header::{ACCEPT_ENCODING, CONTENT_RANGE, RANGE, RETRY_AFTER},
};
use std::{
    error::Error,
    fmt, fs, io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::io::AsyncWriteExt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DownloadProgress {
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
}

/// Cancels a download between network chunks or during a retry delay.
#[derive(Clone, Debug, Default)]
pub struct DownloadCancellation(Arc<AtomicBool>);

impl DownloadCancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DownloadSpec<'a> {
    label: &'a str,
    url: &'a str,
    bytes: u64,
}

impl<'a> DownloadSpec<'a> {
    /// Describes an artifact from an HTTPS URL with its expected byte length.
    pub const fn new(label: &'a str, url: &'a str, bytes: u64) -> Self {
        Self { label, url, bytes }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DownloadPolicy {
    pub max_attempts: u32,
    pub connect_timeout: Duration,
    pub read_timeout: Duration,
    pub retry_delay: Duration,
    /// Do not leave a UI-owned worker sleeping indefinitely on a server hint.
    pub max_automatic_retry_delay: Duration,
}

impl Default for DownloadPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 4,
            connect_timeout: Duration::from_secs(15),
            read_timeout: Duration::from_secs(45),
            retry_delay: Duration::from_secs(1),
            max_automatic_retry_delay: Duration::from_secs(60),
        }
    }
}

#[derive(Clone, Debug)]
pub struct DownloadClient {
    client: reqwest::Client,
    policy: DownloadPolicy,
    source: DownloadSource,
    cancellation: Option<DownloadCancellation>,
}

impl DownloadClient {
    pub fn new(user_agent: &str) -> Result<Self, DownloadError> {
        Self::with_policy_and_proxy(user_agent, DownloadPolicy::default(), None)
    }

    pub fn with_policy(user_agent: &str, policy: DownloadPolicy) -> Result<Self, DownloadError> {
        Self::with_policy_and_proxy(user_agent, policy, None)
    }

    pub fn with_proxy(user_agent: &str, proxy_url: Option<&str>) -> Result<Self, DownloadError> {
        Self::with_proxy_and_source(user_agent, proxy_url, DownloadSource::Official)
    }

    pub fn with_proxy_and_source(
        user_agent: &str,
        proxy_url: Option<&str>,
        source: DownloadSource,
    ) -> Result<Self, DownloadError> {
        Self::with_policy_proxy_and_source(user_agent, DownloadPolicy::default(), proxy_url, source)
    }

    pub fn with_proxy_source_and_cancellation(
        user_agent: &str,
        proxy_url: Option<&str>,
        source: DownloadSource,
        cancellation: DownloadCancellation,
    ) -> Result<Self, DownloadError> {
        let mut client = Self::with_proxy_and_source(user_agent, proxy_url, source)?;
        client.cancellation = Some(cancellation);
        Ok(client)
    }

    fn with_policy_and_proxy(
        user_agent: &str,
        policy: DownloadPolicy,
        proxy_url: Option<&str>,
    ) -> Result<Self, DownloadError> {
        Self::with_policy_proxy_and_source(user_agent, policy, proxy_url, DownloadSource::Official)
    }

    fn with_policy_proxy_and_source(
        user_agent: &str,
        policy: DownloadPolicy,
        proxy_url: Option<&str>,
        source: DownloadSource,
    ) -> Result<Self, DownloadError> {
        if policy.max_attempts == 0 {
            return Err(DownloadError::InvalidSpec(
                "download max_attempts must be greater than zero".into(),
            ));
        }
        let mut builder = reqwest::Client::builder()
            .user_agent(user_agent)
            .connect_timeout(policy.connect_timeout)
            .read_timeout(policy.read_timeout);
        if let Some(proxy_url) = proxy_url.filter(|url| !url.trim().is_empty()) {
            builder = builder.proxy(
                reqwest::Proxy::all(proxy_url)
                    .map_err(|error| DownloadError::Client(error.to_string()))?,
            );
        }
        #[cfg(target_os = "android")]
        let builder = {
            let roots = rustls::RootCertStore {
                roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
            };
            let config = rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth();
            builder.use_preconfigured_tls(config)
        };
        let client = builder
            .build()
            .map_err(|error| DownloadError::Client(error.to_string()))?;
        Ok(Self {
            client,
            policy,
            source,
            cancellation: None,
        })
    }

    /// Downloads beside `complete` using a deterministic `.part` sibling.
    pub async fn download_to(
        &self,
        spec: DownloadSpec<'_>,
        complete: &Path,
        on_progress: impl FnMut(DownloadProgress),
    ) -> Result<(), DownloadError> {
        let file_name = complete.file_name().ok_or_else(|| {
            DownloadError::InvalidSpec(format!(
                "download {} destination has no file name",
                spec.label
            ))
        })?;
        let mut partial_name = file_name.to_os_string();
        partial_name.push(".part");
        let partial = complete.with_file_name(partial_name);
        self.download(spec, &partial, complete, on_progress).await
    }

    pub async fn download(
        &self,
        spec: DownloadSpec<'_>,
        partial: &Path,
        complete: &Path,
        mut on_progress: impl FnMut(DownloadProgress),
    ) -> Result<(), DownloadError> {
        validate_spec(spec)?;
        self.ensure_not_cancelled(spec.label)?;
        if complete.is_file() {
            match verify_file(complete, spec) {
                Ok(()) => {
                    on_progress(DownloadProgress {
                        downloaded_bytes: spec.bytes,
                        total_bytes: spec.bytes,
                    });
                    return Ok(());
                }
                Err(DownloadError::Size { .. }) => {
                    fs::remove_file(complete).map_err(|source| DownloadError::FileIo {
                        path: complete.to_path_buf(),
                        source,
                    })?;
                }
                Err(error) => return Err(error),
            }
        }
        if let Some(parent) = partial.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|source| DownloadError::FileIo {
                    path: parent.to_path_buf(),
                    source,
                })?;
        }

        let primary_url = self.source.resolve(spec.url);
        self.transfer_with_retries(spec, partial, primary_url.as_ref(), &mut on_progress)
            .await?;
        self.ensure_not_cancelled(spec.label)?;
        tokio::fs::rename(partial, complete)
            .await
            .map_err(|source| DownloadError::FileIo {
                path: complete.to_path_buf(),
                source,
            })
    }

    async fn transfer_with_retries(
        &self,
        spec: DownloadSpec<'_>,
        partial: &Path,
        primary_url: &str,
        on_progress: &mut impl FnMut(DownloadProgress),
    ) -> Result<(), DownloadError> {
        let mut attempt = 1;
        loop {
            self.ensure_not_cancelled(spec.label)?;
            let result = self
                .transfer_once(spec, primary_url, partial, on_progress)
                .await
                .and_then(|()| verify_file(partial, spec));
            if matches!(&result, Err(DownloadError::Size { .. })) {
                fs::remove_file(partial).map_err(|source| DownloadError::FileIo {
                    path: partial.to_path_buf(),
                    source,
                })?;
            }
            match result {
                Ok(()) => return Ok(()),
                // Source selection belongs to the owner and its visible UI.
                // Report official failures promptly so it can select Mirror
                // through the same path as the user's source checkbox.
                Err(error)
                    if self.source == DownloadSource::Official
                        && DownloadSource::Mirror.resolve(spec.url) != spec.url
                        && error.allows_source_fallback() =>
                {
                    return Err(error.with_attempts(attempt));
                }
                Err(error) if error.is_retryable() && attempt < self.policy.max_attempts => {
                    let Some(delay) = self.retry_delay(&error, attempt) else {
                        return Err(error.with_attempts(attempt));
                    };
                    self.sleep_or_cancel(delay, spec.label).await?;
                    attempt += 1;
                }
                Err(error) => return Err(error.with_attempts(attempt)),
            }
        }
    }

    fn retry_delay(&self, error: &DownloadError, attempt: u32) -> Option<Duration> {
        let delay = error.retry_after().unwrap_or_else(|| {
            let multiplier = 1_u32 << (attempt - 1).min(4);
            self.policy.retry_delay.saturating_mul(multiplier)
        });
        (delay <= self.policy.max_automatic_retry_delay).then_some(delay)
    }

    /// Check before publishing or extracting a downloaded artifact, so a
    /// caller can retain staging when Pause arrives after the final chunk.
    pub fn ensure_not_cancelled(&self, label: &str) -> Result<(), DownloadError> {
        if self
            .cancellation
            .as_ref()
            .is_some_and(DownloadCancellation::is_cancelled)
        {
            Err(DownloadError::Cancelled {
                label: label.to_owned(),
            })
        } else {
            Ok(())
        }
    }

    async fn sleep_or_cancel(&self, delay: Duration, label: &str) -> Result<(), DownloadError> {
        let mut remaining = delay;
        let quantum = Duration::from_millis(100);
        while !remaining.is_zero() {
            self.ensure_not_cancelled(label)?;
            let current = remaining.min(quantum);
            tokio::time::sleep(current).await;
            remaining = remaining.saturating_sub(current);
        }
        self.ensure_not_cancelled(label)
    }

    async fn await_or_cancel<T>(
        &self,
        future: impl std::future::Future<Output = T>,
        label: &str,
    ) -> Result<T, DownloadError> {
        let mut future = std::pin::pin!(future);
        loop {
            self.ensure_not_cancelled(label)?;
            // Borrowing the pinned future preserves a pending request between
            // checks, while allowing Pause to stop stalled headers or bodies.
            if let Ok(result) =
                tokio::time::timeout(Duration::from_millis(100), future.as_mut()).await
            {
                return Ok(result);
            }
        }
    }

    async fn transfer_once(
        &self,
        spec: DownloadSpec<'_>,
        url: &str,
        partial: &Path,
        on_progress: &mut impl FnMut(DownloadProgress),
    ) -> Result<(), DownloadError> {
        let mut existing = match fs::metadata(partial) {
            Ok(metadata) => metadata.len(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
            Err(source) => {
                return Err(DownloadError::FileIo {
                    path: partial.to_path_buf(),
                    source,
                });
            }
        };
        if existing > spec.bytes {
            fs::remove_file(partial).map_err(|source| DownloadError::FileIo {
                path: partial.to_path_buf(),
                source,
            })?;
            existing = 0;
        }
        if existing == spec.bytes {
            on_progress(DownloadProgress {
                downloaded_bytes: existing,
                total_bytes: spec.bytes,
            });
            return Ok(());
        }

        let mut request = self.client.get(url).header(ACCEPT_ENCODING, "identity");
        if existing > 0 {
            request = request.header(RANGE, format!("bytes={existing}-"));
        }
        let mut response = self
            .await_or_cancel(request.send(), spec.label)
            .await?
            .map_err(|error| DownloadError::Transfer {
                label: spec.label.to_owned(),
                message: error.to_string(),
                attempts: 0,
            })?;
        let status = response.status();
        let append = match status {
            StatusCode::PARTIAL_CONTENT => {
                validate_content_range(&response, existing, spec.bytes, spec.label)?;
                existing > 0
            }
            StatusCode::OK => false,
            _ => {
                return Err(DownloadError::HttpStatus {
                    label: spec.label.to_owned(),
                    status,
                    retry_after: parse_retry_after(&response),
                    attempts: 0,
                });
            }
        };
        let mut downloaded = if append { existing } else { 0 };
        let expected_response_bytes = spec.bytes.saturating_sub(downloaded);
        if let Some(actual) = response.content_length()
            && actual != expected_response_bytes
        {
            return Err(DownloadError::RemoteSize {
                label: spec.label.to_owned(),
                expected: expected_response_bytes,
                actual,
            });
        }

        let mut output = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .open(partial)
            .await
            .map_err(|source| DownloadError::FileIo {
                path: partial.to_path_buf(),
                source,
            })?;
        on_progress(DownloadProgress {
            downloaded_bytes: downloaded,
            total_bytes: spec.bytes,
        });

        let result = loop {
            if let Err(error) = self.ensure_not_cancelled(spec.label) {
                break Err(error);
            }
            let chunk = match self.await_or_cancel(response.chunk(), spec.label).await {
                Err(error) => break Err(error),
                Ok(Ok(Some(chunk))) => chunk,
                Ok(Ok(None)) => break Ok(()),
                Ok(Err(error)) => {
                    break Err(DownloadError::Transfer {
                        label: spec.label.to_owned(),
                        message: error.to_string(),
                        attempts: 0,
                    });
                }
            };
            downloaded = downloaded.saturating_add(chunk.len() as u64);
            if downloaded > spec.bytes {
                break Err(DownloadError::Size {
                    path: partial.to_path_buf(),
                    expected: spec.bytes,
                    actual: downloaded,
                });
            }
            output
                .write_all(&chunk)
                .await
                .map_err(|source| DownloadError::FileIo {
                    path: partial.to_path_buf(),
                    source,
                })?;
            on_progress(DownloadProgress {
                downloaded_bytes: downloaded,
                total_bytes: spec.bytes,
            });
        };
        output
            .flush()
            .await
            .map_err(|source| DownloadError::FileIo {
                path: partial.to_path_buf(),
                source,
            })?;
        output
            .sync_all()
            .await
            .map_err(|source| DownloadError::FileIo {
                path: partial.to_path_buf(),
                source,
            })?;
        result?;
        if downloaded != spec.bytes {
            return Err(DownloadError::Incomplete {
                label: spec.label.to_owned(),
                expected: spec.bytes,
                actual: downloaded,
                attempts: 0,
            });
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum DownloadError {
    Client(String),
    InvalidSpec(String),
    FileIo {
        path: PathBuf,
        source: io::Error,
    },
    HttpStatus {
        label: String,
        status: StatusCode,
        retry_after: Option<Duration>,
        attempts: u32,
    },
    Transfer {
        label: String,
        message: String,
        attempts: u32,
    },
    Incomplete {
        label: String,
        expected: u64,
        actual: u64,
        attempts: u32,
    },
    RemoteSize {
        label: String,
        expected: u64,
        actual: u64,
    },
    Range {
        label: String,
        expected_start: u64,
        value: String,
    },
    Size {
        path: PathBuf,
        expected: u64,
        actual: u64,
    },
    Cancelled {
        label: String,
    },
}

impl DownloadError {
    #[must_use]
    pub const fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled { .. })
    }

    #[must_use]
    pub fn allows_source_fallback(&self) -> bool {
        matches!(
            self,
            Self::HttpStatus { .. }
                | Self::Transfer { .. }
                | Self::Incomplete { .. }
                | Self::RemoteSize { .. }
                | Self::Range { .. }
                | Self::Size { .. }
        )
    }

    fn is_retryable(&self) -> bool {
        match self {
            Self::Transfer { .. } | Self::Incomplete { .. } | Self::Size { .. } => true,
            Self::HttpStatus {
                status: StatusCode::FORBIDDEN,
                retry_after: Some(_),
                ..
            } => true,
            Self::HttpStatus { status, .. } => matches!(
                *status,
                StatusCode::REQUEST_TIMEOUT
                    | StatusCode::TOO_MANY_REQUESTS
                    | StatusCode::INTERNAL_SERVER_ERROR
                    | StatusCode::BAD_GATEWAY
                    | StatusCode::SERVICE_UNAVAILABLE
                    | StatusCode::GATEWAY_TIMEOUT
            ),
            _ => false,
        }
    }

    fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::HttpStatus { retry_after, .. } => *retry_after,
            _ => None,
        }
    }

    fn with_attempts(mut self, attempts: u32) -> Self {
        match &mut self {
            Self::HttpStatus {
                attempts: value, ..
            }
            | Self::Transfer {
                attempts: value, ..
            }
            | Self::Incomplete {
                attempts: value, ..
            } => *value = attempts,
            _ => {}
        }
        self
    }
}

impl fmt::Display for DownloadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Client(message) => write!(formatter, "cannot create download client: {message}"),
            Self::InvalidSpec(message) => formatter.write_str(message),
            Self::FileIo { path, source } => {
                write!(formatter, "cannot write {}: {source}", path.display())
            }
            Self::HttpStatus {
                label,
                status,
                retry_after,
                attempts,
            } => {
                write!(
                    formatter,
                    "download {label} returned HTTP {status} after {attempts} attempt(s)"
                )?;
                if let Some(delay) = retry_after {
                    write!(formatter, "; server requested a {}s delay", delay.as_secs())?;
                }
                formatter.write_str("; retry to resume")
            }
            Self::Transfer {
                label,
                message,
                attempts,
            } => write!(
                formatter,
                "download {label} was interrupted after {attempts} attempt(s): {message}; retry to resume"
            ),
            Self::Incomplete {
                label,
                expected,
                actual,
                attempts,
            } => write!(
                formatter,
                "download {label} stopped at {actual}/{expected} bytes after {attempts} attempt(s); retry to resume"
            ),
            Self::RemoteSize {
                label,
                expected,
                actual,
            } => write!(
                formatter,
                "download {label} reported {actual} bytes; the manifest expects {expected}"
            ),
            Self::Range {
                label,
                expected_start,
                value,
            } => write!(
                formatter,
                "download {label} returned invalid Content-Range {value:?}; expected byte {expected_start}"
            ),
            Self::Size {
                path,
                expected,
                actual,
            } => write!(
                formatter,
                "downloaded file {} is {actual} bytes; expected {expected}",
                path.display()
            ),
            Self::Cancelled { label } => write!(formatter, "download {label} was cancelled"),
        }
    }
}

impl Error for DownloadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::FileIo { source, .. } => Some(source),
            _ => None,
        }
    }
}

fn validate_spec(spec: DownloadSpec<'_>) -> Result<(), DownloadError> {
    let url = reqwest::Url::parse(spec.url).map_err(|error| {
        DownloadError::InvalidSpec(format!(
            "download {} has an invalid URL: {error}",
            spec.label
        ))
    })?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(DownloadError::InvalidSpec(format!(
            "download {} must use an HTTPS URL without embedded credentials",
            spec.label
        )));
    }
    if spec.bytes == 0 {
        return Err(DownloadError::InvalidSpec(format!(
            "download {} has no configured byte length",
            spec.label
        )));
    }
    Ok(())
}

fn validate_content_range(
    response: &reqwest::Response,
    expected_start: u64,
    expected_total: u64,
    label: &str,
) -> Result<(), DownloadError> {
    let value = response
        .headers()
        .get(CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let parsed = value
        .strip_prefix("bytes ")
        .and_then(|value| value.split_once('/'))
        .and_then(|(range, total)| {
            let (start, end) = range.split_once('-')?;
            Some((
                start.parse::<u64>().ok()?,
                end.parse::<u64>().ok()?,
                total.parse::<u64>().ok()?,
            ))
        });
    if parsed != Some((expected_start, expected_total - 1, expected_total)) {
        return Err(DownloadError::Range {
            label: label.to_owned(),
            expected_start,
            value: value.to_owned(),
        });
    }
    Ok(())
}

fn verify_file(path: &Path, spec: DownloadSpec<'_>) -> Result<(), DownloadError> {
    let actual_size = fs::metadata(path)
        .map_err(|source| DownloadError::FileIo {
            path: path.to_path_buf(),
            source,
        })?
        .len();
    if actual_size != spec.bytes {
        return Err(DownloadError::Size {
            path: path.to_path_buf(),
            expected: spec.bytes,
            actual: actual_size,
        });
    }
    Ok(())
}

fn parse_retry_after(response: &reqwest::Response) -> Option<Duration> {
    let value = response.headers().get(RETRY_AFTER)?.to_str().ok()?;
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    httpdate::parse_http_date(value)
        .ok()?
        .duration_since(std::time::SystemTime::now())
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::mpsc,
        thread,
        time::{Instant, SystemTime, UNIX_EPOCH},
    };

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    fn partial_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "xrtranslate-download-{}-{}.part",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn read_request(stream: &mut std::net::TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        String::from_utf8(request).unwrap()
    }

    fn response_once(response: &'static [u8]) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            stream.write_all(response).unwrap();
            request
        });
        (url, worker)
    }

    #[test]
    fn official_failure_is_reported_and_selected_mirror_resumes_the_partial() {
        let partial = partial_path();
        fs::write(&partial, b"abc").unwrap();
        let (official, official_worker) = response_once(
            b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        let (mirror, mirror_worker) = response_once(
            b"HTTP/1.1 206 Partial Content\r\nContent-Length: 3\r\nContent-Range: bytes 3-5/6\r\nConnection: close\r\n\r\ndef",
        );
        let client = DownloadClient::new("download-test").unwrap();
        let mut progress = Vec::new();
        let error = runtime()
            .block_on(client.transfer_with_retries(
                DownloadSpec::new("test", "https://github.com/example/pinned/model", 6),
                &partial,
                &official,
                &mut |value| progress.push(value),
            ))
            .unwrap_err();
        assert!(error.allows_source_fallback());
        assert_eq!(fs::read(&partial).unwrap(), b"abc");
        let mut mirror_client = client.clone();
        mirror_client.source = DownloadSource::Mirror;
        runtime()
            .block_on(mirror_client.transfer_with_retries(
                DownloadSpec::new("test", "https://github.com/example/pinned/model", 6),
                &partial,
                &mirror,
                &mut |value| progress.push(value),
            ))
            .unwrap();
        official_worker.join().unwrap();
        assert!(
            mirror_worker
                .join()
                .unwrap()
                .to_ascii_lowercase()
                .contains("range: bytes=3-")
        );
        assert_eq!(fs::read(&partial).unwrap(), b"abcdef");
        assert_eq!(progress.first().unwrap().downloaded_bytes, 3);
        assert_eq!(progress.last().unwrap().downloaded_bytes, 6);
        fs::remove_file(partial).unwrap();
    }

    #[test]
    fn pause_keeps_received_bytes_and_does_not_try_the_mirror() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let mirror = TcpListener::bind("127.0.0.1:0").unwrap();
        mirror.set_nonblocking(true).unwrap();
        let (release_tx, release_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_request(&mut stream);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\nabc")
                .unwrap();
            let _ = release_rx.recv_timeout(Duration::from_secs(3));
        });
        let cancellation = DownloadCancellation::default();
        let client = DownloadClient::with_proxy_source_and_cancellation(
            "download-test",
            None,
            DownloadSource::Official,
            cancellation.clone(),
        )
        .unwrap();
        let partial = partial_path();
        let result = runtime().block_on(client.transfer_with_retries(
            DownloadSpec::new("test", "https://github.com/example/pinned/model", 6),
            &partial,
            &url,
            &mut |progress| {
                if progress.downloaded_bytes == 3 {
                    cancellation.cancel();
                }
            },
        ));
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        assert!(result.unwrap_err().is_cancelled());
        assert_eq!(fs::read(&partial).unwrap(), b"abc");
        assert_eq!(
            mirror.accept().unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        fs::remove_file(partial).unwrap();
    }

    #[test]
    fn pause_interrupts_a_stalled_network_wait() {
        let cancellation = DownloadCancellation::default();
        let client = DownloadClient::with_proxy_source_and_cancellation(
            "download-test",
            None,
            DownloadSource::Official,
            cancellation.clone(),
        )
        .unwrap();
        let cancel = thread::spawn(move || {
            thread::sleep(Duration::from_millis(30));
            cancellation.cancel();
        });
        let started = Instant::now();
        let result =
            runtime().block_on(client.await_or_cancel(std::future::pending::<()>(), "test"));
        cancel.join().unwrap();
        assert!(result.unwrap_err().is_cancelled());
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
