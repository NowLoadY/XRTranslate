//! XRTranslate release packaging.
//! It never scans nor copies the repository's Python backend, server, launch
//! scripts, requirements, or llama.cpp binaries.

#![forbid(unsafe_code)]

use std::{
    error::Error,
    ffi::OsStr,
    fmt, fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use clap::Parser;
use serde_json::{Map, Value, json};
use xr_corpus_core::validate_seed_database;
use xrtranslate_assets::{
    ModelAssetId, ModelAssetManifest, ModelAssetsConfig, ResolvedModelAssets,
};
use xrtranslate_config::{AppConfig, RuntimeLayout};

const RELEASE_LAYOUT_VERSION: u32 = 4;
const VAD_RELATIVE_PATH: &str = RuntimeLayout::VAD_MODEL_PATH;
const VAD_MODEL_VERSION: &str = "v6.2.1";
const VAD_MODEL_BYTES: u64 = RuntimeLayout::VAD_MODEL_BYTES;
const SPEAKER_RELATIVE_PATH: &str = RuntimeLayout::SPEAKER_MODEL_PATH;
const SPEAKER_MODEL_BYTES: u64 = RuntimeLayout::SPEAKER_MODEL_BYTES;
const DENOISE_RELATIVE_PATH: &str = RuntimeLayout::DENOISE_MODEL_PATH;
const DENOISE_MODEL_BYTES: u64 = RuntimeLayout::DENOISE_MODEL_BYTES;
const INTERNAL_BIN_DIRECTORY: &str = "bin";
const ONNX_LICENSE_RELATIVE_PATH: &str = "licenses/onnxruntime/LICENSE";
const ONNX_NOTICES_RELATIVE_PATH: &str = "licenses/onnxruntime/ThirdPartyNotices.txt";

#[derive(Debug)]
struct VcRuntimeFile {
    name: &'static str,
    source: PathBuf,
    bytes: u64,
}

struct OnnxCpuMetadata {
    path: &'static str,
    bytes: u64,
    source_archive: &'static str,
    license_bytes: u64,
    notices_bytes: u64,
}

fn onnx_cpu_metadata(client: &Path) -> OnnxCpuMetadata {
    if client
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
    {
        OnnxCpuMetadata {
            path: "runtime/onnxruntime/cpu/onnxruntime.dll",
            bytes: RuntimeLayout::ONNX_CPU_CORE_WIN_BYTES,
            source_archive: RuntimeLayout::ONNX_CPU_CORE_WIN_SOURCE_ARCHIVE,
            license_bytes: 1_094,
            notices_bytes: 331_175,
        }
    } else {
        OnnxCpuMetadata {
            path: "runtime/onnxruntime/cpu/libonnxruntime.so.1.28.0",
            bytes: RuntimeLayout::ONNX_CPU_CORE_LINUX_BYTES,
            source_archive: "onnxruntime-linux-x64-1.28.0.tgz",
            license_bytes: 1_073,
            notices_bytes: 325_054,
        }
    }
}
const CORPUS_DATABASE_PATH: &str = "runtime/xr-corpus.sqlite";
const CORPUS_SEED_PATH: &str = "corpora/default.sqlite";

#[derive(Debug, Parser)]
#[command(
    name = "xrtranslate-packager",
    version,
    about = "Build a Python-free native XRTranslate release directory"
)]
struct Arguments {
    /// Rust desktop-client executable built for the target platform.
    #[arg(long)]
    rust_client_bin: PathBuf,
    /// Native xrtranslate-backend executable built for the target platform.
    #[arg(long)]
    backend_bin: PathBuf,
    #[arg(long)]
    corpus_bin: PathBuf,
    /// Native xrtranslate-installer executable built for the target platform.
    #[arg(long)]
    installer_bin: PathBuf,
    /// Native xrtranslate-updater executable built for the target platform.
    #[arg(long)]
    updater_bin: PathBuf,
    /// Compatibility configuration to rewrite for the staged release.
    #[arg(long, default_value = "config.json")]
    config: PathBuf,
    /// Desktop resources copied into `resources/`.
    #[arg(long, default_value = "rust-client/resources")]
    resources_dir: PathBuf,
    /// Default terminology database copied into the user's runtime on first launch.
    #[arg(long, default_value = "XR-Corpus/corpora/default.sqlite")]
    seed_database: PathBuf,
    #[arg(long, default_value = "LICENSE")]
    license: PathBuf,
    /// Standard Silero VAD 16 kHz ONNX file.
    #[arg(long)]
    vad_model: Option<PathBuf>,
    /// 3D-Speaker ERes2NetV2 speaker-embedding ONNX file bundled in every release.
    #[arg(long)]
    speaker_model: Option<PathBuf>,
    /// GTCRN-Light v3 speech enhancement ONNX file bundled in every release.
    #[arg(long)]
    denoise_model: Option<PathBuf>,
    /// Verified ONNX Runtime 1.28 core used by CPU-only hosts and as the
    /// universal fallback. GPU providers remain managed downloads.
    #[arg(long)]
    onnx_runtime_cpu: PathBuf,
    #[arg(long)]
    onnx_runtime_license: PathBuf,
    #[arg(long)]
    onnx_runtime_notices: PathBuf,
    /// Visual Studio x64 release CRT redistributable directory (required for Windows).
    #[arg(long)]
    vc_runtime_dir: Option<PathBuf>,
    /// Destination release directory. It must not already exist.
    #[arg(long)]
    output: PathBuf,
    /// Include already installed, validated Qwen3-ASR and Hy-MT2 GGUF models.
    /// Without this flag the package contains the model layout and installer only.
    #[arg(long)]
    include_models: bool,
    /// Validate every input and produce the release manifest in memory, without writing output.
    #[arg(long)]
    check: bool,
}

#[derive(Debug)]
enum PackageError {
    InvalidInput(String),
    Io { context: String, source: io::Error },
    Json(serde_json::Error),
    Config(xrtranslate_config::ConfigError),
    Assets(xrtranslate_assets::ModelAssetsPreflightError),
}

impl fmt::Display for PackageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(message) => formatter.write_str(message),
            Self::Io { context, source } => write!(formatter, "{context}: {source}"),
            Self::Json(source) => source.fmt(formatter),
            Self::Config(source) => source.fmt(formatter),
            Self::Assets(source) => source.fmt(formatter),
        }
    }
}

impl Error for PackageError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Json(source) => Some(source),
            Self::Config(source) => Some(source),
            Self::Assets(source) => Some(source),
            Self::InvalidInput(_) => None,
        }
    }
}

impl From<serde_json::Error> for PackageError {
    fn from(source: serde_json::Error) -> Self {
        Self::Json(source)
    }
}

impl From<xrtranslate_config::ConfigError> for PackageError {
    fn from(source: xrtranslate_config::ConfigError) -> Self {
        Self::Config(source)
    }
}

impl From<xrtranslate_assets::ModelAssetsPreflightError> for PackageError {
    fn from(source: xrtranslate_assets::ModelAssetsPreflightError) -> Self {
        Self::Assets(source)
    }
}

#[derive(Debug)]
struct ReleasePlan {
    rust_client_bin: PathBuf,
    backend_bin: PathBuf,
    corpus_bin: PathBuf,
    installer_bin: PathBuf,
    updater_bin: PathBuf,
    resources_dir: PathBuf,
    seed_database: PathBuf,
    license: PathBuf,
    vad_model: PathBuf,
    speaker_model: PathBuf,
    denoise_model: PathBuf,
    onnx_runtime_cpu: PathBuf,
    onnx_runtime_license: PathBuf,
    onnx_runtime_notices: PathBuf,
    vc_runtime: Vec<VcRuntimeFile>,
    output: PathBuf,
    include_models: bool,
    assets: ResolvedModelAssets,
    packaged_config: String,
    manifest: Value,
}

fn main() -> Result<(), Box<dyn Error>> {
    let arguments = Arguments::parse();
    let check = arguments.check;
    let plan = ReleasePlan::from_arguments(arguments)?;
    let onnx = onnx_cpu_metadata(&plan.rust_client_bin);
    verify_file_size("--vad-model", &plan.vad_model, VAD_MODEL_BYTES)?;
    verify_file_size("--speaker-model", &plan.speaker_model, SPEAKER_MODEL_BYTES)?;
    verify_file_size("--denoise-model", &plan.denoise_model, DENOISE_MODEL_BYTES)?;
    verify_file_size("--onnx-runtime-cpu", &plan.onnx_runtime_cpu, onnx.bytes)?;
    verify_file_size(
        "--onnx-runtime-license",
        &plan.onnx_runtime_license,
        onnx.license_bytes,
    )?;
    verify_file_size(
        "--onnx-runtime-notices",
        &plan.onnx_runtime_notices,
        onnx.notices_bytes,
    )?;
    if plan.output.exists() {
        return Err(PackageError::InvalidInput(format!(
            "refusing to overwrite existing release output {}",
            plan.output.display()
        ))
        .into());
    }

    if plan.include_models {
        plan.assets.check().into_result()?;
    }

    if plan.manifest["python"].as_bool() != Some(false) {
        return Err(PackageError::InvalidInput(
            "internal safety check failed: release manifest is not Python-free".into(),
        )
        .into());
    }

    if check {
        println!(
            "Native release inputs are valid. Dry run would stage {}{}.",
            plan.output.display(),
            if plan.include_models {
                " with verified GGUF models"
            } else {
                " without GGUF models"
            }
        );
        return Ok(());
    }

    let output = package(&plan)?;
    println!("Native Python-free release staged at {}", output.display());
    Ok(())
}

impl ReleasePlan {
    fn from_arguments(arguments: Arguments) -> Result<Self, PackageError> {
        require_regular_file("--rust-client-bin", &arguments.rust_client_bin)?;
        require_regular_file("--backend-bin", &arguments.backend_bin)?;
        require_regular_file("--corpus-bin", &arguments.corpus_bin)?;
        require_regular_file("--installer-bin", &arguments.installer_bin)?;
        require_regular_file("--updater-bin", &arguments.updater_bin)?;
        require_regular_file("--onnx-runtime-cpu", &arguments.onnx_runtime_cpu)?;
        require_regular_file("--onnx-runtime-license", &arguments.onnx_runtime_license)?;
        require_regular_file("--onnx-runtime-notices", &arguments.onnx_runtime_notices)?;
        require_regular_file("--config", &arguments.config)?;
        require_directory("--resources-dir", &arguments.resources_dir)?;
        require_regular_file("--seed-database", &arguments.seed_database)?;
        require_regular_file("--license", &arguments.license)?;
        let vc_runtime = windows_vc_runtime(
            &arguments.rust_client_bin,
            arguments.vc_runtime_dir.as_deref(),
        )?;

        let project_root = arguments
            .config
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let vad_model = arguments
            .vad_model
            .unwrap_or_else(|| project_root.join(VAD_RELATIVE_PATH));
        require_regular_file("--vad-model", &vad_model)?;
        let speaker_model = arguments
            .speaker_model
            .unwrap_or_else(|| project_root.join(SPEAKER_RELATIVE_PATH));
        require_regular_file("--speaker-model", &speaker_model)?;
        let denoise_model = arguments
            .denoise_model
            .unwrap_or_else(|| project_root.join(DENOISE_RELATIVE_PATH));
        require_regular_file("--denoise-model", &denoise_model)?;

        ensure_directory_is_native("--resources-dir", &arguments.resources_dir)?;
        ensure_native_file("--seed-database", &arguments.seed_database)?;
        validate_seed_database(&arguments.seed_database).map_err(|error| {
            PackageError::InvalidInput(format!("invalid --seed-database: {error}"))
        })?;
        ensure_native_file("--vad-model", &vad_model)?;
        ensure_native_file("--speaker-model", &speaker_model)?;
        ensure_native_file("--denoise-model", &denoise_model)?;

        let config = AppConfig::from_path(&arguments.config)?;
        let mut asset_config = ModelAssetsConfig::with_directory_overrides(
            config.model_manager.models_directory.clone(),
            config.model_manager.qwen3_asr_gguf_directory.clone(),
            config.model_manager.hunyuan_mt_gguf_directory.clone(),
        );
        for key in config.active_native_model_assets() {
            if let Some(id) = ModelAssetId::from_config_key(&key) {
                asset_config.select_asset(id);
            }
        }
        let assets = asset_config.resolve(&project_root);
        let packaged_config = rewrite_config(&arguments.config)?;
        let manifest = release_manifest(
            &arguments.rust_client_bin,
            &arguments.backend_bin,
            &arguments.corpus_bin,
            &arguments.installer_bin,
            &arguments.updater_bin,
            arguments.include_models,
            &assets,
            &RuntimeLayout::for_project_root(&project_root),
            &vc_runtime,
        );

        Ok(Self {
            rust_client_bin: arguments.rust_client_bin,
            backend_bin: arguments.backend_bin,
            corpus_bin: arguments.corpus_bin,
            installer_bin: arguments.installer_bin,
            updater_bin: arguments.updater_bin,
            resources_dir: arguments.resources_dir,
            seed_database: arguments.seed_database,
            license: arguments.license,
            vad_model,
            speaker_model,
            denoise_model,
            onnx_runtime_cpu: arguments.onnx_runtime_cpu,
            onnx_runtime_license: arguments.onnx_runtime_license,
            onnx_runtime_notices: arguments.onnx_runtime_notices,
            vc_runtime,
            output: arguments.output,
            include_models: arguments.include_models,
            assets,
            packaged_config,
            manifest,
        })
    }
}

fn package(plan: &ReleasePlan) -> Result<PathBuf, PackageError> {
    let staging = staging_path(&plan.output)?;
    fs::create_dir(&staging).map_err(|source| PackageError::Io {
        context: format!(
            "cannot create release staging directory {}",
            staging.display()
        ),
        source,
    })?;

    let result = (|| {
        copy_file_to(
            &plan.rust_client_bin,
            &staging.join(release_client_name(&plan.rust_client_bin)?),
        )?;
        copy_file_to(
            &plan.backend_bin,
            &staging
                .join(INTERNAL_BIN_DIRECTORY)
                .join(native_binary_name(
                    "xrtranslate-backend",
                    &plan.backend_bin,
                )?),
        )?;
        copy_file_to(
            &plan.corpus_bin,
            &staging
                .join(INTERNAL_BIN_DIRECTORY)
                .join(native_binary_name("xr-corpus-server", &plan.corpus_bin)?),
        )?;
        copy_file_to(
            &plan.installer_bin,
            &staging
                .join(INTERNAL_BIN_DIRECTORY)
                .join(native_binary_name(
                    "xrtranslate-installer",
                    &plan.installer_bin,
                )?),
        )?;
        copy_file_to(
            &plan.updater_bin,
            &staging
                .join(INTERNAL_BIN_DIRECTORY)
                .join(native_binary_name(
                    "xrtranslate-updater",
                    &plan.updater_bin,
                )?),
        )?;
        copy_vc_runtime(&plan.vc_runtime, &staging)?;
        copy_native_directory(&plan.resources_dir, &staging.join("resources"))?;
        copy_file_to(&plan.seed_database, &staging.join(CORPUS_SEED_PATH))?;
        copy_file_to(&plan.license, &staging.join("LICENSE"))?;
        // Export the exact curated references used by the app, never runtime recordings.
        for voice in xrtranslate_assets::voices::BUILTIN_VOICES {
            let directory = staging.join("resources/voices").join(voice.id);
            fs::create_dir_all(&directory)
                .and_then(|()| {
                    fs::write(directory.join("reference.wav"), voice.wav)?;
                    fs::write(directory.join("reference.txt"), voice.transcript)?;
                    fs::write(directory.join("SOURCE.md"), voice.source_notice)?;
                    fs::write(directory.join("LICENSE"), voice.license)
                })
                .map_err(|source| PackageError::Io {
                    context: format!("cannot write built-in voice resources {}", voice.id),
                    source,
                })?;
        }
        copy_file_to(&plan.vad_model, &staging.join(VAD_RELATIVE_PATH))?;
        copy_file_to(&plan.speaker_model, &staging.join(SPEAKER_RELATIVE_PATH))?;
        copy_file_to(&plan.denoise_model, &staging.join(DENOISE_RELATIVE_PATH))?;
        copy_file_to(
            &plan.onnx_runtime_cpu,
            &staging.join(onnx_cpu_metadata(&plan.rust_client_bin).path),
        )?;
        copy_file_to(
            &plan.onnx_runtime_license,
            &staging.join(ONNX_LICENSE_RELATIVE_PATH),
        )?;
        copy_file_to(
            &plan.onnx_runtime_notices,
            &staging.join(ONNX_NOTICES_RELATIVE_PATH),
        )?;
        fs::write(staging.join("config.json"), &plan.packaged_config).map_err(|source| {
            PackageError::Io {
                context: format!("cannot write staged config in {}", staging.display()),
                source,
            }
        })?;
        write_model_layout(&staging, plan.include_models, &plan.assets)?;
        if plan.include_models {
            copy_model_packages(&staging, &plan.assets)?;
        }
        fs::write(
            staging.join("release-manifest.json"),
            format!("{}\n", serde_json::to_string_pretty(&plan.manifest)?),
        )
        .map_err(|source| PackageError::Io {
            context: format!(
                "cannot write staged release manifest in {}",
                staging.display()
            ),
            source,
        })?;
        verify_staged_release(&staging)?;
        Ok(())
    })();

    if let Err(error) = result {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    fs::rename(&staging, &plan.output).map_err(|source| PackageError::Io {
        context: format!(
            "cannot atomically publish release staging {} to {}",
            staging.display(),
            plan.output.display()
        ),
        source,
    })?;
    Ok(plan.output.clone())
}

fn rewrite_config(config_path: &Path) -> Result<String, PackageError> {
    let text = fs::read_to_string(config_path).map_err(|source| PackageError::Io {
        context: format!("cannot read config {}", config_path.display()),
        source,
    })?;
    let mut root: Value = serde_json::from_str(&text)?;
    let root_object = root.as_object_mut().ok_or_else(|| {
        PackageError::InvalidInput(format!(
            "config {} must contain a JSON object",
            config_path.display()
        ))
    })?;
    let manager = root_object
        .entry("model_manager")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| {
            PackageError::InvalidInput("config.model_manager must be a JSON object".into())
        })?;
    manager.insert("llama_server_path".into(), Value::String(String::new()));
    manager.insert("runtime_directory".into(), Value::String("runtime".into()));
    manager.insert("models_directory".into(), Value::String("models".into()));
    manager.remove("runtime_root");
    manager.remove("qwen3_asr_gguf_directory");
    manager.remove("hunyuan_mt_gguf_directory");
    let speaker = root_object
        .entry("speaker")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| PackageError::InvalidInput("config.speaker must be a JSON object".into()))?;
    speaker.insert("enabled".into(), Value::Bool(true));
    speaker.insert(
        "model_path".into(),
        Value::String(SPEAKER_RELATIVE_PATH.into()),
    );
    let denoise = root_object
        .entry("denoise")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| PackageError::InvalidInput("config.denoise must be a JSON object".into()))?;
    denoise.insert("enabled".into(), Value::Bool(true));
    denoise.insert(
        "model_path".into(),
        Value::String(DENOISE_RELATIVE_PATH.into()),
    );
    let prompt_context = root_object
        .entry("prompt_context")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| {
            PackageError::InvalidInput("config.prompt_context must be a JSON object".into())
        })?;
    prompt_context.insert(
        "database_path".into(),
        Value::String(CORPUS_DATABASE_PATH.into()),
    );
    prompt_context.insert(
        "seed_database_path".into(),
        Value::String(CORPUS_SEED_PATH.into()),
    );
    for optional in ["tts", "ocr"] {
        if let Some(section) = root_object.get_mut(optional).and_then(Value::as_object_mut) {
            section.insert("provider".into(), Value::String("none".into()));
        }
    }
    Ok(format!("{}\n", serde_json::to_string_pretty(&root)?))
}

fn release_manifest(
    rust_client: &Path,
    backend: &Path,
    corpus: &Path,
    installer: &Path,
    updater: &Path,
    include_models: bool,
    assets: &ResolvedModelAssets,
    runtime_layout: &RuntimeLayout,
    vc_runtime: &[VcRuntimeFile],
) -> Value {
    let onnx = onnx_cpu_metadata(rust_client);
    let model_packages = assets
        .iter()
        .iter()
        .map(|asset| manifest_json(asset.manifest()))
        .collect::<Vec<_>>();
    let runtime_directory = runtime_layout
        .llama_cpp_directory()
        .strip_prefix(runtime_layout.project_root())
        .unwrap_or_else(|_| std::path::Path::new(RuntimeLayout::LLAMA_CPP_DIRECTORY))
        .to_string_lossy()
        .replace('\\', "/");
    json!({
        "layout_version": RELEASE_LAYOUT_VERSION,
        "python": false,
        "entrypoints": {
            "client": release_client_name(rust_client).unwrap_or_else(|_| "XRTranslate".into()),
            "backend": format!("{INTERNAL_BIN_DIRECTORY}/{}", native_binary_name("xrtranslate-backend", backend).unwrap_or_else(|_| "xrtranslate-backend".into())),
            "corpus": format!("{INTERNAL_BIN_DIRECTORY}/{}", native_binary_name("xr-corpus-server", corpus).unwrap_or_else(|_| "xr-corpus-server".into())),
            "installer": format!("{INTERNAL_BIN_DIRECTORY}/{}", native_binary_name("xrtranslate-installer", installer).unwrap_or_else(|_| "xrtranslate-installer".into())),
            "updater": format!("{INTERNAL_BIN_DIRECTORY}/{}", native_binary_name("xrtranslate-updater", updater).unwrap_or_else(|_| "xrtranslate-updater".into())),
        },
        "runtime": {
            "included": true,
            "msvc": {
                "included": !vc_runtime.is_empty(),
                "delivery": "app-local",
                "files": vc_runtime.iter().map(|file| json!({
                    "paths": [file.name, format!("{INTERNAL_BIN_DIRECTORY}/{}", file.name)],
                    "architecture": "x86_64",
                    "bytes": file.bytes,
                })).collect::<Vec<_>>(),
            },
            "directory": runtime_directory,
            "setup_required": "Download the selected models and compatible runtime in the client welcome flow.",
            "onnx_cuda": {
                "included": false,
                "selection_marker": RuntimeLayout::NATIVE_RUNTIME_SELECTION_FILE,
                "provider_directory": RuntimeLayout::ONNX_RUNTIME_DIRECTORY,
                "cuda_directory": RuntimeLayout::CUDA_RUNTIME_DIRECTORY,
                "delivery": "managed-download"
            },
            "onnx_cpu": {
                "included": true,
                "path": onnx.path,
                "release": "1.28.0",
                "bytes": onnx.bytes,
                "source_archive": onnx.source_archive,
                "license": ONNX_LICENSE_RELATIVE_PATH,
                "third_party_notices": ONNX_NOTICES_RELATIVE_PATH
            }
        },
        "vad_model": {
            "path": VAD_RELATIVE_PATH,
            "architecture": "Silero VAD",
            "format": "ONNX opset 16",
            "sample_rate_hz": 16_000,
            "frame_samples": 512,
            "source": {
                "repository": "snakers4/silero-vad",
                "revision": VAD_MODEL_VERSION,
            },
            "bytes": VAD_MODEL_BYTES,
        },
        "speaker_model": {
            "path": SPEAKER_RELATIVE_PATH,
            "architecture": "ERes2NetV2",
            "source": {
                "repository": "iic/speech_eres2netv2_sv_zh-cn_16k-common",
                "revision": "v1.0.1",
            },
            "bytes": SPEAKER_MODEL_BYTES,
        },
        "denoise_model": {
            "path": DENOISE_RELATIVE_PATH,
            "architecture": "GTCRN-Light-v3",
            "source": {
                "repository": "k2-fsa/sherpa-onnx",
                "release": "speech-enhancement-models",
            },
            "bytes": DENOISE_MODEL_BYTES,
        },
        "resources": "resources",
        "corpora": {
            "database_path": CORPUS_DATABASE_PATH,
            "seed_database_path": CORPUS_SEED_PATH,
            "format": "sqlite",
            "dynamic_sources_supported": true,
        },
        "models": {
            "included": include_models,
            "root": "models",
            "packages": model_packages,
        },
        "excluded": ["backend/", "server/", "main.py", "start_services.py", "requirements.txt"],
    })
}

fn manifest_json(manifest: &ModelAssetManifest) -> Value {
    json!({
        "id": manifest.id.as_str(),
        "directory": format!("models/{}", manifest.relative_directory),
        "source": {
            "repository": manifest.source.repository,
            "revision": manifest.source.revision,
        },
        "files": manifest.required_files.iter().map(|file| json!({
            "path": file.relative_path,
            "bytes": file.bytes,
            "url": manifest.source.file_url(file.relative_path),
        })).collect::<Vec<_>>(),
    })
}

fn write_model_layout(
    staging: &Path,
    include_models: bool,
    assets: &ResolvedModelAssets,
) -> Result<(), PackageError> {
    let models = staging.join("models");
    fs::create_dir_all(&models).map_err(|source| PackageError::Io {
        context: format!("cannot create staged models layout {}", models.display()),
        source,
    })?;
    let layout = json!({
        "included": include_models,
        "install_command": format!("{INTERNAL_BIN_DIRECTORY}/xrtranslate-installer install qwen3-asr-gguf && {INTERNAL_BIN_DIRECTORY}/xrtranslate-installer install hy-mt2"),
        "packages": assets.iter().iter().map(|asset| manifest_json(asset.manifest())).collect::<Vec<_>>(),
    });
    fs::write(
        models.join("native-model-layout.json"),
        format!("{}\n", serde_json::to_string_pretty(&layout)?),
    )
    .map_err(|source| PackageError::Io {
        context: "cannot write native model layout".into(),
        source,
    })
}

fn copy_model_packages(staging: &Path, assets: &ResolvedModelAssets) -> Result<(), PackageError> {
    for asset in assets.iter() {
        let target = staging
            .join("models")
            .join(asset.manifest().relative_directory);
        fs::create_dir_all(&target).map_err(|source| PackageError::Io {
            context: format!("cannot create staged model directory {}", target.display()),
            source,
        })?;
        for index in 0..asset.manifest().required_files.len() {
            let source = asset.required_file_path(index);
            let relative = &asset.manifest().required_files[index].relative_path;
            copy_file_to(&source, &target.join(relative))?;
        }
    }
    Ok(())
}

fn verify_staged_release(staging: &Path) -> Result<(), PackageError> {
    for private in [
        "runtime/debug.md",
        "runtime/voice_clones",
        "runtime/recordings",
        "runtime/user-config.json",
        "runtime/rust-client-settings.json",
    ] {
        if staging.join(private).exists() {
            return Err(PackageError::InvalidInput(format!(
                "staged release contains private local data: {private}"
            )));
        }
    }
    for forbidden in [
        "backend",
        "server",
        "main.py",
        "start_services.py",
        "requirements.txt",
    ] {
        if staging.join(forbidden).exists() {
            return Err(PackageError::InvalidInput(format!(
                "staged release unexpectedly contains forbidden Python artifact {forbidden}"
            )));
        }
    }
    ensure_directory_is_native("staged release", staging)?;
    for required in ["LICENSE", CORPUS_SEED_PATH] {
        if !staging.join(required).exists() {
            return Err(PackageError::InvalidInput(format!(
                "staged release is missing required corpus asset {required}"
            )));
        }
    }
    let manifest_path = staging.join("release-manifest.json");
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(&manifest_path).map_err(|source| {
            PackageError::Io {
                context: format!(
                    "cannot read staged release manifest {}",
                    manifest_path.display()
                ),
                source,
            }
        })?)?;
    if manifest["python"].as_bool() != Some(false) {
        return Err(PackageError::InvalidInput(
            "staged release manifest must set python to false".into(),
        ));
    }
    Ok(())
}

fn require_regular_file(label: &str, path: &Path) -> Result<(), PackageError> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_) => Err(PackageError::InvalidInput(format!(
            "{label} must be a regular file: {}",
            path.display()
        ))),
        Err(source) => Err(PackageError::Io {
            context: format!("cannot inspect {label} at {}", path.display()),
            source,
        }),
    }
}

fn require_directory(label: &str, path: &Path) -> Result<(), PackageError> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(PackageError::InvalidInput(format!(
            "{label} must be a directory: {}",
            path.display()
        ))),
        Err(source) => Err(PackageError::Io {
            context: format!("cannot inspect {label} at {}", path.display()),
            source,
        }),
    }
}

fn windows_vc_runtime(
    client: &Path,
    directory: Option<&Path>,
) -> Result<Vec<VcRuntimeFile>, PackageError> {
    if !client
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
    {
        return Ok(Vec::new());
    }
    let directory = directory.ok_or_else(|| {
        PackageError::InvalidInput(
            "Windows releases require --vc-runtime-dir pointing to the Visual Studio x64 release CRT redistributable directory".into(),
        )
    })?;
    require_directory("--vc-runtime-dir", directory)?;
    // Validate the resolved location too: a path alias must not turn System32
    // or a debug-runtime directory into an apparent redistributable input.
    let directory = fs::canonicalize(directory).map_err(|source| PackageError::Io {
        context: format!("cannot resolve --vc-runtime-dir {}", directory.display()),
        source,
    })?;
    let components = directory
        .components()
        .rev()
        .take(6)
        .map(|part| part.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect::<Vec<_>>();
    let is_redist = match components.as_slice() {
        [crt, architecture, version, msvc, redist, vc] => {
            vc == "vc"
                && redist == "redist"
                && msvc == "msvc"
                && architecture == "x64"
                && version.contains('.')
                && version
                    .split('.')
                    .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
                && crt
                    .strip_prefix("microsoft.vc")
                    .and_then(|suffix| suffix.strip_suffix(".crt"))
                    .is_some_and(|toolset| {
                        !toolset.is_empty() && toolset.bytes().all(|byte| byte.is_ascii_digit())
                    })
        }
        _ => false,
    };
    if !is_redist {
        return Err(PackageError::InvalidInput(
            "--vc-runtime-dir must be a Visual Studio VC/Redist/MSVC/<version>/x64/Microsoft.VC*.CRT release redistributable directory; System32 and DebugCRT are not release inputs".into(),
        ));
    }
    let mut files = Vec::new();
    for &name in RuntimeLayout::WINDOWS_CRT_REQUIRED_FILES
        .iter()
        .chain(RuntimeLayout::WINDOWS_CRT_OPTIONAL_FILES)
    {
        let source = directory.join(name);
        if !source.exists() && RuntimeLayout::WINDOWS_CRT_OPTIONAL_FILES.contains(&name) {
            continue;
        }
        require_regular_file("--vc-runtime-dir", &source)?;
        let bytes = fs::read(&source).map_err(|source_error| PackageError::Io {
            context: format!("cannot read VC runtime {}", source.display()),
            source: source_error,
        })?;
        validate_x64_crt_dll(&source, &bytes)?;
        files.push(VcRuntimeFile {
            name,
            source,
            bytes: bytes.len() as u64,
        });
    }
    Ok(files)
}

fn validate_x64_crt_dll(path: &Path, bytes: &[u8]) -> Result<(), PackageError> {
    let invalid = || {
        PackageError::InvalidInput(format!(
            "VC runtime must be an AMD64 PE32+ release DLL: {}",
            path.display()
        ))
    };
    if bytes.get(..2) != Some(b"MZ") {
        return Err(invalid());
    }
    let offset = bytes.get(0x3c..0x40).ok_or_else(invalid)?;
    let pe = u32::from_le_bytes(offset.try_into().map_err(|_| invalid())?) as usize;
    let header_end = pe.checked_add(26).ok_or_else(invalid)?;
    let header = bytes.get(pe..header_end).ok_or_else(invalid)?;
    let optional_size = u16::from_le_bytes([header[20], header[21]]) as usize;
    let section_count = u16::from_le_bytes([header[6], header[7]]) as usize;
    let optional_end = pe
        .checked_add(24)
        .and_then(|start| start.checked_add(optional_size))
        .ok_or_else(invalid)?;
    if &header[..4] != b"PE\0\0"
        || u16::from_le_bytes([header[4], header[5]]) != 0x8664
        || u16::from_le_bytes([header[22], header[23]]) & 0x2000 == 0
        || u16::from_le_bytes([header[24], header[25]]) != 0x20b
        || optional_size < 112
        || section_count == 0
        || optional_end
            .checked_add(section_count * 40)
            .is_none_or(|end| end > bytes.len())
    {
        return Err(invalid());
    }
    Ok(())
}

fn copy_vc_runtime(files: &[VcRuntimeFile], staging: &Path) -> Result<(), PackageError> {
    for file in files {
        // Both the GUI and the helper executables need an application-local CRT.
        for directory in [staging.to_path_buf(), staging.join(INTERNAL_BIN_DIRECTORY)] {
            let destination = directory.join(file.name);
            copy_file_to(&file.source, &destination)?;
            verify_file_size("staged VC runtime", &destination, file.bytes)?;
        }
    }
    Ok(())
}

fn verify_file_size(label: &str, path: &Path, expected_bytes: u64) -> Result<(), PackageError> {
    let metadata = fs::metadata(path).map_err(|source| PackageError::Io {
        context: format!("cannot inspect {label} at {}", path.display()),
        source,
    })?;
    if !metadata.is_file() || metadata.len() != expected_bytes {
        return Err(PackageError::InvalidInput(format!(
            "{label} has {} bytes, expected {expected_bytes}: {}",
            metadata.len(),
            path.display()
        )));
    }
    Ok(())
}

fn ensure_directory_is_native(label: &str, directory: &Path) -> Result<(), PackageError> {
    for entry in fs::read_dir(directory).map_err(|source| PackageError::Io {
        context: format!("cannot enumerate {label} at {}", directory.display()),
        source,
    })? {
        let entry = entry.map_err(|source| PackageError::Io {
            context: format!("cannot enumerate {label} at {}", directory.display()),
            source,
        })?;
        let file_type = entry.file_type().map_err(|source| PackageError::Io {
            context: format!("cannot inspect {}", entry.path().display()),
            source,
        })?;
        if file_type.is_symlink() {
            return Err(PackageError::InvalidInput(format!(
                "{label} must not contain symbolic links: {}",
                entry.path().display()
            )));
        }
        if file_type.is_dir() {
            if is_python_directory_name(entry.file_name().as_os_str()) {
                return Err(PackageError::InvalidInput(format!(
                    "{label} contains a forbidden Python directory: {}",
                    entry.path().display()
                )));
            }
            ensure_directory_is_native(label, &entry.path())?;
        } else if file_type.is_file() {
            ensure_native_file(label, &entry.path())?;
        } else {
            return Err(PackageError::InvalidInput(format!(
                "{label} contains a non-regular file: {}",
                entry.path().display()
            )));
        }
    }
    Ok(())
}

fn ensure_native_file(label: &str, path: &Path) -> Result<(), PackageError> {
    let is_python = path.extension().is_some_and(|extension| {
        ["py", "pyc", "pyo", "pyw"]
            .iter()
            .any(|forbidden| extension.eq_ignore_ascii_case(OsStr::new(forbidden)))
    }) || path
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case(OsStr::new("requirements.txt")));
    if is_python {
        return Err(PackageError::InvalidInput(format!(
            "{label} contains a Python artifact that native releases may not copy: {}",
            path.display()
        )));
    }
    Ok(())
}

fn is_python_directory_name(name: &OsStr) -> bool {
    ["backend", "server", "__pycache__", "venv", ".venv"]
        .iter()
        .any(|forbidden| name.eq_ignore_ascii_case(OsStr::new(forbidden)))
}

fn should_exclude_from_release_resources(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    lower == "mpv-2.zip"
        || lower.ends_with(".def")
        || lower.ends_with(".dll")
        || lower.ends_with(".dylib")
        || lower.ends_with(".so")
        || lower.contains(".so.")
}

fn copy_native_directory(source: &Path, target: &Path) -> Result<(), PackageError> {
    ensure_directory_is_native("release input", source)?;
    fs::create_dir_all(target).map_err(|source| PackageError::Io {
        context: format!("cannot create release directory {}", target.display()),
        source,
    })?;
    for entry in fs::read_dir(source).map_err(|error| PackageError::Io {
        context: format!("cannot enumerate release input {}", source.display()),
        source: error,
    })? {
        let entry = entry.map_err(|error| PackageError::Io {
            context: format!("cannot enumerate release input {}", source.display()),
            source: error,
        })?;
        let destination = target.join(entry.file_name());
        let file_type = entry.file_type().map_err(|source| PackageError::Io {
            context: format!("cannot inspect release input {}", entry.path().display()),
            source,
        })?;
        if file_type.is_dir() {
            copy_native_directory(&entry.path(), &destination)?;
        } else if file_type.is_file() {
            if should_exclude_from_release_resources(&entry.path()) {
                continue;
            }
            ensure_native_file("release input", &entry.path())?;
            copy_file_to(&entry.path(), &destination)?;
        } else {
            return Err(PackageError::InvalidInput(format!(
                "release input contains a symbolic link or non-regular file: {}",
                entry.path().display()
            )));
        }
    }
    Ok(())
}

fn copy_file_to(source: &Path, destination: &Path) -> Result<(), PackageError> {
    ensure_native_file("release input", source)?;
    let parent = destination.parent().ok_or_else(|| {
        PackageError::InvalidInput(format!(
            "release destination has no parent: {}",
            destination.display()
        ))
    })?;
    fs::create_dir_all(parent).map_err(|source| PackageError::Io {
        context: format!("cannot create release directory {}", parent.display()),
        source,
    })?;
    fs::copy(source, destination).map_err(|source_error| PackageError::Io {
        context: format!(
            "cannot copy {} to {}",
            source.display(),
            destination.display()
        ),
        source: source_error,
    })?;
    Ok(())
}

fn file_name(path: &Path) -> Result<String, PackageError> {
    path.file_name()
        .and_then(OsStr::to_str)
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            PackageError::InvalidInput(format!("path has no valid file name: {}", path.display()))
        })
}

fn native_binary_name(expected_stem: &str, source: &Path) -> Result<String, PackageError> {
    let extension = source.extension().and_then(OsStr::to_str);
    Ok(match extension {
        Some(extension) if !extension.is_empty() => format!("{expected_stem}.{extension}"),
        _ => expected_stem.into(),
    })
}

fn release_client_name(source: &Path) -> Result<String, PackageError> {
    let extension = source.extension().and_then(OsStr::to_str);
    let version = env!("CARGO_PKG_VERSION");
    Ok(match extension {
        Some(extension) if !extension.is_empty() => format!("XRTranslate-v{version}.{extension}"),
        _ => format!("XRTranslate-v{version}"),
    })
}

fn staging_path(output: &Path) -> Result<PathBuf, PackageError> {
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| {
            PackageError::InvalidInput(format!(
                "release output must have a parent directory: {}",
                output.display()
            ))
        })?;
    fs::create_dir_all(parent).map_err(|source| PackageError::Io {
        context: format!("cannot create release output parent {}", parent.display()),
        source,
    })?;
    let file_name = file_name(output)?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            PackageError::InvalidInput(format!("system clock cannot build staging path: {error}"))
        })?
        .as_nanos();
    let staging = parent.join(format!(
        ".{file_name}.staging-{}-{nonce}",
        std::process::id()
    ));
    if staging.exists() {
        return Err(PackageError::InvalidInput(format!(
            "refusing to reuse existing staging path {}",
            staging.display()
        )));
    }
    Ok(staging)
}

trait AssetIter {
    fn iter(&self) -> [&xrtranslate_assets::ResolvedModelAsset; 2];
}

impl AssetIter for ResolvedModelAssets {
    fn iter(&self) -> [&xrtranslate_assets::ResolvedModelAsset; 2] {
        [&self.qwen3_asr, &self.hunyuan_mt]
    }
}
