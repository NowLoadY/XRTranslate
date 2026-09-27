//! Enumerate language-compatible installed packages and invoke the single-run
//! diagnostic in isolated processes so ONNX/GPU state cannot leak between cases.

use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    process::{Command, Stdio},
};

use serde_json::{Value, json};
use xrtranslate_assets::ModelCapability;
use xrtranslate_engine::language::SupportedLanguage;

use super::roundtrip::Settings;
use crate::Arguments;

#[derive(Debug, clap::Args)]
pub(crate) struct Options {
    /// JSON object mapping source language codes to test text in that language.
    #[arg(long)]
    inputs: PathBuf,
    /// Restrict target languages; omitted means every compatible target language.
    #[arg(long, value_delimiter = ',')]
    target: Vec<String>,
    #[arg(long)]
    output: PathBuf,
    /// Save the inventory and complete case list without loading any model.
    #[arg(long)]
    plan_only: bool,
    #[command(flatten)]
    settings: Settings,
}

pub(crate) fn run(args: &Arguments, options: &Options) -> Result<(), Box<dyn std::error::Error>> {
    let (root, config) = super::load_config(args)?;
    let models = config.installed_models(&root);
    let inputs: BTreeMap<String, String> = serde_json::from_slice(&fs::read(&options.inputs)?)?;
    if inputs.is_empty() {
        return Err("inputs must contain at least one language and text".into());
    }
    let mut sources = BTreeMap::new();
    for (code, text) in inputs {
        let source = language(&code)?;
        if text.trim().is_empty() {
            return Err(format!("empty input for {code}").into());
        }
        if sources.insert(source.code(), (source, text)).is_some() {
            return Err(format!("duplicate source language: {code}").into());
        }
    }
    let targets = options
        .target
        .iter()
        .map(|code| language(code))
        .collect::<Result<Vec<_>, _>>()?;
    let mut cases = Vec::new();
    for asr in models
        .iter()
        .filter(|m| m.capability == ModelCapability::Asr)
    {
        for mt in models
            .iter()
            .filter(|m| m.capability == ModelCapability::Translation)
        {
            for tts in models
                .iter()
                .filter(|m| m.capability == ModelCapability::Tts)
            {
                for (source, text) in sources.values() {
                    if !mt.supports_language(*source) {
                        continue;
                    }
                    for target in
                        xrtranslate_engine::language::LanguageSet::from_codes(mt.languages, false)
                            .iter()
                    {
                        if source.base_code() == target.base_code()
                            || !asr.supports_language(target)
                            || !tts.supports_language(target)
                            || (!targets.is_empty() && !targets.contains(&target))
                        {
                            continue;
                        }
                        cases.push(json!({"asr": asr.id, "translation": mt.id, "tts": tts.id,
                            "source": source.code(), "target": target.code(), "text": text,
                            "directory": format!("case-{:04}", cases.len() + 1), "status": "pending"}));
                    }
                }
            }
        }
    }
    let mut protected = vec![args.config.as_path(), options.inputs.as_path()];
    protected.extend(options.settings.reference_wav.as_deref());
    super::results::prepare(&options.output, &protected)?;
    let mut summary = json!({"status": "planned", "models": models,
        "total": cases.len(), "passed": 0, "failed": 0, "cases": cases});
    let mut failures = Vec::new();
    save(options, &summary, &failures)?;
    if summary["total"] == 0 {
        summary["status"] = json!("failed");
        summary["error"] = json!("no compatible installed model combinations");
        save(options, &summary, &failures)?;
        return Err("no compatible installed model combinations; see matrix.json".into());
    }
    if options.plan_only {
        print_summary(options, &summary);
        return Ok(());
    }
    summary["status"] = json!("running");
    for index in 0..summary["cases"].as_array().unwrap().len() {
        let case = &summary["cases"][index];
        eprintln!("roundtrip matrix: case {}/{}", index + 1, summary["total"]);
        let result = run_case(args, options, case);
        let (passed, error) = match result {
            Ok(()) => (true, None),
            Err(error) => (false, Some(error.to_string())),
        };
        let counter = if passed { "passed" } else { "failed" };
        summary[counter] = json!(summary[counter].as_u64().unwrap() + 1);
        summary["cases"][index]["status"] = json!(if passed { "passed" } else { "failed" });
        summary["cases"][index]["error"] = json!(error);
        if !passed {
            failures.push(super::results::failure(
                &options.output,
                &summary["cases"][index],
            ));
        }
        save(options, &summary, &failures)?;
    }
    let passed = summary["failed"] == 0;
    summary["status"] = json!(if passed { "passed" } else { "failed" });
    save(options, &summary, &failures)?;
    print_summary(options, &summary);
    if passed {
        Ok(())
    } else {
        Err("one or more roundtrip cases failed; see matrix.json and case logs".into())
    }
}

fn run_case(
    args: &Arguments,
    options: &Options,
    case: &Value,
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = options.output.join(case["directory"].as_str().unwrap());
    let log = fs::File::create(directory.with_extension("log"))?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("--config")
        .arg(&args.config)
        .arg("--manage-llama-servers")
        .arg("--model-start-timeout-seconds")
        .arg(args.model_start_timeout_seconds.to_string())
        .arg("roundtrip")
        .arg("--output")
        .arg(&directory)
        .arg("--voice")
        .arg(&options.settings.voice)
        .arg("--timeout-seconds")
        .arg(options.settings.timeout_seconds.to_string());
    for (flag, key) in [
        ("--asr-model", "asr"),
        ("--translation-model", "translation"),
        ("--tts-model", "tts"),
        ("--source", "source"),
        ("--target", "target"),
        ("--text", "text"),
    ] {
        command.arg(flag).arg(case[key].as_str().unwrap());
    }
    if let Some(path) = &options.settings.reference_wav {
        command.arg("--reference-wav").arg(path);
    }
    if let Some(text) = &options.settings.reference_text {
        command.arg("--reference-text").arg(text);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let argv = std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    fs::write(
        directory.with_extension("command.json"),
        serde_json::to_vec_pretty(&argv)?,
    )?;
    let status = command
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .status()?;
    let bytes = fs::read(directory.join("report.json"))
        .map_err(|error| format!("{status}; report unavailable: {error}"))?;
    let report: Value = serde_json::from_slice(&bytes)?;
    if status.success() && report["status"] == "passed" {
        return Ok(());
    }
    Err(format!(
        "{}: {} ({status})",
        report["stage"].as_str().unwrap_or("unknown"),
        report["error"].as_str().unwrap_or("failed")
    )
    .into())
}

fn language(code: &str) -> Result<SupportedLanguage, String> {
    SupportedLanguage::parse(code)
        .ok_or_else(|| format!("expected one explicit language, got {code:?}"))
}

fn save(options: &Options, summary: &Value, failures: &[Value]) -> Result<(), std::io::Error> {
    fs::write(
        options.output.join("matrix.json"),
        serde_json::to_vec_pretty(summary)?,
    )?;
    super::results::write_failures(&options.output, summary, failures)
}

fn print_summary(options: &Options, summary: &Value) {
    println!(
        "{}",
        json!({"status": summary["status"], "total": summary["total"],
        "passed": summary["passed"], "failed": summary["failed"],
        "matrix": options.output.join("matrix.json"), "failures": options.output.join("failures.json")})
    );
}
