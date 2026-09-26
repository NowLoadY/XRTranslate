use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

async fn inference(
    content: &'static str,
    provider: &str,
) -> (
    NativeInference,
    Arc<AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    let requests = Arc::new(AtomicUsize::new(0));
    let count = requests.clone();
    let router = axum::Router::new().route(
        "/v1/chat/completions",
        axum::routing::post(move || {
            count.fetch_add(1, Ordering::Relaxed);
            async move {
                axum::Json(serde_json::json!({"choices": [{"message": {"content": content}}]}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let mut document: serde_json::Value =
        serde_json::from_str(include_str!("../../../../config.json")).unwrap();
    document["asr"]["provider"] = provider.into();
    document["asr"]["providers"][provider]["api_key"] = "test-key".into();
    document["translation"]["provider"] = "openai".into();
    document["translation"]["providers"]["openai"]["api_key"] = "test-key".into();
    document["tts"]["enabled"] = false.into();
    let config = AppConfig::from_value(document).unwrap();
    let plan = NativeProviderPlan::resolve(&config, &std::env::temp_dir()).unwrap();
    let mut inference = NativeInference::new(&plan, None).unwrap();
    let http = ReqwestClient::with_default_direct_timeout().unwrap();
    inference.asr = if provider == "qwen" {
        NativeAsrAdapter::AudioChat(
            xrtranslate_inference::Qwen3AsrAdapter::new(http, &endpoint, "fixture").unwrap(),
        )
    } else {
        NativeAsrAdapter::OpenAi(
            xrtranslate_inference::OpenAiAsrAdapter::with_bearer_token(
                http, &endpoint, "fixture", "test-key",
            )
            .unwrap(),
        )
    };
    (inference, requests, server)
}

async fn recognize(
    inference: &NativeInference,
    source: &str,
    target: &str,
) -> Result<Option<RecognizedOutput>, InferenceFailure> {
    inference
        .transcribe(
            &vec![100; 16_000],
            source,
            target,
            &mut AdaptiveLanguageRoute::default(),
            &PromptNodeGraph::builtin_default(),
            AsrPromptContext::default(),
            &[],
        )
        .await
}

#[tokio::test]
async fn automatic_fixed_target_needs_one_asr_call_even_without_language_metadata() {
    let (inference, requests, server) =
        inference("We can meet near the station tomorrow.", "openai").await;
    let output = recognize(&inference, "auto", "zh").await.unwrap().unwrap();
    assert_eq!(output.source_language, "auto");
    assert_eq!(output.target_language, "zh");
    assert_eq!(output.route_switched, None);
    assert_eq!(requests.load(Ordering::Relaxed), 1);
    server.abort();
}

#[tokio::test]
async fn detected_input_is_checked_against_translation_support() {
    let (mut inference, requests, server) =
        inference("language Japanese<asr_text>こんにちは世界", "qwen").await;
    let output = recognize(&inference, "auto", "en").await.unwrap().unwrap();
    assert_eq!(
        (
            output.source_language.as_str(),
            output.target_language.as_str()
        ),
        ("ja", "en")
    );
    inference.languages.translation = Some(xrtranslate_engine::language::LanguageSet::from_codes(
        &["zh", "en"],
        false,
    ));
    let error = recognize(&inference, "auto", "en").await.err().unwrap();
    assert!(
        error.message.contains("Japanese is unavailable"),
        "{}",
        error.message
    );
    let before = requests.load(Ordering::Relaxed);
    assert!(recognize(&inference, "ja", "en").await.is_err());
    assert!(recognize(&inference, "auto", "zh,ja").await.is_err());
    assert_eq!(requests.load(Ordering::Relaxed), before);
    server.abort();
}
