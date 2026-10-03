use super::*;
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn incomplete_translation_retries_once_with_room_to_finish_and_clears_the_preview() {
    for (first, second, limit, window, retry_limit) in [
        ("length", "stop", 256, 2048, Some(512)),
        ("length", "length", 256, 2048, Some(512)),
        ("length", "stop", 768, 2048, Some(1024)),
        ("length", "stop", 1536, 2048, Some(1536)),
        ("content_filter", "stop", 256, 2048, None),
        ("tool_calls", "stop", 256, 2048, None),
    ] {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let router = axum::Router::new().route(
            "/v1/chat/completions",
            axum::routing::post(move |axum::Json(request): axum::Json<serde_json::Value>| {
                let mut requests = captured.lock().unwrap();
                let reason = if requests.is_empty() { first } else { second };
                requests.push(request);
                let text = if reason == "stop" {
                    "你好，朋友。"
                } else {
                    "你好"
                };
                let body = format!(
                    "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                    serde_json::json!({"choices": [{"delta": {"content": text}}]}),
                    serde_json::json!({"choices": [{"delta": {}, "finish_reason": reason}]})
                );
                async move {
                    (
                        [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                        body,
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let config = AppConfig::from_json_str(include_str!("../../../../config.json")).unwrap();
        let mut plan = NativeProviderPlan::resolve(&config, &std::env::temp_dir()).unwrap();
        plan.set_managed_ports(None, Some(port)).unwrap();
        let mut inference = NativeInference::for_text(&plan).unwrap();
        inference.translation_max_output_tokens = limit;
        inference.translation_context_window_tokens = window;
        let context = TranslationPromptContext {
            terminology_rows: vec!["CONTEXT_MARKER".into()],
            ..Default::default()
        };
        let mut updates = Vec::new();
        let result = inference
            .translate_segment_streaming(
                &TranslationSegmentPair {
                    source_text: "Hello, friend.".into(),
                    translation_text: "Hello, friend.".into(),
                },
                "en",
                "zh",
                PromptNodeGraph::builtin_default(),
                context,
                |text| {
                    updates.push(text);
                    std::future::ready(())
                },
            )
            .await;
        server.abort();

        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), if retry_limit.is_some() { 2 } else { 1 });
        assert_eq!(requests[0]["max_tokens"], limit);
        assert!(
            requests[0]["messages"]
                .to_string()
                .contains("CONTEXT_MARKER")
        );
        if let Some(retry_limit) = retry_limit {
            assert_eq!(requests[1]["max_tokens"], retry_limit);
            let messages = requests[1]["messages"].to_string();
            assert!(!messages.contains("CONTEXT_MARKER"));
            assert!(messages.contains("Hello, friend."));
            assert!(updates.iter().any(String::is_empty));
        }
        if retry_limit.is_some() && second == "stop" {
            assert_eq!(result.unwrap().translated_text, "你好，朋友。");
            assert_eq!(updates.last().unwrap(), "你好，朋友。");
        } else {
            let error = result.err().unwrap();
            assert!(
                error
                    .message
                    .contains(if retry_limit.is_some() { second } else { first })
            );
            assert!(updates.last().unwrap().is_empty());
        }
    }
}
