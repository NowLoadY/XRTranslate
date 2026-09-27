//! Ownership checks, cleanup and focused failure reports for diagnostic outputs.

use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

const MARKER: &str = ".xrtranslate-diagnostics";

/// Delete only recognized diagnostic artifacts, after validating the entire
/// directory. Inputs and unrelated files must never be removed by a rerun.
pub(super) fn prepare(
    directory: &Path,
    inputs: &[&Path],
) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(directory)?;
    let root = fs::canonicalize(directory)?;
    if fs::canonicalize(std::env::current_dir()?)?.starts_with(&root)
        || inputs
            .iter()
            .filter_map(|path| fs::canonicalize(path).ok())
            .any(|path| path.starts_with(&root))
    {
        return Err("diagnostic output must not contain the workspace or an input file".into());
    }
    let mut entries = Vec::new();
    collect_results(directory, &root, &mut entries)?;
    for path in entries {
        if path.is_dir() {
            fs::remove_dir(path)?;
        } else {
            fs::remove_file(path)?;
        }
    }
    fs::write(directory.join(MARKER), "XRTranslate diagnostics\n")?;
    Ok(())
}

fn collect_results(
    directory: &Path,
    root: &Path,
    entries: &mut Vec<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let metadata = fs::symlink_metadata(directory)?;
    #[cfg(windows)]
    let linked = {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0 // reparse points, including junctions
    };
    #[cfg(not(windows))]
    let linked = metadata.file_type().is_symlink();
    if linked || !fs::canonicalize(directory)?.starts_with(root) {
        return Err(format!(
            "refusing cleanup through linked directory {}",
            directory.display()
        )
        .into());
    }
    let children = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    let owned = directory.join(MARKER).is_file()
        || ["report.json", "matrix.json"].iter().any(|name| {
            fs::read(directory.join(name))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .is_some_and(|value| value["stages"].is_array() || value["cases"].is_array())
        });
    if !children.is_empty() && !owned {
        return Err(format!(
            "{} is not a diagnostic results directory",
            directory.display()
        )
        .into());
    }
    for entry in children {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if entry.file_type()?.is_dir() && case_name(&name) {
            collect_results(&path, root, entries)?;
        } else if !entry.file_type()?.is_file()
            || !(matches!(
                name.as_ref(),
                MARKER
                    | "report.json"
                    | "matrix.json"
                    | "speech.wav"
                    | "failures.json"
                    | "failures.md"
                    | "command.json"
            ) || name.strip_suffix(".log").is_some_and(case_name)
                || name.strip_suffix(".command.json").is_some_and(case_name))
        {
            return Err(format!(
                "refusing to delete unrelated result entry {}",
                path.display()
            )
            .into());
        }
        entries.push(path);
    }
    Ok(())
}

fn case_name(name: &str) -> bool {
    name.strip_prefix("case-").is_some_and(|suffix| {
        !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
    })
}

pub(super) fn failure(directory: &Path, case: &Value) -> Value {
    let case_dir = directory.join(case["directory"].as_str().unwrap());
    let report = fs::read(case_dir.join("report.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    let command = fs::read(case_dir.with_extension("command.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    let log = case_dir.with_extension("log");
    json!({"combination": case, "report": report, "command": command,
        "log": log, "log_tail": log_tail(&log).unwrap_or_else(|error| error.to_string())})
}

fn log_tail(path: &Path) -> Result<String, std::io::Error> {
    let mut file = fs::File::open(path)?;
    let offset = file.metadata()?.len().saturating_sub(16 * 1024);
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub(super) fn write_failures(
    directory: &Path,
    summary: &Value,
    failures: &[Value],
) -> Result<(), std::io::Error> {
    let mut by_stage = BTreeMap::<&str, usize>::new();
    for failure in failures {
        *by_stage
            .entry(failure["report"]["stage"].as_str().unwrap_or("process"))
            .or_default() += 1;
    }
    let report = json!({"status": summary["status"], "total": summary["total"], "passed": summary["passed"],
        "failed": summary["failed"], "by_stage": by_stage, "error": summary["error"], "failures": failures});
    fs::write(
        directory.join("failures.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    let mut markdown = format!(
        "# 回环异常报告\n\n状态：{}；组合：{}；通过：{}；失败：{}。\n",
        summary["status"], summary["total"], summary["passed"], summary["failed"]
    );
    if let Some(error) = summary["error"].as_str() {
        markdown.push_str(&format!("\n{error}\n"));
    }
    for (stage, count) in by_stage {
        markdown.push_str(&format!("\n- {stage}: {count}\n"));
    }
    for failure in failures {
        let case = &failure["combination"];
        let name = case["directory"].as_str().unwrap_or("unknown");
        markdown.push_str(&format!("\n## {name} · {} → {}\n\nASR：{}；翻译：{}；TTS：{}\n\n错误：{}\n\n[阶段及提示词完整记录]({name}/report.json) · [完整日志]({name}.log)\n\n原文：\n\n````text\n{}\n````\n",
            case["source"], case["target"], case["asr"], case["translation"], case["tts"], case["error"], case["text"].as_str().unwrap_or("")));
        if let Some(stages) = failure["report"]["stages"].as_array() {
            for stage in stages {
                markdown.push_str(&format!(
                    "\n### {} · {} ms\n\n````text\n{}\n````\n",
                    stage["stage"],
                    stage["elapsed_ms"],
                    stage["text"]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| stage.to_string())
                ));
            }
        }
        markdown.push_str(&format!("\n复现参数（程序和参数数组）：\n\n````json\n{}\n````\n\n日志末尾（最多 16 KiB）：\n\n````text\n{}\n````\n",
            serde_json::to_string_pretty(&failure["command"])?, failure["log_tail"].as_str().unwrap_or("")));
    }
    fs::write(directory.join("failures.md"), markdown)
}
