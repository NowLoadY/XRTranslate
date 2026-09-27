use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use xrtranslate_assets::{ModelAssetId, ModelCapability, manifest_for};
use xrtranslate_config::{AppConfig, RuntimeLayout};
use xrtranslate_engine::TranslationSegmentPair;
use xrtranslate_inference::pcm16_mono_16khz_to_wav;
use xrtranslate_prompt::{AsrPromptContext, PromptNodeGraph, TranslationPromptContext};

use crate::{
    Arguments,
    language::{AdaptiveLanguageRoute, SupportedLanguage},
    model_runtime::{NativeProviderPlan, initialize_managed_onnx_runtime},
    pipeline::{NativeInference, TranslationOutput},
    tts_session::restore_persisted_voice_clones,
};

#[derive(Debug, clap::Args)]
pub(crate) struct Options {
    /// Select installed packages for this run without saving configuration.
    #[arg(long, value_parser = parse_model)]
    asr_model: Option<ModelAssetId>,
    #[arg(long, value_parser = parse_model)]
    translation_model: Option<ModelAssetId>,
    #[arg(long, value_parser = parse_model)]
    tts_model: Option<ModelAssetId>,
    #[arg(long)]
    text: String,
    #[arg(long, default_value = "zh")]
    source: String,
    #[arg(long, default_value = "en")]
    target: String,
    #[command(flatten)]
    settings: Settings,
    /// Results directory; previous diagnostic artifacts are cleared before running.
    #[arg(long)]
    output: PathBuf,
}

#[derive(Debug, clap::Args)]
pub(super) struct Settings {
    /// Existing voice name, or the name to register from --reference-wav.
    #[arg(long, default_value = crate::tts_session::MICROPHONE_VOICE_NAME)]
    pub voice: String,
    #[arg(long)]
    pub reference_wav: Option<PathBuf>,
    #[arg(long, requires = "reference_wav")]
    pub reference_text: Option<String>,
    /// Deadline for the complete run, including model startup.
    #[arg(long, default_value_t = 600, value_parser = clap::value_parser!(u64).range(1..))]
    pub timeout_seconds: u64,
}

pub(crate) async fn run(
    args: &Arguments,
    options: &Options,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut inputs = vec![args.config.as_path()];
    inputs.extend(options.settings.reference_wav.as_deref());
    super::results::prepare(&options.output, &inputs)?;
    let mut report = json!({"status": "running", "stage": "setup", "input": options.text,
        "source": options.source, "target": options.target, "stages": []});
    let started = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(options.settings.timeout_seconds),
        execute(args, options, &mut report),
    )
    .await;
    let error = match result {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(error),
        Err(_) => Some(format!(
            "run exceeded {} seconds",
            options.settings.timeout_seconds
        )),
    };
    report["status"] = json!(if error.is_some() { "failed" } else { "passed" });
    report["elapsed_ms"] = json!(started.elapsed().as_millis());
    report["error"] = json!(error);
    let serialized = serde_json::to_string_pretty(&report)?;
    std::fs::write(options.output.join("report.json"), &serialized)?;
    println!("{serialized}");
    error.map_or(Ok(()), |error| Err(error.into()))
}

async fn execute(args: &Arguments, options: &Options, report: &mut Value) -> Result<(), String> {
    if options.text.trim().is_empty() {
        return Err("test text must not be empty".into());
    }
    let source = SupportedLanguage::parse(&options.source)
        .ok_or("source must be one explicit language")?
        .code();
    let target = SupportedLanguage::parse(&options.target)
        .ok_or("target must be one explicit language")?
        .code();
    if source == target {
        return Err("source and target must differ to exercise translation".into());
    }
    let (root, config) = super::load_config(args)?;
    if (options.asr_model.is_some() || options.translation_model.is_some())
        && !args.manage_llama_servers
    {
        return Err("model overrides require --manage-llama-servers to ensure the selected weights are loaded".into());
    }
    let selections = [
        (options.asr_model, ModelCapability::Asr),
        (options.translation_model, ModelCapability::Translation),
        (options.tts_model, ModelCapability::Tts),
    ];
    let mut ids = Vec::new();
    for (id, capability) in selections {
        if let Some(id) = id {
            if manifest_for(id).capability != capability {
                return Err(format!("{id} is not a {capability:?} model"));
            }
            ids.push(id);
        }
    }
    let config: AppConfig = config.with_model_assets(&ids)?;
    initialize_managed_onnx_runtime(&root, &config)?;
    let mut plan = NativeProviderPlan::resolve(&config, &root)?;
    report["models"] = json!({"asr": plan.asr_model_alias(), "translation": plan.translation_model_alias(), "tts_provider": config.tts.provider, "assets": config.active_native_model_assets()});
    plan.check_assets()?;
    plan.language_capabilities
        .for_text()
        .select(source, target)?;
    plan.language_capabilities.select(target, source)?;
    if args.manage_llama_servers {
        crate::assign_managed_ports(&mut plan)?;
    }
    report["stage"] = json!("model_startup");
    let _processes = if args.manage_llama_servers {
        let mut processes = crate::start_llama_servers(&plan).map_err(|e| e.to_string())?;
        crate::wait_for_model_servers(&plan, args.model_start_timeout_seconds, &mut processes)
            .await
            .map_err(|e| e.to_string())?;
        Some(processes)
    } else {
        None
    };
    let inference = NativeInference::for_diagnostics(&plan)?;
    report["stage"] = json!("voice_setup");
    let tts = plan.tts_adapter(&config)?.ok_or("TTS is not configured")?;
    tts.prepare().await.map_err(|e| e.to_string())?;
    if !tts.supports_language(target) {
        return Err(format!("TTS does not support {target}"));
    }
    if let Some(path) = &options.settings.reference_wav {
        tts.register_voice(
            &options.settings.voice,
            std::fs::read(path).map_err(|e| e.to_string())?,
            options.settings.reference_text.as_deref().unwrap_or(""),
        )
        .await
        .map_err(|e| e.to_string())?;
    } else {
        restore_persisted_voice_clones(
            &RuntimeLayout::for_config(&root, &config.model_manager).voice_clones_directory(),
            &tts,
        )
        .await;
    }
    if !tts.has_voice(&options.settings.voice).await {
        return Err("voice unavailable; supply --reference-wav and its --reference-text, or an existing --voice".into());
    }

    report["stage"] = json!("translation");
    let start = Instant::now();
    let translated = translate(&inference, &options.text, source, target).await?;
    record(
        report,
        start,
        json!({"text": translated.translated_text, "prompt_trace": translated.prompt_trace}),
    );

    report["stage"] = json!("tts");
    let start = Instant::now();
    let mut samples = Vec::new();
    for chunk in crate::tts_session::split_text(
        &translated.translated_text,
        crate::tts_session::max_input_chars(&config),
    ) {
        let audio = tts
            .synthesize(&chunk, &options.settings.voice, target)
            .await
            .map_err(|e| e.to_string())?;
        samples.extend(audio.resample_pcm16(16_000).map_err(|e| e.to_string())?);
    }
    if samples.is_empty() || samples.iter().all(|s| *s == 0) {
        return Err("TTS produced empty or silent audio".into());
    }
    let pcm: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    std::fs::write(
        options.output.join("speech.wav"),
        pcm16_mono_16khz_to_wav(&pcm).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    record(
        report,
        start,
        json!({"audio": "speech.wav", "sample_rate": 16000, "duration_ms": samples.len() * 1000 / 16000}),
    );

    report["stage"] = json!("asr");
    let start = Instant::now();
    let recognized = inference
        .transcribe(
            &samples,
            target,
            source,
            &mut AdaptiveLanguageRoute::default(),
            &PromptNodeGraph::builtin_default(),
            AsrPromptContext::default(),
            &[],
        )
        .await
        .map_err(|e| e.message)?
        .ok_or("ASR returned no accepted speech")?;
    record(
        report,
        start,
        json!({"text": recognized.source_text, "prompt_trace": recognized.prompt_trace}),
    );

    report["stage"] = json!("back_translation");
    let start = Instant::now();
    let back = translate(&inference, &recognized.source_text, target, source).await?;
    record(
        report,
        start,
        json!({"text": back.translated_text, "prompt_trace": back.prompt_trace}),
    );
    Ok(())
}

async fn translate(
    inference: &NativeInference,
    text: &str,
    source: &str,
    target: &str,
) -> Result<TranslationOutput, String> {
    let segment = TranslationSegmentPair {
        source_text: text.into(),
        translation_text: text.into(),
    };
    let result = inference
        .translate_segment(
            &segment,
            source,
            target,
            PromptNodeGraph::builtin_default(),
            TranslationPromptContext::default(),
        )
        .await
        .map_err(|e| e.message)?;
    if result.translated_text.trim().is_empty() {
        return Err("translation returned empty text".into());
    }
    Ok(result)
}

fn record(report: &mut Value, start: Instant, mut result: Value) {
    result["stage"] = report["stage"].clone();
    result["elapsed_ms"] = json!(start.elapsed().as_millis());
    eprintln!("roundtrip: {} completed", report["stage"]);
    report["stages"].as_array_mut().unwrap().push(result);
}

fn parse_model(key: &str) -> Result<ModelAssetId, String> {
    ModelAssetId::from_config_key(key).ok_or_else(|| format!("unknown model asset: {key}"))
}
