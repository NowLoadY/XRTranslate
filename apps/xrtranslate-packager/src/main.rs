//! XRTranslate release packaging.
//! It never scans nor copies the repository's Python backend, server, launch
//! scripts, requirements, or llama.cpp binaries.

#![forbid(unsafe_code)]

use std::{
    error::Error,
    ffi::OsStr,
    fmt, fs, io,
    io::Read,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use clap::Parser;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use xr_corpus_core::validate_seed_database;
use xrtranslate_assets::{
    ModelAssetId, ModelAssetManifest, ModelAssetsConfig, ResolvedModelAssets,
};
use xrtranslate_config::{AppConfig, RuntimeLayout};

const RELEASE_LAYOUT_VERSION: u32 = 4;
const VAD_RELATIVE_PATH: &str = RuntimeLayout::VAD_MODEL_PATH;
const VAD_MODEL_VERSION: &str = "v6.2.1";
const VAD_MODEL_BYTES: u64 = RuntimeLayout::VAD_MODEL_BYTES;
const VAD_MODEL_SHA256: &str = RuntimeLayout::VAD_MODEL_SHA256;
const SPEAKER_RELATIVE_PATH: &str = RuntimeLayout::SPEAKER_MODEL_PATH;
const SPEAKER_MODEL_BYTES: u64 = RuntimeLayout::SPEAKER_MODEL_BYTES;
const SPEAKER_MODEL_SHA256: &str = RuntimeLayout::SPEAKER_MODEL_SHA256;
const DENOISE_RELATIVE_PATH: &str = RuntimeLayout::DENOISE_MODEL_PATH;
const DENOISE_MODEL_BYTES: u64 = RuntimeLayout::DENOISE_MODEL_BYTES;
const DENOISE_MODEL_SHA256: &str = RuntimeLayout::DENOISE_MODEL_SHA256;
const INTERNAL_BIN_DIRECTORY: &str = "bin";
const ONNX_LICENSE_RELATIVE_PATH: &str = "licenses/onnxruntime/LICENSE";
const ONNX_NOTICES_RELATIVE_PATH: &str = "licenses/onnxruntime/ThirdPartyNotices.txt";

#[derive(Debug)]
struct VcRuntimeFile {
    name: &'static str,
    source: PathBuf,
    bytes: u64,
    sha256: String,
}

struct OnnxCpuMetadata {
    path: &'static str,
    bytes: u64,
    sha256: &'static str,
    source_archive: &'static str,
    license_bytes: u64,
    license_sha256: &'static str,
    notices_bytes: u64,
    notices_sha256: &'static str,
}

fn onnx_cpu_metadata(client: &Path) -> OnnxCpuMetadata {
    if client
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
    {
        OnnxCpuMetadata {
            path: "runtime/onnxruntime/cpu/onnxruntime.dll",
            bytes: RuntimeLayout::ONNX_CPU_CORE_WIN_BYTES,
            sha256: RuntimeLayout::ONNX_CPU_CORE_WIN_SHA256,
            source_archive: RuntimeLayout::ONNX_CPU_CORE_WIN_SOURCE_ARCHIVE,
            license_bytes: 1_094,
            license_sha256: "c250d6278f0b47a6439fb7592b08b58a55eb9f535aa49a1db63211c3f982b674",
            notices_bytes: 331_175,
            notices_sha256: "fb0af774b4d7cffc5b9d046f2aaeade2f37df2f80abf8033c95dfffcc77a8866",
        }
    } else {
        OnnxCpuMetadata {
            path: "runtime/onnxruntime/cpu/libonnxruntime.so.1.28.0",
            bytes: RuntimeLayout::ONNX_CPU_CORE_LINUX_BYTES,
            sha256: RuntimeLayout::ONNX_CPU_CORE_LINUX_SHA256,
            source_archive: "onnxruntime-linux-x64-1.28.0.tgz",
            license_bytes: 1_073,
            license_sha256: "2f07c72751aed99790b8a4869cf2311df85a860b22ded05fa22803587a48922c",
            notices_bytes: 325_054,
            notices_sha256: "0e07b95f3a8d6230037707c5c4a2b554d12c4cb67369669ac255635528ffcee2",
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
    /// Include already installed, hash-verified Qwen3-ASR and Hy-MT2 GGUF models.
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
    verify_file_integrity(
        "--vad-model",
        &plan.vad_model,
        VAD_MODEL_BYTES,
        VAD_MODEL_SHA256,
    )?;
    verify_file_integrity(
        "--speaker-model",
        &plan.speaker_model,
        SPEAKER_MODEL_BYTES,
        SPEAKER_MODEL_SHA256,
    )?;
    verify_file_integrity(
        "--denoise-model",
        &plan.denoise_model,
        DENOISE_MODEL_BYTES,
        DENOISE_MODEL_SHA256,
    )?;
    verify_file_integrity(
        "--onnx-runtime-cpu",
        &plan.onnx_runtime_cpu,
        onnx.bytes,
        onnx.sha256,
    )?;
    verify_file_integrity(
        "--onnx-runtime-license",
        &plan.onnx_runtime_license,
        onnx.license_bytes,
        onnx.license_sha256,
    )?;
    verify_file_integrity(
        "--onnx-runtime-notices",
        &plan.onnx_runtime_notices,
        onnx.notices_bytes,
        onnx.notices_sha256,
    )?;
    if plan.output.exists() {
        return Err(PackageError::InvalidInput(format!(
            "refusing to overwrite existing release output {}",
            plan.output.display()
        ))
        .into());
    }

    if plan.include_models {
        plan.assets.verify_integrity().into_result()?;
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
                    "sha256": file.sha256,
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
                "sha256": onnx.sha256,
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
            "sha256": VAD_MODEL_SHA256,
        },
        "speaker_model": {
            "path": SPEAKER_RELATIVE_PATH,
            "architecture": "ERes2NetV2",
            "source": {
                "repository": "iic/speech_eres2netv2_sv_zh-cn_16k-common",
                "revision": "v1.0.1",
            },
            "bytes": SPEAKER_MODEL_BYTES,
            "sha256": SPEAKER_MODEL_SHA256,
        },
        "denoise_model": {
            "path": DENOISE_RELATIVE_PATH,
            "architecture": "GTCRN-Light-v3",
            "source": {
                "repository": "k2-fsa/sherpa-onnx",
                "release": "speech-enhancement-models",
            },
            "bytes": DENOISE_MODEL_BYTES,
            "sha256": DENOISE_MODEL_SHA256,
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
            "sha256": file.sha256,
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
            sha256: format!("{:x}", Sha256::digest(&bytes)),
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
            verify_file_integrity("staged VC runtime", &destination, file.bytes, &file.sha256)?;
        }
    }
    Ok(())
}

fn verify_file_integrity(
    label: &str,
    path: &Path,
    expected_bytes: u64,
    expected_sha256: &str,
) -> Result<(), PackageError> {
    let metadata = fs::metadata(path).map_err(|source| PackageError::Io {
        context: format!("cannot inspect {label} at {}", path.display()),
        source,
    })?;
    if metadata.len() != expected_bytes {
        return Err(PackageError::InvalidInput(format!(
            "{label} has {} bytes, expected {expected_bytes}: {}",
            metadata.len(),
            path.display()
        )));
    }
    let mut file = fs::File::open(path).map_err(|source| PackageError::Io {
        context: format!("cannot open {label} at {}", path.display()),
        source,
    })?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|source| PackageError::Io {
            context: format!("cannot hash {label} at {}", path.display()),
            source,
        })?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    let actual_sha256 = format!("{:x}", digest.finalize());
    if !actual_sha256.eq_ignore_ascii_case(expected_sha256) {
        return Err(PackageError::InvalidInput(format!(
            "{label} SHA-256 mismatch: expected {expected_sha256}, got {actual_sha256}: {}",
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

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use super::*;

    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

    fn temp_directory(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "xrtranslate-packager-{label}-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn write(path: &Path, contents: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn x64_dll_fixture() -> Vec<u8> {
        let mut bytes = vec![0; 512];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[0x3c..0x40].copy_from_slice(&0x80_u32.to_le_bytes());
        bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
        bytes[0x84..0x86].copy_from_slice(&0x8664_u16.to_le_bytes());
        bytes[0x86..0x88].copy_from_slice(&1_u16.to_le_bytes());
        bytes[0x94..0x96].copy_from_slice(&240_u16.to_le_bytes());
        bytes[0x96..0x98].copy_from_slice(&0x2002_u16.to_le_bytes());
        bytes[0x98..0x9a].copy_from_slice(&0x20b_u16.to_le_bytes());
        bytes
    }

    fn write_vc_runtime_fixture(directory: &Path) {
        for &name in RuntimeLayout::WINDOWS_CRT_REQUIRED_FILES {
            write(&directory.join(name), &x64_dll_fixture());
        }
    }

    fn vc_redist_directory(root: &Path) -> PathBuf {
        root.join("VC/Redist/MSVC/14.44.35112/x64/Microsoft.VC143.CRT")
    }

    #[test]
    fn windows_release_requires_complete_x64_release_crt() {
        let root = temp_directory("crt-inputs");
        let directory = vc_redist_directory(&root);
        let client = Path::new("rust-client.exe");
        assert!(windows_vc_runtime(client, None).is_err());
        assert!(
            windows_vc_runtime(Path::new("rust-client"), None)
                .unwrap()
                .is_empty()
        );
        write_vc_runtime_fixture(&directory);
        assert_eq!(
            windows_vc_runtime(client, Some(&directory)).unwrap().len(),
            RuntimeLayout::WINDOWS_CRT_REQUIRED_FILES.len()
        );
        let required = directory.join("msvcp140_1.dll");
        fs::remove_file(&required).unwrap();
        assert!(windows_vc_runtime(client, Some(&directory)).is_err());

        let mut x86_dll = x64_dll_fixture();
        x86_dll[0x84..0x86].copy_from_slice(&0x14c_u16.to_le_bytes());
        write(&required, &x86_dll);
        assert!(windows_vc_runtime(client, Some(&directory)).is_err());
        write(&required, b"not a PE file");
        assert!(windows_vc_runtime(client, Some(&directory)).is_err());
        write(&required, &x64_dll_fixture()[..256]);
        assert!(windows_vc_runtime(client, Some(&directory)).is_err());

        let debug_directory = directory.with_file_name("Microsoft.VC143.DebugCRT");
        write_vc_runtime_fixture(&debug_directory);
        assert!(windows_vc_runtime(client, Some(&debug_directory)).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn vc_runtime_input_rejects_system_directory_even_with_valid_x64_dlls() {
        let root = temp_directory("crt-system-directory");
        let directory = root.join("Windows/System32");
        write_vc_runtime_fixture(&directory);
        let error = windows_vc_runtime(Path::new("client.exe"), Some(&directory)).unwrap_err();
        assert!(error.to_string().contains("Visual Studio"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn vc_runtime_is_staged_beside_gui_and_helpers_without_unlisted_files() {
        let root = temp_directory("crt-layout");
        let directory = vc_redist_directory(&root);
        let staging = root.join("release");
        write_vc_runtime_fixture(&directory);
        write(&directory.join("concrt140.dll"), &x64_dll_fixture());
        for name in [
            "msvcp140d.dll",
            "kernel32.dll",
            "personal.txt",
            "subdir/helper.dll",
        ] {
            write(&directory.join(name), b"must stay out of the release");
        }
        let files = windows_vc_runtime(Path::new("client.exe"), Some(&directory)).unwrap();
        copy_vc_runtime(&files, &staging).unwrap();

        assert_eq!(
            files.len(),
            RuntimeLayout::WINDOWS_CRT_REQUIRED_FILES.len() + 1
        );
        for target in [&staging, &staging.join(INTERNAL_BIN_DIRECTORY)] {
            for file in &files {
                assert_eq!(fs::read(target.join(file.name)).unwrap(), x64_dll_fixture());
            }
            for name in ["msvcp140d.dll", "kernel32.dll", "personal.txt", "subdir"] {
                assert!(!target.join(name).exists());
            }
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rewrite_config_clears_runtime_path_and_makes_models_release_relative() {
        let root = temp_directory("config");
        let config = root.join("config.json");
        write(&config, br#"{"model_manager":{"llama_server_path":"C:/old/llama-server.exe","models_directory":"C:/old/models","qwen3_asr_gguf_directory":"C:/old/qwen"},"tts":{"provider":"openvoice"},"ocr":{"provider":"paddle-ocr-vl"}}"#);

        let rewritten: Value = serde_json::from_str(&rewrite_config(&config).unwrap()).unwrap();
        assert_eq!(rewritten["model_manager"]["llama_server_path"], "");
        assert_eq!(rewritten["model_manager"]["runtime_directory"], "runtime");
        assert_eq!(rewritten["model_manager"]["models_directory"], "models");
        assert_eq!(rewritten["speaker"]["enabled"], true);
        assert_eq!(rewritten["speaker"]["model_path"], SPEAKER_RELATIVE_PATH);
        assert_eq!(
            rewritten["prompt_context"]["database_path"],
            CORPUS_DATABASE_PATH
        );
        assert_eq!(
            rewritten["prompt_context"]["seed_database_path"],
            CORPUS_SEED_PATH
        );
        assert!(
            rewritten["model_manager"]
                .get("qwen3_asr_gguf_directory")
                .is_none()
        );
        assert_eq!(rewritten["tts"]["provider"], "none");
        assert_eq!(rewritten["ocr"]["provider"], "none");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_input_rejects_python_files() {
        let root = temp_directory("python");
        let directory = root.join("resources");
        write(&directory.join("helper.py"), b"print('python')");
        assert!(ensure_directory_is_native("fixture", &directory).is_err());
        fs::remove_file(directory.join("helper.py")).unwrap();
        write(
            &directory.join("backend/native-looking-file.txt"),
            b"still forbidden",
        );
        assert!(ensure_directory_is_native("fixture", &directory).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dry_release_layout_contains_only_allowed_native_inputs() {
        let root = temp_directory("layout");
        let source = root.join("source");
        let output = root.join("release");
        let extension = if cfg!(windows) { ".exe" } else { "" };
        let client = source.join(format!("rust-client{extension}"));
        let backend = source.join(format!("custom-backend{extension}"));
        let corpus_server = source.join(format!("custom-corpus{extension}"));
        let installer = source.join(format!("custom-installer{extension}"));
        let updater = source.join(format!("custom-updater{extension}"));
        let resources = source.join("resources");
        let seed_database = source.join("default.sqlite");
        let vad = source.join("silero_vad.onnx");
        let speaker = source.join("speaker_embedding.onnx");
        let denoise = source.join("gtcrn_simple.onnx");
        let onnx_runtime_cpu = source.join("onnxruntime.dll");
        let onnx_runtime_license = source.join("onnxruntime-LICENSE");
        let onnx_runtime_notices = source.join("onnxruntime-ThirdPartyNotices.txt");
        let vc_runtime_dir = cfg!(windows).then(|| vc_redist_directory(&source));
        if let Some(directory) = &vc_runtime_dir {
            write_vc_runtime_fixture(directory);
        }
        let config = source.join("config.json");
        let license = source.join("LICENSE");
        write(&client, b"client");
        write(&backend, b"backend");
        write(&corpus_server, b"corpus");
        write(&installer, b"installer");
        write(&updater, b"updater");
        write(&resources.join("docs/welcome.md"), b"native resource");
        write(&resources.join("mpv.def"), b"build-time exports");
        write(&resources.join("bin/mpv-2.dll"), b"runtime download");
        write(
            &resources.join("bin/future-runtime.dll"),
            b"future runtime download",
        );
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../XR-Corpus/corpora/default.sqlite"),
            &seed_database,
        )
        .unwrap();
        write(&vad, b"onnx");
        write(&speaker, b"onnx");
        write(&denoise, b"onnx");
        write(&onnx_runtime_cpu, b"onnx runtime");
        write(&onnx_runtime_license, b"onnx license");
        write(&onnx_runtime_notices, b"onnx notices");
        write(&config, br#"{"model_manager":{"llama_server_path":"old"},"ocr":{"provider":"paddle-ocr-vl","providers":{"paddle-ocr-vl":{"transport":"local","model_asset":"paddle-ocr-vl-1.6-gguf"}}}}"#);
        write(
            &source.join("models/paddle-ocr-vl/PaddleOCR-VL-1.6/PaddleOCR-VL-1.6-GGUF.gguf"),
            b"optional OCR model",
        );
        write(&license, b"AGPL-3.0-only");
        write(&source.join("runtime/debug.md"), b"hello\nlocal only");
        write(
            &source.join("runtime/voice_clones/xrtranslate_microphone.wav"),
            b"private recording",
        );
        write(
            &source.join("runtime/voice_clones/selection.json"),
            br#"{"builtin_id":"miku"}"#,
        );
        write(
            &source.join("runtime/recordings/personal.wav"),
            b"private recording",
        );
        write(
            &source.join("runtime/voice_clones/cards/ab/abcdef/reference.wav"),
            b"private card audio",
        );

        let plan = ReleasePlan::from_arguments(Arguments {
            rust_client_bin: client,
            backend_bin: backend,
            corpus_bin: corpus_server,
            installer_bin: installer,
            updater_bin: updater,
            config,
            resources_dir: resources,
            seed_database,
            license,
            vad_model: Some(vad),
            speaker_model: Some(speaker),
            denoise_model: Some(denoise),
            onnx_runtime_cpu,
            onnx_runtime_license,
            onnx_runtime_notices,
            vc_runtime_dir,
            output: output.clone(),
            include_models: false,
            check: false,
        })
        .unwrap();
        package(&plan).unwrap();

        let packaged: Value =
            serde_json::from_slice(&fs::read(output.join("config.json")).unwrap()).unwrap();
        assert_eq!(packaged["ocr"]["provider"], "none");
        assert!(!output.join("models/paddle-ocr-vl").exists());
        assert!(output.join("LICENSE").is_file());
        let version = env!("CARGO_PKG_VERSION");
        assert!(
            output
                .join(format!("XRTranslate-v{version}{extension}"))
                .is_file()
        );
        assert!(
            output
                .join(INTERNAL_BIN_DIRECTORY)
                .join(format!("xrtranslate-backend{extension}"))
                .is_file()
        );
        assert!(
            output
                .join(INTERNAL_BIN_DIRECTORY)
                .join(format!("xr-corpus-server{extension}"))
                .is_file()
        );
        assert!(
            output
                .join(INTERNAL_BIN_DIRECTORY)
                .join(format!("xrtranslate-installer{extension}"))
                .is_file()
        );
        assert!(
            output
                .join(INTERNAL_BIN_DIRECTORY)
                .join(format!("xrtranslate-updater{extension}"))
                .is_file()
        );
        assert!(output.join("resources/docs/welcome.md").is_file());
        assert!(!output.join("resources/mpv.def").exists());
        assert!(!output.join("resources/bin/mpv-2.dll").exists());
        assert!(!output.join("resources/bin/future-runtime.dll").exists());
        assert!(output.join(CORPUS_SEED_PATH).is_file());
        assert!(!output.join(CORPUS_DATABASE_PATH).exists());
        assert!(output.join(VAD_RELATIVE_PATH).is_file());
        assert!(output.join(SPEAKER_RELATIVE_PATH).is_file());
        assert!(output.join(DENOISE_RELATIVE_PATH).is_file());
        assert!(
            output
                .join(onnx_cpu_metadata(&plan.rust_client_bin).path)
                .is_file()
        );
        assert!(output.join(ONNX_LICENSE_RELATIVE_PATH).is_file());
        assert!(output.join(ONNX_NOTICES_RELATIVE_PATH).is_file());
        assert!(output.join("release-manifest.json").is_file());
        assert!(!output.join("runtime/llama.cpp").exists());
        assert!(!output.join("runtime/debug.md").exists());
        assert!(!output.join("runtime/voice_clones").exists());
        assert!(!output.join("runtime/recordings").exists());
        for voice in xrtranslate_assets::voices::BUILTIN_VOICES {
            let directory = output.join("resources/voices").join(voice.id);
            assert_eq!(
                fs::read(directory.join("reference.wav")).unwrap(),
                voice.wav
            );
            assert_eq!(
                fs::read_to_string(directory.join("reference.txt")).unwrap(),
                voice.transcript
            );
            assert_eq!(
                fs::read_to_string(directory.join("LICENSE")).unwrap(),
                voice.license
            );
            assert_eq!(
                fs::read_to_string(directory.join("SOURCE.md")).unwrap(),
                voice.source_notice
            );
        }
        assert!(!output.join("backend").exists());
        assert!(!output.join("server").exists());
        assert!(!output.join("main.py").exists());
        assert!(!output.join("requirements.txt").exists());
        let manifest: Value = serde_json::from_str(
            &fs::read_to_string(output.join("release-manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["python"], false);
        assert_eq!(manifest["runtime"]["included"], true);
        assert_eq!(manifest["runtime"]["msvc"]["included"], cfg!(windows));
        if cfg!(windows) {
            let files = manifest["runtime"]["msvc"]["files"].as_array().unwrap();
            assert_eq!(files.len(), RuntimeLayout::WINDOWS_CRT_REQUIRED_FILES.len());
            for file in files {
                for path in file["paths"].as_array().unwrap() {
                    assert_eq!(
                        fs::read(output.join(path.as_str().unwrap())).unwrap(),
                        x64_dll_fixture()
                    );
                }
                assert_eq!(file["architecture"], "x86_64");
                assert_eq!(file["bytes"], 512);
                assert_eq!(
                    file["sha256"],
                    format!("{:x}", Sha256::digest(x64_dll_fixture()))
                );
            }
            assert!(!manifest.to_string().contains("Microsoft.VC143.CRT"));
        }
        assert_eq!(
            manifest["runtime"]["onnx_cpu"]["path"],
            onnx_cpu_metadata(&plan.rust_client_bin).path
        );
        assert_eq!(manifest["runtime"]["onnx_cuda"]["included"], false);
        assert_eq!(
            manifest["runtime"]["onnx_cuda"]["selection_marker"],
            RuntimeLayout::NATIVE_RUNTIME_SELECTION_FILE
        );
        assert_eq!(manifest["vad_model"]["path"], VAD_RELATIVE_PATH);
        assert_eq!(
            manifest["vad_model"]["source"]["revision"],
            VAD_MODEL_VERSION
        );
        assert_eq!(manifest["vad_model"]["bytes"], VAD_MODEL_BYTES);
        assert_eq!(manifest["vad_model"]["sha256"], VAD_MODEL_SHA256);
        assert_eq!(manifest["speaker_model"]["path"], SPEAKER_RELATIVE_PATH);
        assert_eq!(manifest["denoise_model"]["path"], DENOISE_RELATIVE_PATH);
        assert_eq!(
            manifest["entrypoints"]["client"],
            format!("XRTranslate-v{version}{extension}")
        );
        assert_eq!(manifest["models"]["included"], false);
        assert_eq!(manifest["corpora"]["database_path"], CORPUS_DATABASE_PATH);
        assert_eq!(manifest["corpora"]["seed_database_path"], CORPUS_SEED_PATH);
        assert_eq!(
            manifest["entrypoints"]["updater"],
            format!("{INTERNAL_BIN_DIRECTORY}/xrtranslate-updater{extension}")
        );
        fs::remove_dir_all(root).unwrap();
    }
}
