//! Build/distribute the SDK and build an independently checked-out plugin.
use serde_json::Value;
use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn manifest(path: &Path) -> Result<toml::Value> {
    Ok(toml::from_str(&fs::read_to_string(path)?)?)
}
fn copy_sources(source: &Path, destination: &Path) -> Result<()> {
    if source.is_dir() {
        fs::create_dir_all(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            if matches!(entry.file_name().to_str(), Some("target" | ".git")) {
                continue;
            }
            if entry.file_type()?.is_symlink() {
                return Err("SDK sources cannot contain symbolic links".into());
            }
            copy_sources(&entry.path(), &destination.join(entry.file_name()))?;
        }
    } else {
        fs::copy(source, destination)?;
    }
    Ok(())
}
fn local_dependencies(
    value: &toml::Value,
    directory: &Path,
    found: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    if let Some(table) = value.as_table() {
        for value in table.values() {
            if let Some(path) = value.get("path").and_then(toml::Value::as_str) {
                let path = directory.join(path);
                if path.join("Cargo.toml").is_file() {
                    let path = path.canonicalize()?;
                    if found.insert(path.clone()) {
                        local_dependencies(&manifest(&path.join("Cargo.toml"))?, &path, found)?;
                    }
                }
            } else {
                local_dependencies(value, directory, found)?;
            }
        }
    }
    Ok(())
}

fn export(root: &Path, destination: &Path) -> Result<()> {
    let root = root.canonicalize()?;
    if destination.exists() {
        return Err("SDK output already exists; choose a new directory".into());
    }
    let mut packages = BTreeSet::new();
    for name in [
        "xrtranslate-plugin",
        "xrtranslate-client-core",
        "xrtranslate-config",
    ] {
        let directory = root.join("crates").join(name).canonicalize()?;
        packages.insert(directory.clone());
        local_dependencies(
            &manifest(&directory.join("Cargo.toml"))?,
            &directory,
            &mut packages,
        )?;
    }
    fs::create_dir_all(destination)?;
    for package in &packages {
        let relative = package.strip_prefix(&root)?;
        fs::create_dir_all(destination.join(relative).parent().unwrap())?;
        copy_sources(package, &destination.join(relative))?;
    }
    for name in [
        "Cargo.lock",
        "config.json",
        "LICENSE",
        "assets",
        "third_party/winit",
        "examples/plugin-window",
    ] {
        let target = destination.join(name);
        fs::create_dir_all(target.parent().unwrap())?;
        super::copy(&root.join(name), &target)?;
    }
    fs::copy(root.join("Cargo.lock"), destination.join("SDK.lock"))?;
    let mut workspace = manifest(&root.join("Cargo.toml"))?;
    workspace["workspace"]["members"] = toml::Value::Array(
        packages
            .iter()
            .filter_map(|path| path.strip_prefix(&root).ok())
            .filter(|path| path.starts_with("crates"))
            .map(|path| toml::Value::String(path.to_string_lossy().replace('\\', "/")))
            .collect(),
    );
    workspace["workspace"]
        .as_table_mut()
        .unwrap()
        .remove("exclude");
    fs::write(
        destination.join("Cargo.toml"),
        toml::to_string_pretty(&workspace)?,
    )?;
    if destination.join("XR-Corpus").exists() {
        let mut corpus = manifest(&root.join("XR-Corpus/Cargo.toml"))?;
        corpus["workspace"]["members"] = toml::Value::Array(
            packages
                .iter()
                .filter_map(|path| path.strip_prefix(root.join("XR-Corpus")).ok())
                .map(|path| toml::Value::String(path.to_string_lossy().replace('\\', "/")))
                .collect(),
        );
        fs::write(
            destination.join("XR-Corpus/Cargo.toml"),
            toml::to_string_pretty(&corpus)?,
        )?;
        for name in ["LICENSE", "NOTICE"] {
            let source = root.join("XR-Corpus").join(name);
            if source.is_file() {
                fs::copy(source, destination.join("XR-Corpus").join(name))?;
            }
        }
    }
    let compiler = Command::new("rustc").arg("-vV").output()?;
    if !compiler.status.success() {
        return Err("Could not identify the SDK compiler".into());
    }
    fs::write(
        destination.join("sdk.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "sdk_version":env!("CARGO_PKG_VERSION"),
            "host_version":workspace["workspace"]["package"]["version"].as_str(),
            "compiler":String::from_utf8(compiler.stdout)?,
            "packages":packages.iter().map(|p| p.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/")).collect::<Vec<_>>()
        }))?,
    )?;
    let executable = std::env::current_exe()?;
    fs::copy(
        &executable,
        destination.join(executable.file_name().unwrap()),
    )?;
    fs::copy(
        root.join("crates/xrtranslate-plugin/SDK.md"),
        destination.join("README.md"),
    )?;
    println!("SDK created at {}", destination.display());
    Ok(())
}

fn build(arguments: &[OsString]) -> Result<()> {
    let source =
        Path::new(arguments.first().ok_or("Missing plugin source directory")?).canonicalize()?;
    let mut sdk = None;
    let mut output = None;
    let mut target = None;
    let mut features = None;
    let mut debug = false;
    let mut offline = false;
    let mut locked = false;
    let mut args = arguments[1..].iter();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--sdk") => sdk = args.next().map(PathBuf::from),
            Some("--output") => output = args.next().map(PathBuf::from),
            Some("--target-dir") => target = args.next().map(PathBuf::from),
            Some("--features") => features = args.next().cloned(),
            Some("--debug") => debug = true,
            Some("--offline") => offline = true,
            Some("--locked") => locked = true,
            _ => return Err(format!("Unknown build option: {}", arg.to_string_lossy()).into()),
        }
    }
    let sdk = sdk.ok_or("Pass --sdk <SDK directory>")?.canonicalize()?;
    let output = output.ok_or("Pass --output <plugin package directory>")?;
    let info: Value = serde_json::from_slice(&fs::read(sdk.join("sdk.json"))?)?;
    let plugin: xrtranslate_plugin::Manifest =
        serde_json::from_slice(&fs::read(source.join("plugin.json"))?)?;
    plugin.validate()?;
    let package = manifest(&source.join("Cargo.toml"))?;
    let name = package["package"]["name"]
        .as_str()
        .ok_or("Plugin package name is missing")?;
    let mut command = Command::new("cargo");
    command
        .current_dir(&source)
        .args(["build", "--manifest-path"])
        .arg(source.join("Cargo.toml"))
        .args([
            "--package",
            name,
            "--message-format=json-render-diagnostics",
        ]);
    if !debug {
        command.arg("--release");
    }
    if offline {
        command.arg("--offline");
    }
    if locked {
        command.arg("--locked");
    }
    if let Some(target) = target {
        command.arg("--target-dir").arg(target);
    }
    if let Some(features) = features {
        command.arg("--features").arg(features);
    }
    for relative in info["packages"]
        .as_array()
        .ok_or("Invalid SDK package list")?
    {
        let directory = sdk.join(relative.as_str().ok_or("Invalid SDK package path")?);
        let dependency = manifest(&directory.join("Cargo.toml"))?;
        let name = dependency["package"]["name"]
            .as_str()
            .ok_or("SDK package name missing")?;
        command.arg("--config").arg(format!(
            "patch.crates-io.{name}.path={:?}",
            directory.to_string_lossy()
        ));
    }
    command.arg("--config").arg(format!(
        "patch.crates-io.winit.path={:?}",
        sdk.join("third_party/winit").to_string_lossy()
    ));
    // Seed the first build with SDK versions. Afterwards Cargo manages the
    // plugin's own lockfile; --locked is available for reproducible builds.
    let workspace = source
        .ancestors()
        .find(|path| manifest(&path.join("Cargo.toml")).is_ok_and(|m| m.get("workspace").is_some()))
        .unwrap_or(&source);
    let lock = workspace.join("Cargo.lock");
    if !lock.exists() {
        fs::copy(sdk.join("SDK.lock"), &lock)?;
    }
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let mut artifact = None;
    for line in BufReader::new(child.stdout.take().unwrap()).lines() {
        let line = line?;
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if message["reason"] == "compiler-message" {
            if let Some(text) = message["message"]["rendered"].as_str() {
                eprint!("{text}");
            }
        } else if message["reason"] == "compiler-artifact" && message["target"]["name"] == name {
            artifact = message["executable"].as_str().map(PathBuf::from);
        }
    }
    if !child.wait()?.success() {
        return Err("Plugin build failed".into());
    }
    super::stage(
        &source,
        &artifact.ok_or("Plugin artifact was not produced")?,
        &output,
        true,
    )?;
    println!("Plugin package created at {}", output.display());
    Ok(())
}

pub(super) fn run(args: &[OsString]) -> Result<()> {
    if args[0] == "sdk" && args.len() == 3 {
        export(Path::new(&args[1]), Path::new(&args[2]))
    } else if args[0] == "build" {
        build(&args[1..])
    } else {
        Err("Usage: xrt-plugin sdk <source-root> <output> | build <plugin-source> --sdk <SDK directory> --output <plugin directory> [--target-dir <cache>] [--features <features>] [--debug] [--offline] [--locked]".into())
    }
}
