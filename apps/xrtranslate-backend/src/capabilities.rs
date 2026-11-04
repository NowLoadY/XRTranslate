//! Shared resources are prepared by the first consumer that needs them.
use crate::{
    BackendState, OnnxRuntimeDiagnostic, RuntimeLayout, TtsAdapter, VoiceLibrary,
    initialize_managed_onnx_runtime, runtime_diagnostic, start_llama_servers,
    wait_for_model_servers,
};
use std::sync::Arc;
use xrtranslate_assets::ModelCapability;

pub(crate) struct TextResources {
    plan: crate::NativeProviderPlan,
    inference: tokio::sync::OnceCell<crate::NativeInference>,
    _processes: Vec<crate::LlamaServerProcess>,
}

pub(crate) struct SpeechResources {
    pub tts: Option<TtsAdapter>,
    pub voices: Arc<VoiceLibrary>,
    pub diagnostic: OnnxRuntimeDiagnostic,
}

impl BackendState {
    /// Both roles share the same adapters and scheduler. A separate model is
    /// prepared only when Agent first needs it; following reuses translation.
    pub(crate) async fn prepare_text(
        &self,
        target: xrtranslate_protocol::AskTarget,
    ) -> Result<&crate::NativeInference, String> {
        let resources = if target == xrtranslate_protocol::AskTarget::Agent {
            self.agent_runtime
                .get_or_try_init(|| async {
                    let Some(config) = self.config.for_agent()? else {
                        return Ok(None);
                    };
                    let mut plan = crate::NativeProviderPlan::resolve(&config, &self.project_root)?;
                    plan.check_capability_assets(ModelCapability::Translation)?;
                    let mut processes = Vec::new();
                    if self.manage_models {
                        crate::assign_managed_ports(&mut plan)?;
                        processes = start_llama_servers(&plan, ModelCapability::Translation)?;
                        wait_for_model_servers(
                            &plan,
                            self.model_start_timeout,
                            &mut processes,
                            ModelCapability::Translation,
                        )
                        .await?;
                    }
                    Ok::<_, String>(Some(TextResources {
                        plan,
                        inference: Default::default(),
                        _processes: processes,
                    }))
                })
                .await?
                .as_ref()
        } else {
            None
        };
        let (plan, inference) = resources.map_or(
            (self.model_plan.as_ref(), self.text_inference.as_ref()),
            |resources| (&resources.plan, &resources.inference),
        );
        if target == xrtranslate_protocol::AskTarget::Agent && !plan.translation_supports_chat()? {
            return Err("Selected Agent model does not support general chat".into());
        }
        if !plan.translation_supports_ask()? {
            return Err("Selected translation model does not support Ask instructions".into());
        }
        // Reuse the stateless adapter and its HTTP pool across rounds, without
        // retaining messages or credentials outside their existing owners.
        inference
            .get_or_try_init(|| async { crate::NativeInference::for_text(plan) })
            .await
    }

    async fn prepare_onnx(&self) -> Result<(), String> {
        self.onnx_runtime
            .get_or_try_init(|| async {
                initialize_managed_onnx_runtime(&self.project_root, &self.config)
            })
            .await
            .copied()
    }

    pub(crate) async fn prepare_audio(&self) -> Result<(), String> {
        self.audio_runtime
            .get_or_try_init(|| async {
                self.model_plan
                    .check_capability_assets(ModelCapability::Asr)?;
                self.prepare_onnx().await?;
                let mut processes = if self.manage_models {
                    start_llama_servers(&self.model_plan, ModelCapability::Asr)?
                } else {
                    Vec::new()
                };
                if self.manage_models {
                    wait_for_model_servers(
                        &self.model_plan,
                        self.model_start_timeout,
                        &mut processes,
                        ModelCapability::Asr,
                    )
                    .await?;
                }
                Ok::<_, String>(processes)
            })
            .await
            .map(|_| ())
    }

    pub(crate) async fn prepare_speech(&self) -> Result<&SpeechResources, String> {
        self.speech_runtime
            .get_or_try_init(|| async {
                let tts = self.model_plan.tts_adapter(&self.config)?;
                let diagnostic = if let Some(adapter) =
                    tts.as_ref().filter(|adapter| adapter.is_local())
                {
                    self.model_plan
                        .check_capability_assets(ModelCapability::Tts)?;
                    self.prepare_onnx().await?;
                    let device = adapter.prepare().await.map_err(|error| error.to_string())?;
                    device
                        .map(|device| runtime_diagnostic(&self.project_root, &self.config, device))
                        .unwrap_or_default()
                } else {
                    OnnxRuntimeDiagnostic::default()
                };
                let directory =
                    RuntimeLayout::for_config(&self.project_root, &self.config.model_manager)
                        .voice_clones_directory();
                let voices = Arc::new(
                    VoiceLibrary::open(
                        directory,
                        self.config.tts.provider.clone(),
                        format!(
                            "{}:{}",
                            self.config.tts.provider,
                            self.config
                                .tts
                                .provider_config(&self.config.tts.provider)
                                .and_then(|provider| provider.get("model"))
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("local")
                        ),
                        tts.clone(),
                    )
                    .await,
                );
                Ok::<_, String>(SpeechResources {
                    tts,
                    voices,
                    diagnostic,
                })
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        extract::{ConnectInfo, State},
        response::IntoResponse,
        routing::post,
    };
    use serde_json::{Value, json};
    use xrtranslate_protocol::{AskRequest, AskTarget};

    #[tokio::test]
    async fn text_roles_route_independently_and_do_not_accumulate_history() {
        let (sender, mut received) =
            tokio::sync::mpsc::unbounded_channel::<(std::net::SocketAddr, Value)>();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://{}/v1/chat/completions",
            listener.local_addr().unwrap()
        );
        let mock = Router::new().route("/v1/chat/completions", post(move |ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>, Json(body): Json<Value>| {
            let sender = sender.clone();
            async move {
                sender.send((peer, body)).unwrap();
                Json(json!({"choices":[{"message":{"role":"assistant","content":"Reviewed"},"finish_reason":"stop"}]}))
            }
        }));
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                mock.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap()
        });
        let temp = tempfile::tempdir().unwrap();
        let config = crate::AppConfig::from_value(json!({
            "asr":{"provider":"qwen","providers":{"qwen":{"api_key":"test-only"}}},
            "translation":{"provider":"fixture","providers":{
                "fixture":{"connector":"openai-chat","url":url,"model":"translation-fixture","api_key":"test-only","capabilities":{"supports_ask":true}},
                "assistant":{"connector":"openai-chat","url":url,"model":"agent-fixture","api_key":"test-only","capabilities":{"supports_chat":true}}
            }},
            "agent":{"model":{"provider":"assistant","model":"agent-fixture"}}
        })).unwrap();
        let state = BackendState {
            model_plan: Arc::new(crate::NativeProviderPlan::resolve(&config, temp.path()).unwrap()),
            config,
            corpus_client: crate::CorpusClient::new("http://127.0.0.1:1").unwrap(),
            contexts: crate::ContextRegistry::default(),
            project_root: temp.path().to_owned(),
            onnx_runtime: Default::default(),
            audio_runtime: Default::default(),
            text_inference: Default::default(),
            agent_runtime: Default::default(),
            speech_runtime: Default::default(),
            manage_models: false,
            model_start_timeout: 1,
            next_session_id: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            inference_scheduler: crate::InferenceScheduler::new(1, 1),
        };
        let messages = serde_json::from_value::<Vec<xrtranslate_prompt::PromptMessage>>(json!([
            {"role":"user","content":"Review"},
            {"role":"assistant","content":"Read the result"},
            {"role":"user","content":"Tool feedback: valid"}
        ]))
        .unwrap();
        let mut agent_peer = None;
        for (target, model) in [
            (AskTarget::Translation, "translation-fixture"),
            (AskTarget::Agent, "agent-fixture"),
            (AskTarget::Agent, "agent-fixture"),
        ] {
            let request = AskRequest::Messages {
                target,
                messages: messages.clone(),
                response_schema: None,
            };
            let response = crate::ask(State(state.clone()), Json(request))
                .await
                .into_response();
            assert_eq!(response.status(), axum::http::StatusCode::OK);
            let (peer, payload) = received.try_recv().unwrap();
            if target == AskTarget::Agent {
                assert_eq!(
                    *agent_peer.get_or_insert(peer),
                    peer,
                    "reuse the TCP connection across Agent rounds"
                );
            }
            assert_eq!(payload["model"], model);
            assert_eq!(
                payload["messages"],
                serde_json::to_value(&messages).unwrap()
            );
            assert!(payload.get("conversation").is_none());
            assert!(payload.get("previous_response_id").is_none());
        }
        let mut following = state.clone();
        following.config.agent.model = None;
        following.agent_runtime = Default::default();
        let response = crate::ask(
            State(following),
            Json(AskRequest::Messages {
                target: AskTarget::Agent,
                messages,
                response_schema: None,
            }),
        )
        .await
        .into_response();
        assert_eq!(
            response.status(),
            axum::http::StatusCode::BAD_GATEWAY,
            "Ask-only translation must not enable Agent"
        );
        assert!(
            received.try_recv().is_err(),
            "capability rejection must happen before inference"
        );
        server.abort();
    }
}
