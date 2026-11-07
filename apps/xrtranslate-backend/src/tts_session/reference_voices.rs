//! Temporary reference voices belong to a finite synthesis job, not the user's selection.
use super::speech::{SpeechError, decode_reference};
use crate::model_runtime::TtsAdapter;
use std::{collections::BTreeMap, time::Instant};
use xrtranslate_protocol::tts::SpeechReferenceContext;

#[derive(Default)]
pub(crate) struct ReferenceVoices(BTreeMap<(String, String), ReferenceVoice>);

struct ReferenceVoice {
    id: String,
    language: String,
    used: Instant,
}

impl ReferenceVoices {
    pub(super) async fn resolve(
        &mut self,
        context: &SpeechReferenceContext,
        audio: Option<&str>,
        language: &str,
        adapter: &TtsAdapter,
    ) -> Result<String, SpeechError> {
        if [&context.session, &context.voice]
            .iter()
            .any(|value| value.is_empty() || value.len() > 256)
        {
            return Err("Invalid voice reference session".into());
        }
        let key = (context.session.clone(), context.voice.clone());
        if let Some(voice) = self.0.get_mut(&key) {
            voice.used = Instant::now();
            return Ok(voice.id.clone());
        }
        let wav = decode_reference(audio.ok_or("A voice reference is required")?)?;
        let id = adapter.create_reference_voice(&wav, language).await?;
        // A stopped plugin must not leave an unbounded collection in the host.
        if self.0.len() >= 64
            && let Some(oldest) = self
                .0
                .iter()
                .min_by_key(|(_, voice)| voice.used)
                .map(|(key, _)| key.clone())
            && let Some(old) = self.0.remove(&oldest)
        {
            adapter.delete_reference_voice(&old.id, &old.language).await;
        }
        self.0.insert(
            key,
            ReferenceVoice {
                id: id.clone(),
                language: language.into(),
                used: Instant::now(),
            },
        );
        Ok(id)
    }

    fn take_session(&mut self, session: &str) -> Vec<ReferenceVoice> {
        let keys: Vec<_> = self
            .0
            .keys()
            .filter(|(scope, _)| scope == session)
            .cloned()
            .collect();
        keys.into_iter()
            .filter_map(|key| self.0.remove(&key))
            .collect()
    }
}

pub(crate) async fn release_reference_session(
    axum::extract::State(state): axum::extract::State<crate::BackendState>,
    axum::extract::Path(session): axum::extract::Path<String>,
) -> Result<axum::http::StatusCode, SpeechError> {
    // Finish removal even if the plugin closes while awaiting the response.
    tokio::spawn(async move {
        let speech = state.prepare_speech().await?;
        if let Some(adapter) = &speech.tts {
            let references = speech.reference_voices.lock().await.take_session(&session);
            for voice in references {
                adapter
                    .delete_reference_voice(&voice.id, &voice.language)
                    .await;
            }
        }
        Ok::<_, SpeechError>(axum::http::StatusCode::NO_CONTENT)
    })
    .await
    .map_err(|e| SpeechError::from(e.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, routing::post};
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};
    use xrtranslate_assets::remote::{RemoteConnector, remote_model};
    use xrtranslate_inference::{RemoteTtsAdapter, ReqwestClient};

    #[tokio::test]
    async fn one_enrollment_per_job_voice_and_cleanup_does_not_touch_other_jobs() {
        let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/synthesize", listener.local_addr().unwrap());
        let seen = requests.clone();
        let server = tokio::spawn(async move {
            let router = Router::new().route("/api/v1/services/audio/tts/customization", post(move |Json(body): Json<Value>| {
                let seen = seen.clone();
                async move {
                    let mut seen = seen.lock().unwrap();
                    seen.push(body.clone());
                    Json(match body["input"]["action"].as_str().unwrap() {
                        "create_voice" => json!({"output":{"voice_id":format!("qwen-audio-3.1-tts-plus-xrt-{}",seen.len())}}),
                        "query_voice" => json!({"output":{"status":"OK"}}),
                        "delete_voice" => json!({"output":{}}),
                        _ => panic!("unexpected action"),
                    })
                }
            }));
            axum::serve(listener, router).await.unwrap();
        });
        let adapter = TtsAdapter::remote(
            RemoteTtsAdapter::new(
                ReqwestClient::with_default_direct_timeout().unwrap(),
                endpoint,
                "test-key".into(),
                remote_model(RemoteConnector::DashscopeSpeech, "qwen-audio-3.1-tts-plus").unwrap(),
            ),
            vec!["en".into(), "zh".into()],
        );
        let audio = STANDARD.encode(
            xrtranslate_inference::pcm16_mono_16khz_to_wav(&vec![0; 16000 * 2 * 5]).unwrap(),
        );
        let mut voices = ReferenceVoices::default();
        let context = SpeechReferenceContext {
            session: "job-1".into(),
            voice: "speaker-A".into(),
        };
        let first = voices
            .resolve(&context, Some(&audio), "zh", &adapter)
            .await
            .unwrap();
        for _ in 0..3 {
            assert_eq!(
                voices
                    .resolve(&context, None, "zh", &adapter)
                    .await
                    .unwrap(),
                first
            );
        }
        assert_eq!(requests.lock().unwrap().len(), 2);
        let other = SpeechReferenceContext {
            session: "job-2".into(),
            voice: "speaker-A".into(),
        };
        let second = voices
            .resolve(&other, Some(&audio), "en", &adapter)
            .await
            .unwrap();
        assert_ne!(first, second);
        for voice in voices.take_session("job-1") {
            adapter
                .delete_reference_voice(&voice.id, &voice.language)
                .await;
        }
        assert_eq!(
            requests.lock().unwrap().last().unwrap()["input"]["voice_id"],
            first
        );
        assert_eq!(
            voices.resolve(&other, None, "en", &adapter).await.unwrap(),
            second
        );
        for voice in voices.take_session("job-2") {
            adapter
                .delete_reference_voice(&voice.id, &voice.language)
                .await;
        }
        assert!(voices.0.is_empty());
        assert_eq!(requests.lock().unwrap().len(), 6);
        server.abort();
    }
}
