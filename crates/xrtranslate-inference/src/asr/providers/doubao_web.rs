//! Doubao's account-authenticated web speech protocol. Catalogue keys are local
//! service profiles; this endpoint does not expose a selectable model version.
use crate::{AsrOptions, AsrTranscript, InferenceError, TransportError};
use futures_util::{SinkExt, Stream, StreamExt};
use serde_json::Value;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    time::{Duration, Instant, timeout},
};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{self, Message, client::IntoClientRequest},
};
use xrtranslate_connections::{AccountProvider, AccountSession, AccountStore};

const ENDPOINT: &str = "wss://ws-samantha.doubao.com/samantha/audio/asr";
const FRAME_BYTES: usize = 3_200;
const FINISH_TIMEOUT: Duration = Duration::from_secs(12);

#[derive(Clone, Debug)]
pub struct DoubaoWebAsrAdapter {
    accounts: AccountStore,
}

impl DoubaoWebAsrAdapter {
    pub fn new(
        endpoint: &str,
        model: &str,
        accounts: AccountStore,
    ) -> Result<Self, InferenceError> {
        // Never send account cookies to a user-overridden host or redirect.
        if endpoint != ENDPOINT || model != "doubao-web-speech" {
            return Err(InferenceError::InvalidConfiguration {
                field: "asr",
                message: "Select the registered Doubao account speech service.".into(),
            });
        }
        Ok(Self { accounts })
    }

    pub async fn transcribe_pcm16(
        &self,
        pcm: &[u8],
        options: AsrOptions,
    ) -> Result<AsrTranscript, InferenceError> {
        if pcm.is_empty() || pcm.len() % 2 != 0 {
            return Err(invalid_audio());
        }
        // Recordings are paced; live streams already carry the capture cadence.
        let frames = futures_util::stream::unfold((pcm, true), |(remaining, first)| async move {
            if remaining.is_empty() {
                return None;
            }
            if !first {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let end = remaining.len().min(FRAME_BYTES);
            Some((remaining[..end].to_vec(), (&remaining[end..], false)))
        });
        self.transcribe_stream(frames, options, |_| async {}).await
    }

    pub async fn transcribe_stream<S, F, Fut>(
        &self,
        audio: S,
        options: AsrOptions,
        on_update: F,
    ) -> Result<AsrTranscript, InferenceError>
    where
        S: Stream<Item = Vec<u8>> + Send,
        F: FnMut(String) -> Fut + Send,
        Fut: Future<Output = ()> + Send,
    {
        // No prompt/vocabulary/model/language-selection messages exist in this protocol.
        if options
            .language
            .as_deref()
            .is_some_and(|language| !matches!(language, "zh" | "en" | "auto" | ""))
        {
            return Err(InferenceError::InvalidConfiguration {
                field: "asr.language",
                message: "This account speech profile currently supports Chinese and English."
                    .into(),
            });
        }
        let session = self
            .accounts
            .load(AccountProvider::Doubao)
            .map_err(|_| {
                transport(
                    "account_store",
                    "Cannot read account connection. Sign in again.",
                )
            })?
            .ok_or_else(login_required)?;
        let request = request(&session)?;
        let socket = match timeout(
            Duration::from_secs(15),
            tokio_tungstenite::connect_async_tls_with_config(
                request,
                Some(
                    tungstenite::protocol::WebSocketConfig::default()
                        .max_message_size(Some(1024 * 1024))
                        .max_frame_size(Some(1024 * 1024)),
                ),
                false,
                tls_connector(),
            ),
        )
        .await
        {
            Ok(Ok((socket, _))) => socket,
            Ok(Err(tungstenite::Error::Http(response)))
                if response
                    .body()
                    .as_deref()
                    .and_then(|body| serde_json::from_slice::<Value>(body).ok())
                    .is_some_and(|value| auth_error(&value)) =>
            {
                self.expire(&session)?;
                return Err(login_required());
            }
            // Do not log raw handshake bodies, headers, URLs with IDs, or cookie values.
            Ok(Err(_)) => {
                return Err(transport(
                    "connect",
                    "Account speech connection failed. Check the network and try again.",
                ));
            }
            Err(_) => return Err(transport("timeout", "Account speech connection timed out.")),
        };
        let result = timeout(Duration::from_secs(90), drive(socket, audio, on_update))
            .await
            .unwrap_or_else(|_| Err(incomplete("session_timeout")));
        if matches!(result, Err(InferenceError::AccountLoginRequired { .. })) {
            self.expire(&session)?;
        }
        result
    }

    fn expire(&self, session: &AccountSession) -> Result<(), InferenceError> {
        self.accounts.expire(session).map_err(|_| {
            transport(
                "account_store",
                "Cannot update account connection. Sign out and sign in again.",
            )
        })
    }
}

// Android has no system PEM bundle for rustls-native-certs; match the HTTP
// connector's bundled public trust roots without disabling TLS verification.
fn tls_connector() -> Option<tokio_tungstenite::Connector> {
    #[cfg(target_os = "android")]
    {
        let roots = rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        let config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        Some(tokio_tungstenite::Connector::Rustls(Arc::new(config)))
    }
    #[cfg(not(target_os = "android"))]
    {
        None
    }
}

fn request(session: &AccountSession) -> Result<tungstenite::http::Request<()>, InferenceError> {
    let mut url = reqwest::Url::parse(ENDPOINT).expect("static endpoint");
    url.query_pairs_mut().extend_pairs([
        ("version_code", "20800"),
        ("language", "zh"),
        ("device_platform", "web"),
        ("aid", "497858"),
        ("real_aid", "497858"),
        ("pkg_type", "release_version"),
        ("pc_version", "3.12.3"),
        ("region", ""),
        ("sys_region", ""),
        ("samantha_web", "1"),
        ("use-olympus-account", "1"),
        ("format", "pcm"),
        (
            "device_id",
            session.attribute("device_id").unwrap_or_default(),
        ),
        ("web_id", session.attribute("web_id").unwrap_or_default()),
        ("tea_uuid", session.attribute("web_id").unwrap_or_default()),
        ("web_tab_id", &uuid::Uuid::new_v4().to_string()),
    ]);
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|_| login_required())?;
    request.headers_mut().insert(
        "User-Agent",
        xrtranslate_connections::login::spec(AccountProvider::Doubao)
            .expect("Doubao website login is registered")
            .user_agent
            .parse()
            .expect("static user agent"),
    );
    request.headers_mut().insert(
        "Origin",
        "https://www.doubao.com".parse().expect("static origin"),
    );
    let mut cookie = tungstenite::http::HeaderValue::from_str(session.cookie_header())
        .map_err(|_| login_required())?;
    cookie.set_sensitive(true);
    request.headers_mut().insert("Cookie", cookie);
    Ok(request)
}

async fn drive<W, S, F, Fut>(
    socket: WebSocketStream<W>,
    audio: S,
    mut on_update: F,
) -> Result<AsrTranscript, InferenceError>
where
    W: AsyncRead + AsyncWrite + Unpin + Send,
    S: Stream<Item = Vec<u8>> + Send,
    F: FnMut(String) -> Fut + Send,
    Fut: Future<Output = ()> + Send,
{
    let (mut write, mut read) = socket.split();
    let drained = Arc::new(AtomicBool::new(false));
    let sending_drained = drained.clone();
    let (eof_tx, mut eof_rx) = tokio::sync::oneshot::channel();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    let send = async move {
        let audio = crate::asr::audio::pcm16_frames(audio, FRAME_BYTES);
        futures_util::pin_mut!(audio);
        let mut received_audio = false;
        while let Some(frame) = audio.next().await {
            received_audio = true;
            write
                .send(Message::Binary(frame?.into()))
                .await
                .map_err(|_| transport("send", "Could not send audio."))?;
        }
        if !received_audio {
            return Err(invalid_audio());
        }
        sending_drained.store(true, Ordering::Release);
        let _ = eof_tx.send(());
        // Drain all real audio, then explicitly request the final result.
        // Quiet timers and socket closure never promote a preview.
        write
            .send(Message::Text(r#"{"event":"finish"}"#.into()))
            .await
            .map_err(|_| transport("send", "Could not finish audio input."))?;
        // Keep the writer alive for control frames while waiting for the peer.
        let _ = done_rx.await;
        Ok(())
    };
    let receive = async move {
        let mut text = String::new();
        let mut deadline = None;
        loop {
            let incoming = tokio::select! {
                _ = &mut eof_rx, if deadline.is_none() => { deadline = Some(Instant::now() + FINISH_TIMEOUT); continue; }
                _ = tokio::time::sleep_until(deadline.unwrap_or_else(|| Instant::now() + Duration::from_secs(90))), if deadline.is_some() => return Err(incomplete("missing_finish")),
                incoming = read.next() => incoming,
            };
            let message = match incoming {
                Some(Ok(Message::Text(text))) => text.as_bytes().to_vec(),
                Some(Ok(Message::Binary(bytes))) => bytes.to_vec(),
                Some(Ok(Message::Close(_))) | None => {
                    return Err(incomplete("closed_before_finish"));
                }
                Some(Ok(_)) => continue,
                Some(Err(_)) => {
                    return Err(transport(
                        "receive",
                        "Account speech connection was interrupted.",
                    ));
                }
            };
            match parse_event(&message)? {
                Event::Result(snapshot) => {
                    if snapshot != text {
                        text = snapshot;
                        on_update(text.clone()).await;
                    }
                }
                Event::Finish => {
                    if !drained.load(Ordering::Acquire) {
                        return Err(incomplete("finished_before_audio_drained"));
                    }
                    let _ = done_tx.send(());
                    return Ok(AsrTranscript {
                        language: None,
                        text,
                    });
                }
                Event::Other => {}
            }
        }
    };
    let (_, transcript) = tokio::try_join!(send, receive)?;
    Ok(transcript)
}

enum Event {
    Result(String),
    Finish,
    Other,
}
fn parse_event(bytes: &[u8]) -> Result<Event, InferenceError> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| transport("protocol", "Invalid account speech response."))?;
    if auth_error(&value) {
        return Err(login_required());
    }
    if value
        .get("code")
        .and_then(Value::as_i64)
        .is_some_and(|code| code != 0)
    {
        return Err(transport(
            "service",
            "Account speech service rejected the request. Try again later.",
        ));
    }
    match value.get("event").and_then(Value::as_str) {
        Some("result") => value
            .pointer("/result/Text")
            .and_then(Value::as_str)
            .map(|text| Event::Result(text.to_owned()))
            .ok_or_else(|| transport("protocol", "Missing speech result.")),
        Some("finish") => Ok(Event::Finish),
        _ => Ok(Event::Other),
    }
}

fn auth_error(value: &Value) -> bool {
    // Invalid (709599054) can also indicate a protocol error, so do not log out
    // on that code, HTTP status alone, silence, or transport failures.
    matches!(
        value.get("code").and_then(Value::as_i64),
        Some(710012001 | 710022013)
    )
}
fn login_required() -> InferenceError {
    InferenceError::AccountLoginRequired { provider: "Doubao" }
}
fn invalid_audio() -> InferenceError {
    InferenceError::InvalidAudio {
        message: "expected non-empty 16 kHz mono PCM16".into(),
    }
}
fn transport(kind: &str, message: &str) -> InferenceError {
    InferenceError::Transport {
        endpoint: ENDPOINT.into(),
        source: TransportError::new(kind, message),
    }
}
fn incomplete(reason: &str) -> InferenceError {
    InferenceError::IncompleteCompletion {
        endpoint: ENDPOINT.into(),
        finish_reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn drains_audio_and_requires_explicit_finish() {
        for finish in [false, true] {
            let (client, server) = tokio::io::duplex(64 * 1024);
            let client =
                WebSocketStream::from_raw_socket(client, tungstenite::protocol::Role::Client, None)
                    .await;
            let mut server =
                WebSocketStream::from_raw_socket(server, tungstenite::protocol::Role::Server, None)
                    .await;
            let peer = async move {
                let first = server.next().await.unwrap().unwrap();
                assert_eq!(first.into_data().as_ref(), &[1; FRAME_BYTES]);
                server
                    .send(Message::Text(
                        r#"{"event":"result","code":0,"result":{"Text":"测试"}}"#.into(),
                    ))
                    .await
                    .unwrap();
                let tail = server.next().await.unwrap().unwrap();
                assert_eq!(tail.into_data().as_ref(), &[2, 0, 3, 0]);
                let terminal = server.next().await.unwrap().unwrap();
                assert_eq!(
                    terminal.into_text().unwrap().as_str(),
                    r#"{"event":"finish"}"#
                );
                if finish {
                    server
                        .send(Message::Text(r#"{"event":"finish","code":0}"#.into()))
                        .await
                        .unwrap();
                } else {
                    server.close(None).await.unwrap();
                }
            };
            let mut pcm = vec![1; FRAME_BYTES];
            pcm.extend_from_slice(&[2, 0, 3, 0]);
            let audio = futures_util::stream::iter(pcm.chunks(1024).map(<[u8]>::to_vec));
            let (result, _) = tokio::join!(drive(client, audio, |_| async {}), peer);
            if finish {
                assert_eq!(result.unwrap().text, "测试");
            } else {
                assert!(matches!(
                    result,
                    Err(InferenceError::IncompleteCompletion { .. })
                ));
            }
        }
    }
    #[test]
    fn auth_errors_are_explicit_and_previews_can_retract() {
        assert!(matches!(
            parse_event(br#"{"code":710012001}"#),
            Err(InferenceError::AccountLoginRequired { .. })
        ));
        assert!(!auth_error(&serde_json::json!({"code": 709599054})));
        assert!(
            matches!(parse_event(br#"{"event":"result","code":0,"result":{"Text":""}}"#).unwrap(), Event::Result(text) if text.is_empty())
        );
    }
}
