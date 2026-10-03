use std::{error::Error, fmt};

/// A transport failure before an HTTP response was received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportError {
    /// Short machine-readable classification such as `timeout` or `connect`.
    pub kind: String,
    /// Human-readable diagnostic suitable for a backend error event.
    pub message: String,
}

impl TransportError {
    pub fn new(kind: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for TransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.kind, self.message)
    }
}

impl Error for TransportError {}

/// A structured, user-readable failure returned by an inference adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InferenceError {
    /// An adapter was constructed with an unusable value.
    InvalidConfiguration {
        field: &'static str,
        message: String,
    },
    /// PCM16 input cannot be represented in a WAV container.
    InvalidAudio { message: String },
    /// The HTTP client did not receive a response.
    Transport {
        endpoint: String,
        source: TransportError,
    },
    /// The service returned a non-success status.
    HttpStatus {
        endpoint: String,
        status: u16,
        body_preview: String,
    },
    /// The service response was not an OpenAI chat-completions document.
    InvalidResponse {
        endpoint: String,
        message: String,
        body_preview: String,
    },
    /// The provider ended generation without completing the requested text.
    IncompleteCompletion {
        endpoint: String,
        finish_reason: String,
    },
    /// The model completed the request but produced no usable text.
    EmptyOutput { operation: &'static str },
    /// The model completed the request but echoed instructions or other prompt
    /// material instead of producing a usable domain result.
    RejectedOutput {
        operation: &'static str,
        reason: &'static str,
    },
}

impl fmt::Display for InferenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration { field, message } => {
                write!(formatter, "invalid {field}: {message}")
            }
            Self::InvalidAudio { message } => write!(formatter, "invalid PCM16 audio: {message}"),
            Self::Transport { endpoint, source } => {
                write!(
                    formatter,
                    "inference request to {endpoint} failed: {source}"
                )
            }
            Self::HttpStatus {
                endpoint,
                status,
                body_preview,
            } => write!(
                formatter,
                "inference request to {endpoint} returned HTTP {status}: {body_preview}"
            ),
            Self::InvalidResponse {
                endpoint,
                message,
                body_preview,
            } => write!(
                formatter,
                "invalid inference response from {endpoint}: {message} ({body_preview})"
            ),
            Self::EmptyOutput { operation } => {
                write!(formatter, "{operation} completed without usable text")
            }
            Self::IncompleteCompletion {
                endpoint,
                finish_reason,
            } => write!(
                formatter,
                "incomplete inference response from {endpoint} (finish_reason: {finish_reason})"
            ),
            Self::RejectedOutput { operation, reason } => {
                write!(formatter, "{operation} output was rejected: {reason}")
            }
        }
    }
}

impl Error for InferenceError {}

impl InferenceError {
    /// Whether retrying requires the user to correct a remote provider's
    /// credentials, endpoint, model, or request configuration.
    #[must_use]
    pub fn requires_provider_configuration(&self) -> bool {
        match self {
            Self::InvalidConfiguration { .. } => true,
            Self::HttpStatus {
                endpoint, status, ..
            } => {
                [400, 401, 403, 404, 422].contains(status)
                    && reqwest::Url::parse(endpoint)
                        .ok()
                        .and_then(|url| url.host_str().map(str::to_owned))
                        .is_some_and(|host| {
                            host != "localhost" && host != "127.0.0.1" && host != "::1"
                        })
            }
            _ => false,
        }
    }

    /// Whether the provider returned text that failed a deterministic output
    /// quality gate and may be worth regenerating once.
    #[must_use]
    pub fn is_rejected_output(&self) -> bool {
        matches!(self, Self::RejectedOutput { .. })
    }

    /// A larger generation budget may recover an output or context limit.
    #[must_use]
    pub fn is_output_limit(&self) -> bool {
        matches!(self, Self::IncompleteCompletion { finish_reason, .. } if finish_reason == "length")
    }
}

pub(crate) fn preview(body: &str) -> String {
    const LIMIT: usize = 512;
    let normalized = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= LIMIT {
        return normalized;
    }
    let prefix = normalized.chars().take(LIMIT).collect::<String>();
    format!("{prefix}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_actionable_remote_http_failures_require_configuration() {
        let failure = |endpoint: &str, status| InferenceError::HttpStatus {
            endpoint: endpoint.into(),
            status,
            body_preview: String::new(),
        };
        assert!(
            failure("https://api.openai.com/v1/audio/transcriptions", 401)
                .requires_provider_configuration()
        );
        assert!(
            failure("http://provider.internal/v1/chat/completions", 404)
                .requires_provider_configuration()
        );
        assert!(
            !failure("http://127.0.0.1:8080/v1/chat/completions", 400)
                .requires_provider_configuration()
        );
        assert!(
            !failure("https://api.openai.com/v1/chat/completions", 429)
                .requires_provider_configuration()
        );
    }
}
