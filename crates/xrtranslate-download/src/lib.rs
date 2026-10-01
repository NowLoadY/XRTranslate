//! Shared immutable-artifact transfer infrastructure.
//!
//! Model, runtime, plugin-component, and application-update installers select
//! their own artifacts and own extraction/activation. They all delegate HTTPS,
//! proxying, retries, resume validation, cache recovery, progress, and integrity
//! checks to this crate so those guarantees cannot drift between features.

#![forbid(unsafe_code)]

mod source;

pub use source::DownloadSource;

use reqwest::{
    StatusCode,
    header::{ACCEPT_ENCODING, CONTENT_RANGE, RANGE, RETRY_AFTER},
};
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    fmt, fs,
    io::{self, Read},
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

/// Cooperative cancellation shared by a UI-owned download worker and the
/// control that replaces its source. Cancellation is checked between network
/// chunks and retry delays so the worker releases its `.part` file before the
/// owning installer removes staging.
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
    integrity: DownloadIntegrity<'a>,
}

#[derive(Clone, Copy, Debug)]
enum DownloadIntegrity<'a> {
    Sha256(&'a str),
    SizeOnly,
}

impl<'a> DownloadSpec<'a> {
    /// Creates an immutable artifact contract verified by length and SHA-256.
    pub const fn verified(label: &'a str, url: &'a str, bytes: u64, sha256: &'a str) -> Self {
        Self {
            label,
            url,
            bytes,
            integrity: DownloadIntegrity::Sha256(sha256),
        }
    }

    /// Creates a length-only contract for a trusted HTTPS source that does not
    /// publish a digest. Prefer [`Self::verified`] whenever a digest exists.
    pub const fn size_only(label: &'a str, url: &'a str, bytes: u64) -> Self {
        Self {
            label,
            url,
            bytes,
            integrity: DownloadIntegrity::SizeOnly,
        }
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
        if complete.is_file() {
            match verify_file(complete, spec) {
                Ok(()) => {
                    on_progress(DownloadProgress {
                        downloaded_bytes: spec.bytes,
                        total_bytes: spec.bytes,
                    });
                    return Ok(());
                }
                Err(error) => {
                    fs::remove_file(complete).map_err(|source| DownloadError::FileIo {
                        path: complete.to_path_buf(),
                        source,
                    })?;
                    if !matches!(
                        error,
                        DownloadError::Size { .. } | DownloadError::Integrity { .. }
                    ) {
                        return Err(error);
                    }
                }
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

        for attempt in 1..=self.policy.max_attempts {
            self.ensure_not_cancelled(spec.label)?;
            match self.transfer_once(spec, partial, &mut on_progress).await {
                Ok(()) => {
                    if let Err(error) = verify_file(partial, spec) {
                        let _ = fs::remove_file(partial);
                        if matches!(
                            error,
                            DownloadError::Size { .. } | DownloadError::Integrity { .. }
                        ) && attempt < self.policy.max_attempts
                        {
                            let Some(delay) = self.retry_delay(&error, attempt) else {
                                return Err(error);
                            };
                            self.sleep_or_cancel(delay, spec.label).await?;
                            continue;
                        }
                        return Err(error);
                    }
                    tokio::fs::rename(partial, complete)
                        .await
                        .map_err(|source| DownloadError::FileIo {
                            path: complete.to_path_buf(),
                            source,
                        })?;
                    return Ok(());
                }
                Err(error) if error.is_retryable() && attempt < self.policy.max_attempts => {
                    let Some(delay) = self.retry_delay(&error, attempt) else {
                        return Err(error.with_attempts(attempt));
                    };
                    self.sleep_or_cancel(delay, spec.label).await?;
                }
                Err(error) => return Err(error.with_attempts(attempt)),
            }
        }
        unreachable!("a non-zero retry policy always returns from the loop")
    }

    fn retry_delay(&self, error: &DownloadError, attempt: u32) -> Option<Duration> {
        let delay = error.retry_after().unwrap_or_else(|| {
            let multiplier = 1_u32 << (attempt - 1).min(4);
            self.policy.retry_delay.saturating_mul(multiplier)
        });
        (delay <= self.policy.max_automatic_retry_delay).then_some(delay)
    }

    fn ensure_not_cancelled(&self, label: &str) -> Result<(), DownloadError> {
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

    async fn transfer_once(
        &self,
        spec: DownloadSpec<'_>,
        partial: &Path,
        on_progress: &mut impl FnMut(DownloadProgress),
    ) -> Result<(), DownloadError> {
        let mut existing = fs::metadata(partial)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        if existing > spec.bytes {
            fs::remove_file(partial).map_err(|source| DownloadError::FileIo {
                path: partial.to_path_buf(),
                source,
            })?;
            existing = 0;
        }
        if existing == spec.bytes {
            return Ok(());
        }

        let resolved_url = self.source.resolve(spec.url);
        let mut request = self
            .client
            .get(resolved_url.as_ref())
            .header(ACCEPT_ENCODING, "identity");
        if existing > 0 {
            request = request.header(RANGE, format!("bytes={existing}-"));
        }
        let mut response = request
            .send()
            .await
            .map_err(|error| DownloadError::Transfer {
                label: spec.label.to_owned(),
                message: error.to_string(),
                attempts: 0,
            })?;
        let status = response.status();
        if !status.is_success() {
            return Err(DownloadError::HttpStatus {
                label: spec.label.to_owned(),
                status,
                retry_after: parse_retry_after(&response),
                attempts: 0,
            });
        }

        let append = if existing > 0 && status == StatusCode::PARTIAL_CONTENT {
            validate_content_range(&response, existing, spec.bytes, spec.label)?;
            true
        } else if existing > 0 && status == StatusCode::OK {
            false
        } else if existing == 0 && status == StatusCode::PARTIAL_CONTENT {
            validate_content_range(&response, 0, spec.bytes, spec.label)?;
            false
        } else {
            existing == 0
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

        loop {
            self.ensure_not_cancelled(spec.label)?;
            let chunk = match response.chunk().await {
                Ok(chunk) => chunk,
                Err(error) => {
                    let _ = output.flush().await;
                    let _ = output.sync_all().await;
                    return Err(DownloadError::Transfer {
                        label: spec.label.to_owned(),
                        message: error.to_string(),
                        attempts: 0,
                    });
                }
            };
            let Some(chunk) = chunk else { break };
            downloaded = downloaded.saturating_add(chunk.len() as u64);
            if downloaded > spec.bytes {
                let _ = output.flush().await;
                drop(output);
                let _ = fs::remove_file(partial);
                return Err(DownloadError::Size {
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
        }
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
    Integrity {
        path: PathBuf,
        expected: String,
        actual: String,
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

    fn is_retryable(&self) -> bool {
        match self {
            Self::Transfer { .. } | Self::Incomplete { .. } => true,
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
            Self::Integrity {
                path,
                expected,
                actual,
            } => write!(
                formatter,
                "downloaded file {} has SHA-256 {actual}; expected {expected}",
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
    if let DownloadIntegrity::Sha256(sha256) = spec.integrity
        && (sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err(DownloadError::InvalidSpec(format!(
            "download {} has an invalid SHA-256",
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
            let (start, _) = range.split_once('-')?;
            Some((start.parse::<u64>().ok()?, total.parse::<u64>().ok()?))
        });
    if parsed != Some((expected_start, expected_total)) {
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
    if let DownloadIntegrity::Sha256(expected) = spec.integrity {
        let actual = sha256_file(path).map_err(|source| DownloadError::FileIo {
            path: path.to_path_buf(),
            source,
        })?;
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(DownloadError::Integrity {
                path: path.to_path_buf(),
                expected: expected.to_owned(),
                actual,
            });
        }
    }
    Ok(())
}

fn parse_retry_after(response: &reqwest::Response) -> Option<Duration> {
    let value = response.headers().get(RETRY_AFTER)?.to_str().ok()?;
    parse_retry_after_value(value, std::time::SystemTime::now())
}

fn parse_retry_after_value(value: &str, now: std::time::SystemTime) -> Option<Duration> {
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    httpdate::parse_http_date(value)
        .ok()?
        .duration_since(now)
        .ok()
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_verified_and_explicit_size_only_contracts() {
        assert!(
            validate_spec(DownloadSpec::verified(
                "artifact",
                "https://example.com/file.zip",
                42,
                &"a".repeat(64),
            ))
            .is_ok()
        );
        assert!(
            validate_spec(DownloadSpec::size_only(
                "artifact",
                "https://example.com/file.zip",
                42,
            ))
            .is_ok()
        );
        assert!(
            validate_spec(DownloadSpec::size_only(
                "artifact",
                "https://user:secret@example.com/file.zip",
                42,
            ))
            .is_err()
        );
    }

    #[test]
    fn retry_after_accepts_seconds_and_http_dates() {
        let now = std::time::UNIX_EPOCH + Duration::from_secs(1_000);
        assert_eq!(
            parse_retry_after_value("17", now),
            Some(Duration::from_secs(17))
        );
        let later = now + Duration::from_secs(30);
        let date = httpdate::fmt_http_date(later);
        assert_eq!(
            parse_retry_after_value(&date, now),
            Some(Duration::from_secs(30))
        );

        let client = DownloadClient {
            client: reqwest::Client::new(),
            policy: DownloadPolicy::default(),
            source: DownloadSource::Official,
            cancellation: None,
        };
        let excessive = DownloadError::HttpStatus {
            label: "artifact".into(),
            status: StatusCode::TOO_MANY_REQUESTS,
            retry_after: Some(Duration::from_secs(600)),
            attempts: 0,
        };
        assert_eq!(client.retry_delay(&excessive, 1), None);
    }

    #[test]
    fn cancelled_transfer_stops_before_opening_a_partial_file() {
        let cancellation = DownloadCancellation::default();
        cancellation.cancel();
        let client = DownloadClient::with_proxy_source_and_cancellation(
            "xrtranslate-download-test",
            None,
            DownloadSource::Official,
            cancellation,
        )
        .unwrap();
        let root = std::env::temp_dir().join(format!(
            "xrtranslate-download-cancelled-{}",
            std::process::id()
        ));
        let complete = root.join("artifact.bin");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let error = runtime
            .block_on(client.download_to(
                DownloadSpec::size_only("artifact", "https://example.invalid/artifact", 1),
                &complete,
                |_| {},
            ))
            .unwrap_err();

        assert!(error.is_cancelled());
        assert!(!complete.with_file_name("artifact.bin.part").exists());
    }

    #[test]
    fn size_only_skips_hashing_but_still_enforces_length() {
        let path = std::env::temp_dir().join(format!(
            "xrtranslate-download-contract-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&path, b"payload").unwrap();
        let spec = DownloadSpec::size_only("artifact", "https://example.com/file", 7);
        assert!(verify_file(&path, spec).is_ok());
        let wrong_size = DownloadSpec::size_only("artifact", "https://example.com/file", 8);
        assert!(matches!(
            verify_file(&path, wrong_size),
            Err(DownloadError::Size { .. })
        ));
        fs::remove_file(path).unwrap();
    }
}
