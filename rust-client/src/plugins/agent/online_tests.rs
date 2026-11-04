//! Opt-in live regression using the production agent, media tools and model transport.
//! Requires XRT_AGENT_LIVE_{ROOT,PROVIDER,FIXTURE,OUTPUT}; never writes the source task.
use super::*;
use crate::plugins::media::{MediaPlugin, task::MediaTask};
use std::{fs, io::Write, path::PathBuf};
use xrtranslate_config::{AppConfig, load_user_config_document};
use xrtranslate_connections::AccountStore;
use xrtranslate_inference::{ReqwestClient, TranslationAdapter};
use xrtranslate_prompt::estimated_tokens;

fn required(name: &str) -> String {
    std::env::var(format!("XRT_AGENT_LIVE_{name}"))
        .expect("explicit live-test configuration required")
}

#[test]
#[ignore = "Uses configured credentials and paid/limited inference; explicit opt-in only"]
fn configured_model_reviews_media_with_local_history() {
    let root = PathBuf::from(required("ROOT"));
    let provider = required("PROVIDER");
    let output = PathBuf::from(required("OUTPUT"));
    fs::create_dir_all(&output).unwrap();
    let mut trace = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output.join("trace.jsonl"))
        .unwrap();
    let mut record = |event: Value| {
        serde_json::to_writer(&mut trace, &event).unwrap();
        writeln!(trace).unwrap();
        trace.flush().unwrap();
    };
    let mut raw = load_user_config_document(root.join("config.json"), &root).unwrap();
    assert!(
        raw["translation"]["providers"][&provider].is_object(),
        "Use a configured provider ID"
    );
    raw["translation"]["provider"] = json!(provider);
    if let Ok(model) = std::env::var("XRT_AGENT_LIVE_MODEL") {
        raw["translation"]["providers"][&provider]["model"] = json!(model);
    }
    if let Some(key) = ["translation", "asr", "tts"]
        .into_iter()
        .find_map(|section| {
            raw[section]["providers"][&provider]["api_key"]
                .as_str()
                .filter(|key| !key.is_empty())
                .map(str::to_owned)
        })
    {
        raw["translation"]["providers"][&provider]["api_key"] = json!(key);
    }
    let route = AppConfig::from_value(raw)
        .unwrap()
        .native_model_route()
        .unwrap()
        .translation;
    assert!(
        !route.uses_local_runtime(),
        "Live validation must not start or contact local models"
    );
    let adapter: TranslationAdapter<ReqwestClient> =
        if route.connector == Some(xrtranslate_assets::remote::RemoteConnector::ChatgptResponses) {
            TranslationAdapter::with_chatgpt_account(
                AccountStore::for_project_root(&root),
                &route.url,
                &route.model,
            )
            .unwrap()
        } else {
            TranslationAdapter::with_bearer_token(
                ReqwestClient::new(Duration::from_secs(180)).unwrap(),
                &route.url,
                &route.model,
                route.translation_prompt_profile().unwrap(),
                route.api_key.as_deref().unwrap(),
            )
            .unwrap()
        }
        .with_remote_config(route.connector, &route.capabilities);
    let budget: usize = std::env::var("XRT_AGENT_LIVE_BUDGET")
        .unwrap_or_else(|_| {
            route
                .runtime
                .context_window_tokens
                .saturating_sub(route.runtime.max_tokens)
                .saturating_sub(256)
                .to_string()
        })
        .parse()
        .unwrap();
    let interval = Duration::from_secs(
        std::env::var("XRT_AGENT_LIVE_INTERVAL")
            .unwrap_or_else(|_| "0".into())
            .parse()
            .unwrap(),
    );
    record(
        json!({"event":"configuration","provider":provider,"model":route.model,"input_budget":budget,"max_output_tokens":route.runtime.max_tokens,"declared_ask":route.supports_ask().unwrap(),"declared_chat":route.supports_chat().unwrap(),"json_output":route.capabilities.supports_json_output,"interval_seconds":interval.as_secs()}),
    );
    eprintln!(
        "Live media review: {provider} / {}, budget {budget}",
        route.model
    );

    let mut task: MediaTask =
        serde_json::from_slice(&fs::read(required("FIXTURE")).unwrap()).unwrap();
    task.optimization = None;
    task.is_task_running = false;
    let original = task.subtitles.clone();
    let id = task.id.clone();
    let mut media = MediaPlugin::new();
    media.controller.backend = None;
    media.controller.storage_dir = output.join("media");
    media.controller.store.tasks.clear();
    media.controller.store.add_or_update(task);
    media.controller.open_task(&id).unwrap();
    let (scope, mut goal) = media.prepare_optimization(&id).unwrap();
    // This fixture compares translation quality and source-preserving splits.
    // General media editing also permits source corrections and cue deletion.
    goal.push_str("\nFor this validation, preserve all source words and the covered time span. Do not delete cues or revise original text; exact source-preserving splits are allowed.");
    let mut agent = AgentPlugin::load(output.join("agent"));
    // Explicitly probe candidate models before granting production eligibility.
    // The isolated run never edits the catalogue or the user's configuration.
    agent.configure(true, budget, media.tools(), "中文");
    agent.start(scope.clone(), goal).unwrap();
    let check = ToolCall {
        name: "media.check".into(),
        arguments: scope,
    };
    let before = media.call(&check).unwrap();
    record(json!({"event":"before","check":before,"subtitles":original}));

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut previous: Vec<PromptMessage> = Vec::new();
    let mut expected_appended = 0;
    let mut rounds = 0;
    let mut compactions = 0;
    let mut commits = 0;
    while agent.active() {
        let latest = agent.context.messages.last().cloned();
        let action = agent.poll(|messages| {
            rounds += 1;
            let compacted = !previous.is_empty() && !messages.starts_with(&previous);
            if !previous.is_empty() {
                assert_eq!(messages[..1], previous[..1], "fixed prefix changed");
                if compacted {
                    assert!(messages[1].content.starts_with("Progress note from earlier steps:\n"));
                    assert!(messages.len() <= previous.len() + 2);
                    assert_eq!(messages.last(), latest.as_ref(), "latest feedback lost");
                    compactions += 1;
                } else {
                    assert_eq!(messages.len(), previous.len() + expected_appended, "history duplicated or lost");
                }
            }
            let tokens: usize = messages.iter().map(|message| estimated_tokens(&message.content) + 32).sum();
            assert!(tokens <= budget);
            record(json!({"event":"request","round":rounds,"compacted":compacted,"estimated_tokens":tokens,"messages":messages}));
            previous = messages.clone();
            if rounds > 1 { std::thread::sleep(interval); }
            let started = Instant::now();
            let completion = runtime.block_on(adapter.ask(&messages, route.runtime.context_window_tokens, route.runtime.max_tokens, Some(&AgentPlugin::response_schema())));
            let reply = match completion {
                Ok(completion) => {
                    expected_appended = 2;
                    eprintln!("Round {rounds}: {} messages, estimated {tokens} tokens, compacted={compacted}, usage={:?}", messages.len(), completion.usage);
                    record(json!({"event":"response","round":rounds,"elapsed_ms":started.elapsed().as_millis(),"usage":completion.usage,"text":completion.text}));
                    Ok(completion.text)
                }
                Err(error) => {
                    let invalid_output = error.is_rejected_output();
                    let retryable = error.is_retryable();
                    expected_appended = usize::from(invalid_output);
                    let mut error = error.to_string();
                    if let Some(key) = &route.api_key { error = error.replace(key, "[redacted]"); }
                    eprintln!("Round {rounds}: {error}");
                    record(json!({"event":"error","round":rounds,"error":error,"invalid_output":invalid_output,"retryable":retryable}));
                    Err(xrtranslate_protocol::AskError {error, invalid_output, retryable})
                }
            };
            let (sender, receiver) = crossbeam_channel::bounded(1);
            sender.send(reply).unwrap();
            receiver
        });
        if let Some(Action::Call(call)) = action {
            let result = media.call(&call);
            eprintln!(
                "Tool {}: {}",
                call.name,
                if result.is_ok() { "ok" } else { "error" }
            );
            if call.name == "media.commit" && result.is_ok() {
                commits += 1;
            }
            record(json!({"event":"tool","call":call,"result":result}));
            agent.observe(&call, result);
        }
        if let Some(message) = agent.take_message() {
            record(json!({"event":"message","text":message}));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let after = media.call(&check).unwrap();
    let final_task = media.controller.store.get(&id).unwrap();
    record(
        json!({"event":"finished","rounds":rounds,"compactions":compactions,"commits":commits,"check":after,"agent":agent.last,"subtitles":final_task.subtitles}),
    );
    fs::write(
        output.join("subtitles.srt"),
        final_task.subtitles.export_srt(),
    )
    .unwrap();
    eprintln!(
        "Result: rounds={rounds}, compactions={compactions}, commits={commits}, issues {} -> {}",
        before["count"], after["count"]
    );
    assert_eq!(
        agent.last.as_ref().unwrap()["completed"],
        true,
        "Agent stopped without a valid final decision; see trace.jsonl"
    );
    assert!(
        commits > 0,
        "Agent did not publish a result; see trace.jsonl"
    );
    assert!(rounds >= 3, "Review must exercise multiple tool turns");
    assert_eq!(after["count"], 0, "Unresolved subtitle issues");
    let source = |timeline: &crate::plugins::media::subtitles::SubtitleTimeline| {
        timeline
            .cues()
            .iter()
            .flat_map(|cue| cue.original_text.chars())
            .filter(|ch| !ch.is_whitespace())
            .collect::<String>()
    };
    assert_eq!(source(&original), source(&final_task.subtitles));
    assert_eq!(
        original.cues().first().unwrap().start_ms,
        final_task.subtitles.cues().first().unwrap().start_ms
    );
    assert_eq!(
        original.cues().last().unwrap().end_ms,
        final_task.subtitles.cues().last().unwrap().end_ms
    );
}
