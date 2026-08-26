#![forbid(unsafe_code)]

use std::{
    collections::HashSet,
    error::Error,
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};

use clap::Parser;

const PROTECTED_TOP_LEVEL: &[&str] = &["runtime"];
const RETRY_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Debug, Parser)]
#[command(
    name = "xrtranslate-updater",
    version,
    about = "Apply an XRTranslate update"
)]
struct Arguments {
    #[arg(long)]
    source: PathBuf,
    #[arg(long)]
    target: PathBuf,
    #[arg(long)]
    current_pid: u32,
    #[arg(long)]
    restart: bool,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Arguments::parse();
    wait_for_app_exit(args.current_pid);
    apply_update(&args.source, &args.target)?;
    if args.restart {
        restart_app(&args.source, &args.target)?;
    }
    Ok(())
}

fn wait_for_app_exit(current_pid: u32) {
    let started = Instant::now();
    while started.elapsed() < RETRY_TIMEOUT {
        if !process_exists(current_pid) {
            return;
        }
        thread::sleep(Duration::from_millis(250));
    }
}

fn process_exists(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        Path::new("/proc").join(pid.to_string()).exists()
    }
    #[cfg(target_os = "windows")]
    {
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .is_some_and(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .split_whitespace()
                    .any(|part| part == pid.to_string())
            })
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let _ = pid;
        false
    }
}

fn apply_update(source: &Path, target: &Path) -> Result<(), Box<dyn Error>> {
    require_directory(source, "source")?;
    require_directory(target, "target")?;
    let source_entries = source_entries(source)?;
    let backup = target
        .join("runtime")
        .join("updates")
        .join(format!("backup-{}", std::process::id()));
    if backup.exists() {
        fs::remove_dir_all(&backup)?;
    }
    fs::create_dir_all(&backup)?;

    let result = replace_entries(source, target, &source_entries, &backup);
    if let Err(error) = result {
        let _ = restore_backup(target, &backup);
        return Err(error.into());
    }
    reset_app_state_first_run(target)?;
    let _ = fs::remove_dir_all(&backup);
    Ok(())
}

fn reset_app_state_first_run(target: &Path) -> Result<(), Box<dyn Error>> {
    let runtime_dir = target.join("runtime");
    if !runtime_dir.exists() {
        fs::create_dir_all(&runtime_dir)?;
    }

    let app_state_path = runtime_dir.join("app_state.json");
    let mut app_state: serde_json::Map<String, serde_json::Value> = if app_state_path.is_file() {
        fs::read_to_string(&app_state_path)
            .ok()
            .and_then(|contents| serde_json::from_str(&contents).ok())
            .unwrap_or_default()
    } else {
        serde_json::Map::new()
    };
    app_state.insert("first_run".to_string(), serde_json::Value::Bool(true));
    if let Ok(serialized) = serde_json::to_string_pretty(&app_state) {
        let _ = fs::write(&app_state_path, serialized);
    }

    let settings_path = runtime_dir.join("rust-client-settings.json");
    if settings_path.is_file() {
        if let Ok(contents) = fs::read_to_string(&settings_path) {
            if let Ok(mut settings_val) = serde_json::from_str::<serde_json::Value>(&contents) {
                if let Some(obj) = settings_val.as_object_mut() {
                    obj.insert("first_run".to_string(), serde_json::Value::Bool(true));
                    if let Ok(serialized) = serde_json::to_string_pretty(&settings_val) {
                        let _ = fs::write(&settings_path, serialized);
                    }
                }
            }
        }
    }

    Ok(())
}

fn source_entries(source: &Path) -> Result<HashSet<String>, io::Error> {
    let mut entries = HashSet::new();
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        if let Some(name) = entry.file_name().to_str() {
            entries.insert(name.to_owned());
        }
    }
    Ok(entries)
}

fn replace_entries(
    source: &Path,
    target: &Path,
    source_entries: &HashSet<String>,
    backup: &Path,
) -> Result<(), String> {
    remove_old_client_binary(target, source_entries, backup)?;
    for entry in fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name();
        if name.eq_ignore_ascii_case(OsStr::new("models")) {
            merge_models_directory(&entry.path(), &target.join(&name), target, backup)?;
            continue;
        }
        if name.eq_ignore_ascii_case(OsStr::new("resources")) {
            replace_resources_directory(&entry.path(), &target.join(&name), target, backup)?;
            continue;
        }
        if is_protected(&name) {
            continue;
        }
        let destination = target.join(&name);
        backup_existing(&destination, target, backup)?;
        copy_path(&entry.path(), &destination)?;
    }
    Ok(())
}

/// Replaces packaged resources while preserving locally installed native
/// runtimes that the release intentionally does not carry.
///
/// Runtime installers place optional libraries under `resources/bin`. A new
/// release is authoritative for every file it contains; only dynamic libraries
/// absent from the release are migrated from the previous installation.
fn replace_resources_directory(
    source_resources: &Path,
    target_resources: &Path,
    target_root: &Path,
    backup: &Path,
) -> Result<(), String> {
    backup_existing(target_resources, target_root, backup)?;
    copy_path(source_resources, target_resources)?;

    let relative = target_resources
        .strip_prefix(target_root)
        .map_err(|error| {
            format!(
                "invalid resources path {}: {error}",
                target_resources.display()
            )
        })?;
    restore_local_runtime_libraries(
        &backup.join(relative).join("bin"),
        &source_resources.join("bin"),
        &target_resources.join("bin"),
    )
}

fn restore_local_runtime_libraries(
    previous: &Path,
    release: &Path,
    target: &Path,
) -> Result<(), String> {
    if !previous.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(previous).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let previous_path = entry.path();
        let release_path = release.join(entry.file_name());
        let target_path = target.join(entry.file_name());
        if previous_path.is_dir() {
            restore_local_runtime_libraries(&previous_path, &release_path, &target_path)?;
        } else if previous_path.is_file()
            && is_dynamic_runtime_library(&previous_path)
            && !release_path.exists()
        {
            copy_path(&previous_path, &target_path)?;
        }
    }
    Ok(())
}

fn is_dynamic_runtime_library(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    let name = name.to_ascii_lowercase();
    name.ends_with(".dll")
        || name.ends_with(".dylib")
        || name.ends_with(".so")
        || name.contains(".so.")
}

fn merge_models_directory(
    source_models: &Path,
    target_models: &Path,
    target_root: &Path,
    backup: &Path,
) -> Result<(), String> {
    if !source_models.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(target_models).map_err(|error| error.to_string())?;
    merge_directory_recursive(source_models, target_models, target_root, backup)
}

fn merge_directory_recursive(
    source_dir: &Path,
    target_dir: &Path,
    target_root: &Path,
    backup: &Path,
) -> Result<(), String> {
    for entry in fs::read_dir(source_dir).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let src_path = entry.path();
        let dest_path = target_dir.join(entry.file_name());
        if src_path.is_dir() {
            fs::create_dir_all(&dest_path).map_err(|error| error.to_string())?;
            merge_directory_recursive(&src_path, &dest_path, target_root, backup)?;
        } else if src_path.is_file() {
            if dest_path.exists() {
                backup_existing(&dest_path, target_root, backup)?;
            }
            copy_path(&src_path, &dest_path)?;
        }
    }
    Ok(())
}

fn remove_old_client_binary(
    target: &Path,
    source_entries: &HashSet<String>,
    backup: &Path,
) -> Result<(), String> {
    let entries = fs::read_dir(target).map_err(|error| error.to_string())?;
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = entry.file_name();
        let Some(name_text) = name.to_str() else {
            continue;
        };
        if source_entries.contains(name_text) {
            continue;
        }
        let lower = name_text.to_ascii_lowercase();
        let is_client = if cfg!(target_os = "windows") {
            lower.starts_with("xrtranslate") && lower.ends_with(".exe")
        } else {
            lower.starts_with("xrtranslate")
        };
        if is_client {
            backup_existing(&path, target, backup)?;
        }
    }
    Ok(())
}

fn backup_existing(path: &Path, target: &Path, backup: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let relative = path
        .strip_prefix(target)
        .map_err(|error| format!("invalid update path {}: {error}", path.display()))?;
    let backup_path = backup.join(relative);
    if let Some(parent) = backup_path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    retry_io(|| fs::rename(path, &backup_path))
        .map_err(|error| format!("cannot replace {}: {error}", path.display()))
}

fn restore_backup(target: &Path, backup: &Path) -> Result<(), String> {
    if !backup.exists() {
        return Ok(());
    }
    restore_entries(backup, backup, target)
}

fn restore_entries(root: &Path, directory: &Path, target: &Path) -> Result<(), String> {
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if path.is_dir() {
            restore_entries(root, &path, target)?;
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|error| format!("invalid backup path: {error}"))?;
        let destination = target.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let _ = fs::remove_file(&destination);
        fs::rename(&path, &destination).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn copy_path(source: &Path, destination: &Path) -> Result<(), String> {
    if source.is_dir() {
        fs::create_dir_all(destination).map_err(|error| error.to_string())?;
        for entry in fs::read_dir(source).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            copy_path(&entry.path(), &destination.join(entry.file_name()))?;
        }
        return Ok(());
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    retry_io(|| fs::copy(source, destination))
        .map(|_| ())
        .map_err(|error| format!("cannot copy {}: {error}", source.display()))
}

fn retry_io<T>(mut action: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let started = Instant::now();
    loop {
        match action() {
            Ok(value) => return Ok(value),
            Err(error) if started.elapsed() < RETRY_TIMEOUT => {
                thread::sleep(Duration::from_millis(250));
                if error.kind() == io::ErrorKind::NotFound {
                    return Err(error);
                }
            }
            Err(error) => return Err(error),
        }
    }
}

fn restart_app(source: &Path, target: &Path) -> Result<(), Box<dyn Error>> {
    let manifest_path = source.join("release-manifest.json");
    let manifest: serde_json::Value = serde_json::from_str(&fs::read_to_string(manifest_path)?)?;
    let Some(entrypoint) = manifest
        .pointer("/entrypoints/client")
        .and_then(serde_json::Value::as_str)
    else {
        return Ok(());
    };
    let executable = target.join(entrypoint);
    if executable.is_file() {
        Command::new(executable).current_dir(target).spawn()?;
    }
    Ok(())
}

fn is_protected(name: &OsStr) -> bool {
    PROTECTED_TOP_LEVEL
        .iter()
        .any(|protected| name.eq_ignore_ascii_case(OsStr::new(protected)))
}

fn require_directory(path: &Path, label: &str) -> Result<(), Box<dyn Error>> {
    if path.is_dir() {
        Ok(())
    } else {
        Err(format!("{label} directory does not exist: {}", path.display()).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_models_directory_adds_new_onnx_and_preserves_user_gguf() {
        let temp = std::env::temp_dir().join(format!("xrt_updater_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let source = temp.join("source");
        let target = temp.join("target");
        let backup = temp.join("backup");

        fs::create_dir_all(source.join("models/gtcrn")).unwrap();
        fs::create_dir_all(source.join("models/silero-vad")).unwrap();
        fs::write(
            source.join("models/gtcrn/gtcrn_simple.onnx"),
            b"new_denoise_onnx",
        )
        .unwrap();
        fs::write(
            source.join("models/silero-vad/silero_vad.onnx"),
            b"updated_vad_onnx",
        )
        .unwrap();

        fs::create_dir_all(target.join("models/silero-vad")).unwrap();
        fs::create_dir_all(target.join("models/qwen3-asr")).unwrap();
        fs::write(
            target.join("models/silero-vad/silero_vad.onnx"),
            b"old_vad_onnx",
        )
        .unwrap();
        fs::write(
            target.join("models/qwen3-asr/qwen3.gguf"),
            b"huge_user_downloaded_gguf",
        )
        .unwrap();

        let source_entries = source_entries(&source).unwrap();
        replace_entries(&source, &target, &source_entries, &backup).unwrap();

        // 1. New ONNX model is copied to target
        assert_eq!(
            fs::read(target.join("models/gtcrn/gtcrn_simple.onnx")).unwrap(),
            b"new_denoise_onnx"
        );
        // 2. Updated ONNX model is updated in target
        assert_eq!(
            fs::read(target.join("models/silero-vad/silero_vad.onnx")).unwrap(),
            b"updated_vad_onnx"
        );
        // 3. User's existing GGUF model is untouched and preserved
        assert_eq!(
            fs::read(target.join("models/qwen3-asr/qwen3.gguf")).unwrap(),
            b"huge_user_downloaded_gguf"
        );
        // 4. Old VAD model was backed up
        assert_eq!(
            fs::read(backup.join("models/silero-vad/silero_vad.onnx")).unwrap(),
            b"old_vad_onnx"
        );

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn replacing_resources_preserves_only_target_local_runtime_libraries() {
        let temp =
            std::env::temp_dir().join(format!("xrt_updater_resources_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let source = temp.join("source");
        let target = temp.join("target");
        let backup = temp.join("backup");

        fs::create_dir_all(source.join("resources/bin")).unwrap();
        fs::write(source.join("resources/theme.json"), b"new theme").unwrap();
        fs::write(source.join("resources/bin/release.dll"), b"new release dll").unwrap();

        fs::create_dir_all(target.join("resources/bin/codecs")).unwrap();
        fs::write(target.join("resources/theme.json"), b"old theme").unwrap();
        fs::write(target.join("resources/removed.txt"), b"stale packaged file").unwrap();
        fs::write(target.join("resources/bin/mpv-2.dll"), b"downloaded mpv").unwrap();
        fs::write(
            target.join("resources/bin/codecs/future-codec.dll"),
            b"downloaded future codec",
        )
        .unwrap();
        fs::write(target.join("resources/bin/release.dll"), b"old release dll").unwrap();
        fs::write(
            target.join("resources/bin/local-note.txt"),
            b"not a runtime",
        )
        .unwrap();

        let source_entries = source_entries(&source).unwrap();
        replace_entries(&source, &target, &source_entries, &backup).unwrap();

        assert_eq!(
            fs::read(target.join("resources/bin/mpv-2.dll")).unwrap(),
            b"downloaded mpv"
        );
        assert_eq!(
            fs::read(target.join("resources/bin/codecs/future-codec.dll")).unwrap(),
            b"downloaded future codec"
        );
        assert_eq!(
            fs::read(target.join("resources/bin/release.dll")).unwrap(),
            b"new release dll"
        );
        assert_eq!(
            fs::read(target.join("resources/theme.json")).unwrap(),
            b"new theme"
        );
        assert!(!target.join("resources/removed.txt").exists());
        assert!(!target.join("resources/bin/local-note.txt").exists());

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn update_preserves_runtime_prompt_studio_settings() {
        let temp =
            std::env::temp_dir().join(format!("xrt_updater_prompt_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let source = temp.join("source");
        let target = temp.join("target");
        let backup = temp.join("backup");

        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(target.join("runtime")).unwrap();
        fs::write(
            target.join("runtime/prompt-studio.json"),
            br#"{"active_id":"user-profile-1","profiles":[{"id":"user-profile-1","name":"My saved project"}]}"#,
        )
        .unwrap();
        fs::write(source.join("rust-client.exe"), b"new client").unwrap();

        let source_entries = source_entries(&source).unwrap();
        replace_entries(&source, &target, &source_entries, &backup).unwrap();

        let settings = fs::read_to_string(target.join("runtime/prompt-studio.json")).unwrap();
        assert!(settings.contains("user-profile-1"));
        assert!(settings.contains("My saved project"));

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn update_replaces_release_config_catalogue() {
        let temp =
            std::env::temp_dir().join(format!("xrt_updater_config_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let source = temp.join("source");
        let target = temp.join("target");
        let backup = temp.join("backup");

        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::write(
            source.join("config.json"),
            br#"{"models":{"new-model":{}}}"#,
        )
        .unwrap();
        fs::write(
            target.join("config.json"),
            br#"{"models":{"old-model":{}}}"#,
        )
        .unwrap();

        let source_entries = source_entries(&source).unwrap();
        replace_entries(&source, &target, &source_entries, &backup).unwrap();

        assert_eq!(
            fs::read(target.join("config.json")).unwrap(),
            br#"{"models":{"new-model":{}}}"#
        );
        assert!(backup.join("config.json").is_file());
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn update_resets_first_run_state_to_true_for_onboarding_and_notices() {
        let temp =
            std::env::temp_dir().join(format!("xrt_updater_first_run_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let source = temp.join("source");
        let target = temp.join("target");

        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(target.join("runtime")).unwrap();
        fs::write(
            target.join("runtime/app_state.json"),
            br#"{"first_run":false,"ui_language":"japanese"}"#,
        )
        .unwrap();
        fs::write(
            target.join("runtime/rust-client-settings.json"),
            br#"{"first_run":false,"ui_language":"japanese"}"#,
        )
        .unwrap();

        apply_update(&source, &target).unwrap();

        let app_state: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(target.join("runtime/app_state.json")).unwrap())
                .unwrap();
        assert_eq!(app_state["first_run"], true);
        assert_eq!(app_state["ui_language"], "japanese");

        let settings: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(target.join("runtime/rust-client-settings.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(settings["first_run"], true);
        assert_eq!(settings["ui_language"], "japanese");

        let _ = fs::remove_dir_all(&temp);
    }
}
