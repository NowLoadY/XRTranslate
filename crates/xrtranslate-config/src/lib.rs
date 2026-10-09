//! Native configuration reader for the project-root `config.json`.
//!
//! The typed fields cover the settings needed by the native backend. The full
//! parsed document remains available through [`AppConfig::raw`], so optional
//! provider settings can evolve without forcing this crate to model each one.

#![forbid(unsafe_code)]

mod asr_migration;
mod language;
mod models;

pub use models::InstalledModel;

use std::{
    error::Error,
    fmt, fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
pub use xr_corpus_core::CorpusConfig as PromptContextConfig;

/// A map of provider-specific settings retained without imposing a model
/// schema on optional providers.
pub type ProviderConfigs = Map<String, Value>;

/// Stable on-disk layout for managed native runtimes.
///
/// This is deliberately limited to path resolution. Archive selection and
/// executable validation remain owned by the installer/backend layers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeLayout {
    project_root: PathBuf,
    runtime_root: PathBuf,
}

/// Persisted host selection consumed before the native backend is spawned.
/// Paths are stored relative to the project root so a packaged installation
/// remains movable and no process needs to mutate the system `PATH`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativeRuntimeSelection {
    pub schema_version: u32,
    /// Effective ONNX backend retained for schema-v1 consumers.
    pub backend: NativeRuntimeBackend,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llama_cpp_backend: Option<NativeRuntimeBackend>,
    /// Physical Vulkan adapter index, selected by the host before launch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vulkan_device: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub onnx_backend: Option<NativeRuntimeBackend>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cuda_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_dir: Option<PathBuf>,
    /// ONNX Runtime core selected for this process. CUDA markers point to the
    /// core from the same official archive as their execution providers;
    /// CPU markers point to the compact core shipped with the application.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub onnx_core_library: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cuda_bin_dir: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cudnn_bin_dir: Option<PathBuf>,
    /// Exact preload order for CUDA dependency libraries. ONNX provider DLLs
    /// are loaded by the colocated ONNX Runtime core and must not be preloaded
    /// directly.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preload_libraries: Vec<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NativeRuntimeBackend {
    Cpu,
    Cuda,
    Vulkan,
}

impl NativeRuntimeBackend {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Cuda => "CUDA",
            Self::Vulkan => "Vulkan",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedNativeRuntimeSelection {
    pub backend: NativeRuntimeBackend,
    pub llama_cpp_backend: Option<NativeRuntimeBackend>,
    pub vulkan_device: Option<u32>,
    pub onnx_backend: Option<NativeRuntimeBackend>,
    pub cuda_version: Option<String>,
    pub provider_dir: Option<PathBuf>,
    pub onnx_core_library: Option<PathBuf>,
    pub cuda_bin_dir: Option<PathBuf>,
    pub cudnn_bin_dir: Option<PathBuf>,
    pub preload_libraries: Vec<PathBuf>,
    pub fallback_reason: Option<String>,
}

impl RuntimeLayout {
    pub const DEFAULT_RUNTIME_DIRECTORY: &'static str = "runtime";
    pub const LLAMA_CPP_DIRECTORY: &'static str = "runtime/llama.cpp";
    pub const CUDA_RUNTIME_DIRECTORY: &'static str = "runtime/cuda";
    pub const CUDNN_RUNTIME_DIRECTORY: &'static str = "runtime/cudnn";
    pub const ONNX_RUNTIME_DIRECTORY: &'static str = "runtime/onnxruntime";
    pub const ONNX_CPU_RUNTIME_DIRECTORY: &'static str = "runtime/onnxruntime/cpu";
    /// Immutable Windows application resources, supplied from Visual Studio's
    /// x64 release CRT redistributable directory before any app code can run.
    /// They ship beside the GUI and each helper in `bin`, outside managed model
    /// downloads and backend selection. Debug CRT and OS DLLs are never bundled.
    pub const WINDOWS_CRT_REQUIRED_FILES: &'static [&'static str] = &[
        "msvcp140.dll",
        "msvcp140_1.dll",
        "vcruntime140.dll",
        "vcruntime140_1.dll",
    ];
    /// Additional redistributable components available in some VS 14.x releases.
    pub const WINDOWS_CRT_OPTIONAL_FILES: &'static [&'static str] = &[
        "concrt140.dll",
        "msvcp140_2.dll",
        "msvcp140_atomic_wait.dll",
        "msvcp140_codecvt_ids.dll",
        "vccorlib140.dll",
        "vcruntime140_threads.dll",
    ];
    #[cfg(windows)]
    pub const ONNX_CORE_LIBRARY: &'static str = "onnxruntime.dll";
    #[cfg(not(windows))]
    pub const ONNX_CORE_LIBRARY: &'static str = "libonnxruntime.so.1.28.0";
    pub const NATIVE_RUNTIME_SELECTION_FILE: &'static str = "runtime/native-runtime.json";
    pub const VOICE_CLONES_DIRECTORY: &'static str = "runtime/voice_clones";

    pub const VAD_MODEL_PATH: &'static str =
        "models/silero-vad/src/silero_vad/data/silero_vad.onnx";
    pub const VAD_MODEL_BYTES: u64 = 2_327_524;

    pub const SPEAKER_MODEL_PATH: &'static str =
        "models/3D-Speaker-ERes2NetV2/speaker_embedding.onnx";
    pub const SPEAKER_MODEL_BYTES: u64 = 71_964_309;

    pub const DENOISE_MODEL_PATH: &'static str = "models/gtcrn/gtcrn_simple.onnx";
    pub const DENOISE_MODEL_BYTES: u64 = 535_638;

    pub const ONNX_CPU_CORE_WIN_BYTES: u64 = 16_277_856;
    /// The official CUDA archive also supplies the independently usable CPU core.
    pub const ONNX_CPU_CORE_WIN_SOURCE_ARCHIVE: &'static str =
        "onnxruntime-win-x64-gpu_cuda13-1.28.0.zip";

    pub const ONNX_CPU_CORE_LINUX_BYTES: u64 = 24_268_848;

    #[must_use]
    pub fn for_project_root(project_root: impl AsRef<Path>) -> Self {
        Self::new(project_root, None::<&Path>)
    }

    #[must_use]
    pub fn new(
        project_root: impl AsRef<Path>,
        runtime_directory: Option<impl AsRef<Path>>,
    ) -> Self {
        let project_root = project_root.as_ref().to_path_buf();
        let runtime_directory = runtime_directory.filter(|_| !cfg!(target_os = "android"));
        let runtime_root = match runtime_directory {
            Some(dir) => {
                let dir = normalized_runtime_root(dir.as_ref());
                if dir.is_absolute() {
                    dir.to_path_buf()
                } else {
                    project_root.join(dir)
                }
            }
            None => project_root.join(Self::DEFAULT_RUNTIME_DIRECTORY),
        };
        Self {
            project_root,
            runtime_root,
        }
    }

    #[must_use]
    pub fn for_config(project_root: impl AsRef<Path>, config: &ModelManagerConfig) -> Self {
        Self::new(project_root, config.runtime_directory.as_deref())
    }

    #[must_use]
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    #[must_use]
    pub fn runtime_root(&self) -> &Path {
        &self.runtime_root
    }

    #[must_use]
    pub fn llama_cpp_directory(&self) -> PathBuf {
        self.runtime_root.join("llama.cpp")
    }

    #[must_use]
    pub fn cuda_runtime_directory(&self, cuda_version: &str) -> PathBuf {
        self.runtime_root.join("cuda").join(cuda_version)
    }

    #[must_use]
    pub fn cudnn_runtime_directory(&self, cuda_major: &str) -> PathBuf {
        self.runtime_root.join("cudnn").join(cuda_major)
    }

    #[must_use]
    pub fn onnx_runtime_directory(&self, cuda_version: &str) -> PathBuf {
        self.runtime_root
            .join("onnxruntime")
            .join(format!("cuda-{cuda_version}"))
    }

    #[must_use]
    pub fn onnx_cpu_runtime_directory(&self) -> PathBuf {
        self.runtime_root.join("onnxruntime").join("cpu")
    }

    #[must_use]
    pub fn onnx_cpu_core_library(&self) -> PathBuf {
        self.onnx_cpu_runtime_directory()
            .join(Self::ONNX_CORE_LIBRARY)
    }

    #[must_use]
    pub fn silero_vad_model_path(&self) -> PathBuf {
        self.project_root.join(Self::VAD_MODEL_PATH)
    }

    #[must_use]
    pub fn gtcrn_model_path(&self) -> PathBuf {
        self.project_root.join(Self::DENOISE_MODEL_PATH)
    }

    #[must_use]
    pub fn speaker_embedding_model_path(&self) -> PathBuf {
        self.project_root.join(Self::SPEAKER_MODEL_PATH)
    }

    #[must_use]
    pub fn native_runtime_selection_file(&self) -> PathBuf {
        self.runtime_root.join("native-runtime.json")
    }

    #[must_use]
    pub fn voice_clones_directory(&self) -> PathBuf {
        self.runtime_root.join("voice_clones")
    }

    #[must_use]
    pub fn resolve_native_runtime_selection(
        &self,
        selection: &NativeRuntimeSelection,
    ) -> ResolvedNativeRuntimeSelection {
        let resolve =
            |path: &Option<PathBuf>| path.as_ref().map(|path| self.resolve_configured_path(path));
        ResolvedNativeRuntimeSelection {
            backend: selection.backend,
            llama_cpp_backend: selection.llama_cpp_backend,
            vulkan_device: selection.vulkan_device,
            onnx_backend: selection.onnx_backend,
            cuda_version: selection.cuda_version.clone(),
            provider_dir: resolve(&selection.provider_dir),
            onnx_core_library: resolve(&selection.onnx_core_library),
            cuda_bin_dir: resolve(&selection.cuda_bin_dir),
            cudnn_bin_dir: resolve(&selection.cudnn_bin_dir),
            preload_libraries: selection
                .preload_libraries
                .iter()
                .map(|path| self.resolve_configured_path(path))
                .collect(),
            fallback_reason: selection.fallback_reason.clone(),
        }
    }

    /// Resolves a config path against the config/project root while preserving
    /// explicit absolute paths for existing manual installations.
    #[must_use]
    pub fn resolve_configured_path(&self, configured: impl AsRef<Path>) -> PathBuf {
        let configured = configured.as_ref();
        if configured.is_absolute() {
            configured.to_path_buf()
        } else {
            self.project_root.join(configured)
        }
    }

    /// Resolves the one shared managed executable name on Linux and Windows.
    /// Earlier Windows defaults included `.exe`; both spellings map to the
    /// platform executable only inside the managed runtime directory.
    #[must_use]
    pub fn resolve_llama_server_path(&self, configured: impl AsRef<Path>) -> PathBuf {
        let resolved = self.resolve_configured_path(configured);
        let managed = self.managed_llama_server("llama-server");
        if resolved == managed || resolved == managed.with_extension("exe") {
            self.managed_llama_server(format!("llama-server{}", std::env::consts::EXE_SUFFIX))
        } else {
            resolved
        }
    }

    #[must_use]
    pub fn managed_llama_server(&self, executable: impl AsRef<Path>) -> PathBuf {
        self.llama_cpp_directory().join(executable)
    }

    /// Returns a stable config value: managed files are stored relative to the
    /// project root, while manually selected external files remain absolute.
    #[must_use]
    pub fn config_path_for(&self, path: impl AsRef<Path>) -> PathBuf {
        let path = path.as_ref();
        path.strip_prefix(&self.project_root)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| path.to_path_buf())
    }

    /// Returns the writable user override document for model/provider
    /// settings. The updater preserves runtime on every supported platform.
    #[must_use]
    pub fn user_config_path(project_root: impl AsRef<Path>) -> PathBuf {
        project_root
            .as_ref()
            .join("runtime")
            .join("user-config.json")
    }

    /// Location used by older packaged builds before the runtime directory
    /// became the single cross-platform owner of mutable model settings.
    fn legacy_user_config_path(project_root: impl AsRef<Path>) -> PathBuf {
        let directory = if cfg!(windows) {
            std::env::var_os("LOCALAPPDATA")
                .or_else(|| std::env::var_os("APPDATA"))
                .map(PathBuf::from)
        } else {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config"))
                })
        };
        directory
            .unwrap_or_else(|| project_root.as_ref().join("runtime"))
            .join("XRTranslate")
            .join("user-config.json")
    }
}

/// Older welcome builds accidentally persisted the managed llama-server
/// executable as `runtime_directory`. Keep those user configs usable without
/// treating the executable as a directory or requiring a manual reset.
fn normalized_runtime_root(path: &Path) -> &Path {
    let is_llama_server = path.file_stem().is_some_and(|name| name == "llama-server")
        && path
            .extension()
            .is_none_or(|extension| extension.eq_ignore_ascii_case("exe"));
    let managed_directory = path
        .parent()
        .filter(|parent| parent.file_name().is_some_and(|name| name == "llama.cpp"));
    if is_llama_server && let Some(runtime_root) = managed_directory.and_then(Path::parent) {
        runtime_root
    } else {
        path
    }
}

/// Loads the immutable project defaults and applies the writable user
/// override document with recursive object merging.
pub fn load_user_config_document(
    base_path: impl AsRef<Path>,
    project_root: impl AsRef<Path>,
) -> Result<Value, ConfigError> {
    let legacy_path =
        (!cfg!(debug_assertions)).then(|| RuntimeLayout::legacy_user_config_path(&project_root));
    load_user_config_document_with_legacy(base_path, project_root, legacy_path)
}

fn load_user_config_document_with_legacy(
    base_path: impl AsRef<Path>,
    project_root: impl AsRef<Path>,
    legacy_path: Option<PathBuf>,
) -> Result<Value, ConfigError> {
    let base_path = base_path.as_ref();
    let contents = fs::read_to_string(base_path).map_err(|source| ConfigError::Read {
        path: base_path.to_path_buf(),
        source,
    })?;
    let mut document: Value = serde_json::from_str(&contents).map_err(ConfigError::InvalidJson)?;
    asr_migration::migrate_qwen_asr_defaults(&mut document);
    let override_path = RuntimeLayout::user_config_path(&project_root);
    let migrate_from = legacy_path
        .as_ref()
        .filter(|path| path.is_file() && !override_path.is_file());
    for path in migrate_from
        .into_iter()
        .chain(std::iter::once(&override_path))
    {
        if path.is_file() {
            let contents = fs::read_to_string(path).map_err(|source| ConfigError::Read {
                path: path.clone(),
                source,
            })?;
            let mut overlay: Value =
                serde_json::from_str(&contents).map_err(ConfigError::InvalidJson)?;
            migrate_legacy_openai_asr_model(&mut overlay);
            asr_migration::migrate_qwen_asr_defaults(&mut overlay);
            merge_config_values(&mut document, overlay);
        }
    }
    migrate_legacy_openai_asr_model(&mut document);
    if let Some(legacy_path) = legacy_path
        .as_ref()
        .filter(|path| path.is_file() && *path != &override_path)
    {
        if migrate_from.is_some() {
            save_user_config_document(base_path, &project_root, &document)
                .map_err(ConfigError::Migration)?;
        }
        fs::remove_file(&legacy_path).map_err(|source| ConfigError::Write {
            path: legacy_path.clone(),
            source,
        })?;
    }
    Ok(document)
}

fn migrate_legacy_openai_asr_model(document: &mut Value) -> bool {
    let Some(provider) = document
        .pointer_mut("/asr/providers/openai")
        .and_then(Value::as_object_mut)
    else {
        return false;
    };
    let deprecated = provider.get("model").and_then(Value::as_str) == Some("gpt-4o-audio-preview");
    if deprecated {
        provider.insert("model".into(), Value::from("gpt-4o-transcribe"));
    }
    let transcription = provider
        .get("model")
        .and_then(Value::as_str)
        .is_some_and(|model| {
            model.starts_with("gpt-4o-transcribe")
                || model.starts_with("gpt-4o-mini-transcribe")
                || model == "gpt-transcribe"
                || model == "whisper-1"
        });
    let legacy_endpoint = provider.get("url").and_then(Value::as_str)
        == Some("https://api.openai.com/v1/chat/completions");
    if deprecated || (transcription && legacy_endpoint) {
        provider.insert(
            "url".into(),
            Value::from("https://api.openai.com/v1/audio/transcriptions"),
        );
        provider.insert("asr_prompt_mode".into(), Value::from("context_bias"));
        return true;
    }
    false
}

/// Computes the minimal recursive override needed to represent `effective`
/// on top of `base`. Unchanged defaults therefore remain owned by config.json.
#[must_use]
pub fn user_config_override(base: &Value, effective: &Value) -> Option<Value> {
    if base == effective {
        return None;
    }
    match (base, effective) {
        (Value::Object(base), Value::Object(effective)) => {
            let mut changes = Map::new();
            for (key, value) in effective {
                let change = base
                    .get(key)
                    .and_then(|base_value| user_config_override(base_value, value))
                    .or_else(|| (!base.contains_key(key)).then(|| value.clone()));
                if let Some(change) = change {
                    changes.insert(key.clone(), change);
                }
            }
            Some(Value::Object(changes))
        }
        _ => Some(effective.clone()),
    }
}

/// Persists only user changes and leaves the shipped default document intact.
pub fn save_user_config_document(
    base_path: impl AsRef<Path>,
    project_root: impl AsRef<Path>,
    effective: &Value,
) -> Result<(), String> {
    let base_path = base_path.as_ref();
    let base_contents = fs::read_to_string(base_path)
        .map_err(|error| format!("Cannot read {}: {error}", base_path.display()))?;
    let base: Value = serde_json::from_str(&base_contents)
        .map_err(|error| format!("Invalid config.json: {error}"))?;
    let path = RuntimeLayout::user_config_path(project_root);
    match user_config_override(&base, effective) {
        Some(override_document) if !override_document.as_object().is_some_and(Map::is_empty) => {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| format!("Cannot create {}: {error}", parent.display()))?;
            }
            let formatted = serde_json::to_string_pretty(&override_document)
                .map_err(|error| format!("Cannot serialize user configuration: {error}"))?;
            fs::write(&path, format!("{formatted}\n"))
                .map_err(|error| format!("Cannot save {}: {error}", path.display()))?;
        }
        _ => {
            let _ = fs::remove_file(&path);
        }
    }
    Ok(())
}

fn merge_config_values(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Object(base), Value::Object(overlay)) => {
            for (key, value) in overlay {
                if let Some(existing) = base.get_mut(&key) {
                    merge_config_values(existing, value);
                } else {
                    base.insert(key, value);
                }
            }
        }
        (base, overlay) => *base = overlay,
    }
}

/// The parsed native-backend configuration and the complete original JSON.
#[derive(Debug, Clone, PartialEq)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub audio: AudioConfig,
    pub asr: AsrConfig,
    pub denoise: DenoiseConfig,
    pub speaker: SpeakerConfig,
    pub prompt_context: PromptContextConfig,
    pub integrations: IntegrationsConfig,
    pub storage: StorageConfig,
    pub translation: TranslationConfig,
    pub tts: TtsConfig,
    pub ocr: OcrConfig,
    pub model_manager: ModelManagerConfig,
    /// The unmodified parsed document, including sections unknown to this
    /// crate and frontend preferences.
    pub raw: Value,
    /// The source file when this configuration was loaded from disk.
    pub source_path: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RuntimeRequirements {
    pub llama_cpp: bool,
    /// The selected TTS provider uses the in-process ONNX runtime.
    pub onnx_tts: bool,
    /// A selected model uses the shared CPU ONNX runtime.
    pub onnx_cpu: bool,
    /// Managed ONNX model packages require the shared CUDA/cuDNN closure.
    /// Bundled small ONNX components do not participate in this requirement.
    pub onnx_cuda: bool,
    pub missing_api_key: bool,
}

impl AppConfig {
    /// Resolves host runtime prerequisites from every selected provider that
    /// follows the shared `provider` / `providers` / `transport` contract.
    /// New model capabilities therefore participate without UI changes.
    #[must_use]
    pub fn runtime_requirements(&self) -> RuntimeRequirements {
        let mut requirements = RuntimeRequirements::default();
        let Some(root) = self.raw.as_object() else {
            return requirements;
        };
        for (key, section) in root {
            if cfg!(target_os = "android") && key == "ocr" {
                continue;
            }
            let Some(section) = section.as_object() else {
                continue;
            };
            let Some(selected) = section.get("provider").and_then(Value::as_str) else {
                continue;
            };
            if selected == "none" {
                continue;
            }
            let Some(provider) = section
                .get("providers")
                .and_then(Value::as_object)
                .and_then(|providers| providers.get(selected))
                .and_then(Value::as_object)
            else {
                continue;
            };
            match provider.get("transport").and_then(Value::as_str) {
                Some("local") | None => requirements.llama_cpp = true,
                Some("onnx-cpu") => requirements.onnx_cpu = true,
                Some("onnx") => {
                    requirements.onnx_tts = true;
                    requirements.onnx_cuda = true;
                }
                Some(_) => {
                    requirements.missing_api_key |= provider
                        .get("api_key")
                        .and_then(Value::as_str)
                        .is_none_or(|key| key.trim().is_empty());
                }
            }
        }
        requirements
    }

    /// Returns the resolved runtime layout for this configuration.
    #[must_use]
    pub fn runtime_layout(&self, project_root: impl AsRef<Path>) -> RuntimeLayout {
        RuntimeLayout::for_config(project_root, &self.model_manager)
    }

    /// Reads and validates JSON syntax from `path`.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref().to_path_buf();
        let contents = fs::read_to_string(&path).map_err(|source| ConfigError::Read {
            path: path.clone(),
            source,
        })?;
        let mut config = Self::from_json_str(&contents)?;
        config.source_path = Some(path);
        Ok(config)
    }

    /// Reads the project defaults and applies the user override document.
    pub fn from_path_with_user_config(
        path: impl AsRef<Path>,
        project_root: impl AsRef<Path>,
    ) -> Result<Self, ConfigError> {
        let path = path.as_ref().to_path_buf();
        let raw = load_user_config_document(&path, project_root)?;
        let mut config = Self::from_value(raw)?;
        config.source_path = Some(path);
        Ok(config)
    }

    /// Parses a `config.json` document without associating it with a path.
    pub fn from_json_str(contents: &str) -> Result<Self, ConfigError> {
        let raw: Value = serde_json::from_str(contents).map_err(ConfigError::InvalidJson)?;
        Self::from_value(raw)
    }

    /// Builds a typed configuration while retaining `raw` exactly as parsed.
    pub fn from_value(raw: Value) -> Result<Self, ConfigError> {
        let typed: TypedConfig =
            serde_json::from_value(raw.clone()).map_err(ConfigError::InvalidStructure)?;
        Ok(Self {
            server: typed.server,
            audio: typed.audio,
            asr: typed.asr,
            denoise: typed.denoise,
            speaker: typed.speaker,
            prompt_context: typed.prompt_context,
            integrations: typed.integrations,
            storage: typed.storage,
            translation: typed.translation,
            tts: typed.tts,
            ocr: typed.ocr,
            model_manager: typed.model_manager,
            raw,
            source_path: None,
        })
    }

    /// Resolves the common configuration contract for the selected local ASR
    /// and translation providers without knowing their concrete model family.
    /// Provider factories in the backend decide which implementations they
    /// support; this configuration crate only validates shared local-runtime
    /// fields.
    pub fn native_model_route(&self) -> Result<NativeModelRouteConfig, DefaultGgufValidationError> {
        let mut issues = Vec::new();
        let asr = active_native_provider(
            &self.asr.provider,
            &self.asr.providers,
            "asr",
            LocalModelRuntimeConfig {
                context_window_tokens: 2_048,
                max_tokens: 128,
                parallel_slots: 1,
            },
            &mut issues,
        );
        let translation = active_native_provider(
            &self.translation.provider,
            &self.translation.providers,
            "translation",
            LocalModelRuntimeConfig {
                context_window_tokens: 2_048,
                max_tokens: 256,
                parallel_slots: 2,
            },
            &mut issues,
        );
        let llama_server_path = self.model_manager.llama_server_path.trim();

        if issues.is_empty() {
            Ok(NativeModelRouteConfig {
                llama_server_path: PathBuf::from(llama_server_path),
                asr: asr.expect("checked above"),
                translation: translation.expect("checked above"),
            })
        } else {
            Err(DefaultGgufValidationError { issues })
        }
    }

    /// Validates the first native, Python-free GGUF route and returns the
    /// values necessary to launch its two `llama-server` children.
    ///
    /// This validates configuration only; it intentionally does not check
    /// whether the executable, model files, or HTTP endpoints exist. Those
    /// environment checks belong to the process supervisor.
    pub fn default_gguf(&self) -> Result<DefaultGgufConfig, DefaultGgufValidationError> {
        let mut issues = Vec::new();

        if self.asr.provider.trim() != "qwen3-gguf" {
            issues.push(format!(
                "asr.provider must be \"qwen3-gguf\" for the default GGUF route (found {:?})",
                self.asr.provider
            ));
        }
        if self.translation.provider.trim() != "hunyuan" {
            issues.push(format!(
                "translation.provider must be \"hunyuan\" for the default GGUF route (found {:?})",
                self.translation.provider
            ));
        }
        let llama_server_path = required_non_empty(
            &self.model_manager.llama_server_path,
            "model_manager.llama_server_path",
            &mut issues,
        );
        let hunyuan_gguf_repo = required_non_empty(
            &self.model_manager.hunyuan_gguf_repo,
            "model_manager.hunyuan_gguf_repo",
            &mut issues,
        );
        let asr_url = required_provider_url(
            &self.asr.providers,
            "qwen3-gguf",
            "asr.providers.qwen3-gguf.url",
            &mut issues,
        );
        let translation_url = required_provider_url(
            &self.translation.providers,
            "hunyuan",
            "translation.providers.hunyuan.url",
            &mut issues,
        );
        let asr_runtime = provider_runtime_config(
            &self.asr.providers,
            "qwen3-gguf",
            "asr.providers.qwen3-gguf",
            LocalModelRuntimeConfig {
                context_window_tokens: 2_048,
                max_tokens: 128,
                parallel_slots: 1,
            },
            &mut issues,
        );
        let translation_runtime = provider_runtime_config(
            &self.translation.providers,
            "hunyuan",
            "translation.providers.hunyuan",
            LocalModelRuntimeConfig {
                context_window_tokens: 2_048,
                max_tokens: 256,
                parallel_slots: 2,
            },
            &mut issues,
        );

        if issues.is_empty() {
            Ok(DefaultGgufConfig {
                llama_server_path: PathBuf::from(llama_server_path.expect("checked above")),
                hunyuan_gguf_repo: hunyuan_gguf_repo.expect("checked above"),
                asr_url: asr_url.expect("checked above"),
                translation_url: translation_url.expect("checked above"),
                asr_runtime,
                translation_runtime,
            })
        } else {
            Err(DefaultGgufValidationError { issues })
        }
    }

    /// Convenience form of [`Self::default_gguf`] for callers that only need
    /// validation before their own startup logic.
    pub fn validate_default_gguf(&self) -> Result<(), DefaultGgufValidationError> {
        self.default_gguf().map(|_| ())
    }

    /// Returns the ordered native model-asset keys used by the currently
    /// selected provider objects. Plural
    /// `model_assets` takes precedence over the singular compatibility key.
    /// The UI and installer never hard-code a model name or provider pair.
    #[must_use]
    pub fn active_native_model_assets(&self) -> Vec<String> {
        let mut keys = Vec::new();
        for (provider, providers) in [
            (&self.asr.provider, &self.asr.providers),
            (&self.translation.provider, &self.translation.providers),
            (&self.tts.provider, &self.tts.providers),
            #[cfg(not(target_os = "android"))]
            (&self.ocr.provider, &self.ocr.providers),
        ] {
            let Some(model) = providers.get(provider.trim()).and_then(Value::as_object) else {
                continue;
            };
            let configured = model
                .get("model_assets")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::trim)
                        .filter(|key| !key.is_empty())
                        .collect::<Vec<_>>()
                })
                .filter(|values| !values.is_empty())
                .unwrap_or_else(|| {
                    model
                        .get("model_asset")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|key| !key.is_empty())
                        .into_iter()
                        .collect()
                });
            for model_asset in configured {
                if !keys.iter().any(|existing| existing == model_asset) {
                    keys.push(model_asset.to_owned());
                }
            }
        }
        keys
    }
}

fn required_non_empty(value: &str, path: &str, issues: &mut Vec<String>) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        issues.push(format!("{path} must be a non-empty string"));
        None
    } else {
        Some(value.to_owned())
    }
}

fn required_provider_url(
    providers: &ProviderConfigs,
    provider: &str,
    path: &str,
    issues: &mut Vec<String>,
) -> Option<String> {
    let Some(provider_config) = providers.get(provider) else {
        issues.push(format!(
            "{path} is missing because provider {provider:?} is not configured"
        ));
        return None;
    };
    let Some(provider_config) = provider_config.as_object() else {
        issues.push(format!("{path} must be configured inside a JSON object"));
        return None;
    };
    let Some(url) = provider_config.get("url").and_then(Value::as_str) else {
        issues.push(format!("{path} must be a non-empty transport URL"));
        return None;
    };
    let url = url.trim();
    if !(url.starts_with("http://")
        || url.starts_with("https://")
        || url.starts_with("ws://")
        || url.starts_with("wss://"))
    {
        issues.push(format!(
            "{path} must start with http://, https://, ws://, or wss://"
        ));
        return None;
    }
    Some(url.to_owned())
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct TypedConfig {
    #[serde(default)]
    server: ServerConfig,
    #[serde(default)]
    audio: AudioConfig,
    #[serde(default)]
    asr: AsrConfig,
    #[serde(default)]
    denoise: DenoiseConfig,
    #[serde(default)]
    speaker: SpeakerConfig,
    #[serde(default)]
    prompt_context: PromptContextConfig,
    #[serde(default)]
    integrations: IntegrationsConfig,
    #[serde(default)]
    storage: StorageConfig,
    #[serde(default)]
    translation: TranslationConfig,
    #[serde(default)]
    tts: TtsConfig,
    #[serde(default)]
    ocr: OcrConfig,
    #[serde(default)]
    model_manager: ModelManagerConfig,
}

/// HTTP/WebSocket listener settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_server_port")]
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_server_port(),
        }
    }
}

/// Microphone and TTS PCM settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioConfig {
    #[serde(default = "default_pre_buffer_frames")]
    pub pre_buffer_frames: usize,
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,
    #[serde(default = "default_tts_sample_rate")]
    pub tts_sample_rate: u32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            pre_buffer_frames: default_pre_buffer_frames(),
            sample_rate: default_sample_rate(),
            tts_sample_rate: default_tts_sample_rate(),
        }
    }
}

/// ASR selection and untyped provider options.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AsrConfig {
    #[serde(default = "default_asr_provider")]
    pub provider: String,
    #[serde(default)]
    pub providers: ProviderConfigs,
    #[serde(default = "default_vad_threshold")]
    pub vad_threshold: f64,
    /// Ordinary silence required to close a short utterance.
    #[serde(default = "default_vad_silence_ms")]
    pub vad_silence_ms: u32,
    /// Duration after which a shorter micro-pause may close the utterance.
    #[serde(default = "default_vad_adaptive_after_ms")]
    pub vad_adaptive_after_ms: u32,
    /// Micro-pause accepted after `vad_adaptive_after_ms`.
    #[serde(default = "default_vad_adaptive_silence_ms")]
    pub vad_adaptive_silence_ms: u32,
    /// Hard limit for speech with no usable pause.
    #[serde(default = "default_vad_max_utterance_ms")]
    pub vad_max_utterance_ms: u32,
    /// Audio copied across a hard boundary to protect split phonemes.
    #[serde(default = "default_vad_overlap_ms")]
    pub vad_overlap_ms: u32,
}

impl Default for AsrConfig {
    fn default() -> Self {
        Self {
            provider: default_asr_provider(),
            providers: ProviderConfigs::new(),
            vad_threshold: default_vad_threshold(),
            vad_silence_ms: default_vad_silence_ms(),
            vad_adaptive_after_ms: default_vad_adaptive_after_ms(),
            vad_adaptive_silence_ms: default_vad_adaptive_silence_ms(),
            vad_max_utterance_ms: default_vad_max_utterance_ms(),
            vad_overlap_ms: default_vad_overlap_ms(),
        }
    }
}

impl AsrConfig {
    pub fn provider_config(&self, provider: &str) -> Option<&Value> {
        self.providers.get(provider)
    }
}

/// Native GTCRN-Light v3 speech enhancement and background noise suppression settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DenoiseConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_denoise_model_path")]
    pub model_path: PathBuf,
    #[serde(default = "default_denoise_intra_threads")]
    pub intra_threads: usize,
}

impl Default for DenoiseConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            model_path: default_denoise_model_path(),
            intra_threads: default_denoise_intra_threads(),
        }
    }
}

/// Native speaker-embedding and online-clustering settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpeakerConfig {
    /// Speaker recognition is opt-in because the exported ONNX model is a
    /// separately licensed/downloaded artifact rather than part of the repo.
    #[serde(default)]
    pub enabled: bool,
    /// ERes2NetV2 ONNX file exported with 3D-Speaker's official exporter.
    #[serde(default = "default_speaker_model_path")]
    pub model_path: PathBuf,
    /// Cosine threshold above which an embedding joins an existing centroid.
    #[serde(default = "default_speaker_similarity_threshold")]
    pub similarity_threshold: f64,
    /// Lower threshold applied only to the immediately previous speaker.
    #[serde(default = "default_same_speaker_hysteresis")]
    pub same_speaker_hysteresis: f64,
    /// Required cosine advantage before changing a plausible previous speaker.
    #[serde(default = "default_speaker_switch_margin")]
    pub speaker_switch_margin: f64,
    /// Strict upper bound for per-session centroid memory.
    #[serde(default = "default_max_speakers")]
    pub max_speakers: usize,
    /// Very short speech is not reliable enough to create a voiceprint.
    #[serde(default = "default_speaker_min_utterance_ms")]
    pub min_utterance_ms: u32,
    /// ONNX Runtime CPU threads reserved for speaker embedding inference.
    #[serde(default = "default_speaker_intra_threads")]
    pub intra_threads: usize,
}

impl Default for SpeakerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            model_path: default_speaker_model_path(),
            similarity_threshold: default_speaker_similarity_threshold(),
            same_speaker_hysteresis: default_same_speaker_hysteresis(),
            speaker_switch_margin: default_speaker_switch_margin(),
            max_speakers: default_max_speakers(),
            min_utterance_ms: default_speaker_min_utterance_ms(),
            intra_threads: default_speaker_intra_threads(),
        }
    }
}

fn provider_runtime_config(
    providers: &ProviderConfigs,
    provider: &str,
    path: &str,
    defaults: LocalModelRuntimeConfig,
    issues: &mut Vec<String>,
) -> LocalModelRuntimeConfig {
    let object = providers.get(provider).and_then(Value::as_object);
    let mut value = |field: &str, default: u32, minimum: u32, maximum: u32| {
        let Some(raw) = object.and_then(|provider| provider.get(field)) else {
            return default;
        };
        let Some(raw) = raw.as_u64().and_then(|value| u32::try_from(value).ok()) else {
            issues.push(format!("{path}.{field} must be an integer"));
            return default;
        };
        if !(minimum..=maximum).contains(&raw) {
            issues.push(format!(
                "{path}.{field} must be within {minimum}..={maximum}"
            ));
            return default;
        }
        raw
    };
    let runtime = LocalModelRuntimeConfig {
        context_window_tokens: value(
            "context_window_tokens",
            defaults.context_window_tokens,
            256,
            32_768,
        ),
        max_tokens: value("max_tokens", defaults.max_tokens, 16, 4_096),
        parallel_slots: value("parallel_slots", u32::from(defaults.parallel_slots), 1, 16) as u16,
    };
    if runtime.max_tokens.saturating_add(128) > runtime.context_window_tokens {
        issues.push(format!(
            "{path}.context_window_tokens must leave at least 128 input tokens beyond max_tokens"
        ));
    }
    runtime
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageConfig {
    #[serde(default = "default_log_directory")]
    pub log_dir: PathBuf,
    #[serde(default = "default_log_max_bytes")]
    pub log_max_bytes: u64,
    #[serde(default = "default_log_retained_files")]
    pub log_retained_files: usize,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            log_dir: default_log_directory(),
            log_max_bytes: default_log_max_bytes(),
            log_retained_files: default_log_retained_files(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct IntegrationsConfig {
    #[serde(default)]
    pub vrcx: VrcxIntegrationConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VrcxIntegrationConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_vrcx_snapshot_ttl_seconds")]
    pub snapshot_ttl_seconds: u64,
    #[serde(default = "default_vrcx_max_players")]
    pub max_players: usize,
    #[serde(default = "default_vrcx_poll_interval_ms")]
    pub poll_interval_ms: u64,
    #[serde(default)]
    pub database_path: Option<PathBuf>,
}

impl Default for VrcxIntegrationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            snapshot_ttl_seconds: default_vrcx_snapshot_ttl_seconds(),
            max_players: default_vrcx_max_players(),
            poll_interval_ms: default_vrcx_poll_interval_ms(),
            database_path: None,
        }
    }
}

/// Translation selection and untyped provider options.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranslationConfig {
    #[serde(default = "default_translation_provider")]
    pub provider: String,
    #[serde(default)]
    pub providers: ProviderConfigs,
    #[serde(default = "default_source_lang")]
    pub source_lang: String,
    #[serde(default = "default_target_lang")]
    pub target_lang: String,
}

impl Default for TranslationConfig {
    fn default() -> Self {
        Self {
            provider: default_translation_provider(),
            providers: ProviderConfigs::new(),
            source_lang: default_source_lang(),
            target_lang: default_target_lang(),
        }
    }
}

impl TranslationConfig {
    pub fn provider_config(&self, provider: &str) -> Option<&Value> {
        self.providers.get(provider)
    }
}

/// Optional capability selection, disabled until the user chooses a provider.
pub type TtsConfig = OptionalProviderConfig;
pub type OcrConfig = OptionalProviderConfig;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OptionalProviderConfig {
    #[serde(default = "default_optional_provider")]
    pub provider: String,
    #[serde(default)]
    pub providers: ProviderConfigs,
}

impl Default for OptionalProviderConfig {
    fn default() -> Self {
        Self {
            provider: default_optional_provider(),
            providers: ProviderConfigs::new(),
        }
    }
}

impl OptionalProviderConfig {
    pub fn provider_config(&self, provider: &str) -> Option<&Value> {
        self.providers.get(provider)
    }
}

/// Paths and repositories used to manage llama.cpp-backed GGUF models.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelManagerConfig {
    #[serde(default = "default_hunyuan_gguf_repo")]
    pub hunyuan_gguf_repo: String,
    #[serde(default = "default_llama_server_path")]
    pub llama_server_path: String,
    /// Release files used by the desktop client's optional llama.cpp installer.
    /// Keeping the URLs in `config.json` makes a release update a configuration
    /// change instead of a client-code change.
    #[serde(default)]
    pub llama_cpp: LlamaCppRuntimeConfig,
    /// CUDA execution-provider archives for managed in-process ONNX models.
    /// The compact CPU core is reserved for small bundled application models.
    #[serde(default)]
    pub onnxruntime: OnnxRuntimeConfig,
    /// Optional runtime root, resolved relative to `config.json` by the native
    /// backend and desktop client.
    #[serde(default, alias = "runtime_root")]
    pub runtime_directory: Option<PathBuf>,
    /// Optional models root, resolved relative to `config.json` by the native
    /// backend and desktop client.  Keeping this here prevents each frontend
    /// from inventing a different model search path.
    #[serde(default, alias = "models_root", alias = "model_root")]
    pub models_directory: Option<PathBuf>,
    /// Optional package-directory overrides for a versioned native install.
    #[serde(default)]
    pub qwen3_asr_gguf_directory: Option<PathBuf>,
    #[serde(default)]
    pub hunyuan_mt_gguf_directory: Option<PathBuf>,
    /// Optional user preference for local inference GPU device name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_gpu: Option<String>,
    /// Base bundled models (Silero VAD, GTCRN denoiser, 3D-Speaker diarization).
    #[serde(default)]
    pub bundled_models: Vec<BundledModelAsset>,
}

/// One verified base bundled model required by the native backend pipeline.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundledModelAsset {
    pub name: String,
    pub label: String,
    pub relative_path: String,
    pub bytes: u64,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default)]
    pub archive_format: Option<LlamaCppArchiveFormat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_path: Option<String>,
}

/// A fixed llama.cpp release and its downloadable runtime archives.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlamaCppRuntimeConfig {
    /// Human-readable release identifier used in installer diagnostics.
    #[serde(default)]
    pub release: String,
    /// Page shown by the desktop client's manual-install link.
    #[serde(default)]
    pub release_page: String,
    /// Exact archive names and URLs available to the automatic installer.
    #[serde(default)]
    pub downloads: Vec<LlamaCppDownload>,
}

/// A fixed ONNX Runtime release and its downloadable CUDA providers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnnxRuntimeConfig {
    #[serde(default)]
    pub release: String,
    /// ONNX Runtime core and execution-provider archives.
    #[serde(default)]
    pub downloads: Vec<ManagedRuntimeArchive>,
    /// Universal ONNX Runtime CPU core archive downloads.
    #[serde(default)]
    pub cpu_downloads: Vec<ManagedRuntimeArchive>,
    /// CUDA math-library archives not present in llama.cpp's compact runtime
    /// bundle but required to initialize the ONNX CUDA execution provider.
    #[serde(default)]
    pub cuda_dependency_downloads: Vec<ManagedRuntimeArchive>,
    /// cuDNN archives matched by CUDA major version. Keeping this dependency
    /// declarative lets every ONNX provider share the same GPU runtime closure.
    #[serde(default)]
    pub cudnn_downloads: Vec<ManagedRuntimeArchive>,
}

/// One verified native-runtime archive. Only the declared files are retained
/// after extraction, so SDK headers, import libraries and debug artifacts do
/// not enter a managed runtime directory.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedRuntimeArchive {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub archive_format: LlamaCppArchiveFormat,
    #[serde(default)]
    pub bytes: u64,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub cuda_version: String,
    /// Directory inside the archive that contains `required_files`.
    #[serde(default)]
    pub archive_directory: String,
    #[serde(default)]
    pub required_files: Vec<String>,
}

/// Source-compatible name retained for downstream code written against the
/// original ONNX-provider-only archive schema.
pub type OnnxRuntimeDownload = ManagedRuntimeArchive;

/// One llama.cpp archive available from the configured release.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlamaCppDownload {
    pub name: String,
    pub url: String,
    /// Archive encoding used by the release artifact.
    #[serde(default)]
    pub archive_format: LlamaCppArchiveFormat,
    /// Directory inside a runtime archive containing its shared libraries.
    #[serde(default)]
    pub archive_directory: String,
    #[serde(default)]
    pub bytes: u64,
    /// Rust target family this archive can run on, for example
    /// `windows-x86_64` or `linux-x86_64`.
    #[serde(default)]
    pub target: String,
    /// Runtime role. This keeps selection independent from vendor filenames.
    #[serde(default)]
    pub kind: LlamaCppAssetKind,
    /// Executable produced by a server archive, relative to its extracted root.
    /// An empty value is normalized by the runtime installer for legacy config
    /// entries; new entries must declare it explicitly.
    #[serde(default)]
    pub executable: String,
    /// CUDA runtime version for CUDA server/runtime archives.
    #[serde(default)]
    pub cuda_version: Option<String>,
    /// Exact files required after extraction, excluding `executable`.
    #[serde(default)]
    pub required_files: Vec<String>,
    /// Required file-name prefixes, used for versioned shared libraries.
    #[serde(default)]
    pub required_file_prefixes: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LlamaCppArchiveFormat {
    #[default]
    Zip,
    TarGz,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LlamaCppAssetKind {
    #[default]
    ServerCpu,
    ServerCuda,
    ServerVulkan,
    CudaRuntime,
}

impl Default for ModelManagerConfig {
    fn default() -> Self {
        Self {
            hunyuan_gguf_repo: default_hunyuan_gguf_repo(),
            llama_server_path: default_llama_server_path(),
            llama_cpp: LlamaCppRuntimeConfig::default(),
            onnxruntime: OnnxRuntimeConfig::default(),
            runtime_directory: None,
            models_directory: None,
            qwen3_asr_gguf_directory: None,
            hunyuan_mt_gguf_directory: None,
            preferred_gpu: None,
            bundled_models: Vec::new(),
        }
    }
}

impl ModelManagerConfig {
    #[must_use]
    pub fn resolved_bundled_models(&self) -> Vec<BundledModelAsset> {
        if self.bundled_models.is_empty() {
            Self::default_bundled_models()
        } else {
            self.bundled_models.clone()
        }
    }

    #[must_use]
    pub fn default_bundled_models() -> Vec<BundledModelAsset> {
        vec![
            BundledModelAsset {
                name: "silero_vad.onnx".into(),
                label: "Silero VAD".into(),
                relative_path: RuntimeLayout::VAD_MODEL_PATH.into(),
                bytes: RuntimeLayout::VAD_MODEL_BYTES,
                url: "https://raw.githubusercontent.com/snakers4/silero-vad/master/src/silero_vad/data/silero_vad.onnx".into(),
                target: None,
                archive_format: None,
                archive_path: None,
            },
            BundledModelAsset {
                name: "gtcrn_simple.onnx".into(),
                label: "GTCRN Speech Enhancement".into(),
                relative_path: RuntimeLayout::DENOISE_MODEL_PATH.into(),
                bytes: RuntimeLayout::DENOISE_MODEL_BYTES,
                url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/speech-enhancement-models/gtcrn_simple.onnx".into(),
                target: None,
                archive_format: None,
                archive_path: None,
            },
            BundledModelAsset {
                name: "speaker_embedding.onnx".into(),
                label: "3D-Speaker Diarization".into(),
                relative_path: RuntimeLayout::SPEAKER_MODEL_PATH.into(),
                bytes: 129_865_511,
                url: "https://github.com/NowLoadY/XRTranslate/releases/download/v0.2.11/XRTranslate-v0.2.11-linux-x64.zip".into(),
                target: Some("linux-x86_64".into()),
                archive_format: Some(LlamaCppArchiveFormat::Zip),
                archive_path: Some("XRTranslate-v0.2.11-linux-x64/models/3D-Speaker-ERes2NetV2/speaker_embedding.onnx".into()),
            },
            BundledModelAsset {
                name: "speaker_embedding.onnx".into(),
                label: "3D-Speaker Diarization".into(),
                relative_path: RuntimeLayout::SPEAKER_MODEL_PATH.into(),
                bytes: 121_632_051,
                url: "https://github.com/NowLoadY/XRTranslate/releases/download/v0.2.11/XRTranslate-v0.2.11-win-x64.zip".into(),
                target: Some("windows-x86_64".into()),
                archive_format: Some(LlamaCppArchiveFormat::Zip),
                archive_path: Some("XRTranslate-v0.2.11-win-x64/models/3D-Speaker-ERes2NetV2/speaker_embedding.onnx".into()),
            },
        ]
    }
}

impl OnnxRuntimeConfig {
    #[must_use]
    pub fn default_cpu_downloads() -> Vec<ManagedRuntimeArchive> {
        vec![
            ManagedRuntimeArchive {
                name: "onnxruntime-linux-x64-1.28.0.tgz".into(),
                url: "https://github.com/microsoft/onnxruntime/releases/download/v1.28.0/onnxruntime-linux-x64-1.28.0.tgz".into(),
                archive_format: LlamaCppArchiveFormat::TarGz,
                bytes: 9_125_960,
                target: "linux-x86_64".into(),
                cuda_version: String::new(),
                archive_directory: "onnxruntime-linux-x64-1.28.0/lib".into(),
                required_files: vec!["libonnxruntime.so.1.28.0".into()],
            },
            ManagedRuntimeArchive {
                name: "onnxruntime-win-x64-1.28.0.zip".into(),
                url: "https://github.com/microsoft/onnxruntime/releases/download/v1.28.0/onnxruntime-win-x64-1.28.0.zip".into(),
                archive_format: LlamaCppArchiveFormat::Zip,
                bytes: 78_796_801,
                target: "windows-x86_64".into(),
                cuda_version: String::new(),
                archive_directory: "onnxruntime-win-x64-1.28.0/lib".into(),
                required_files: vec!["onnxruntime.dll".into()],
            },
        ]
    }

    #[must_use]
    pub fn resolved_cpu_downloads(&self) -> Vec<ManagedRuntimeArchive> {
        if self.cpu_downloads.is_empty() {
            Self::default_cpu_downloads()
        } else {
            self.cpu_downloads.clone()
        }
    }
}

/// Configuration needed by the default native GGUF supervisor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultGgufConfig {
    pub llama_server_path: PathBuf,
    pub hunyuan_gguf_repo: String,
    pub asr_url: String,
    pub translation_url: String,
    pub asr_runtime: LocalModelRuntimeConfig,
    pub translation_runtime: LocalModelRuntimeConfig,
}

/// Provider-neutral local model route consumed by backend provider factories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeModelRouteConfig {
    pub llama_server_path: PathBuf,
    pub asr: NativeProviderConfig,
    pub translation: NativeProviderConfig,
}

impl NativeModelRouteConfig {
    /// Returns whether at least one selected capability still needs a local
    /// llama.cpp process. Remote API routes can run without the native model
    /// executable or package files.
    #[must_use]
    pub fn uses_local_runtime(&self) -> bool {
        self.asr.uses_local_runtime() || self.translation.uses_local_runtime()
    }
}

/// How an ASR provider interprets text delivered before recognition.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AsrPromptMode {
    #[default]
    None,
    /// A semantic instruction prompt (for example, output and language rules).
    Instruction,
    /// Lexical/context bias text. It must not be treated as an instruction.
    ContextBias,
}

/// Common settings shared by every native model provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeProviderConfig {
    pub provider: String,
    /// Wire protocol: local runtime, OpenAI-compatible HTTP, native DashScope
    /// HTTP, or a provider-native WebSocket.
    pub transport: String,
    pub url: String,
    /// Remote model identifier. Local routes may leave this empty and use the
    /// provider's stable server alias instead.
    pub model: String,
    /// Bearer credential for remote routes. It is intentionally optional in
    /// the typed contract so settings can be edited before a key is entered.
    pub api_key: Option<String>,
    /// Stable local package key. Older configurations may omit this and let
    /// the backend provider profile choose its compatibility default.
    pub model_asset: Option<String>,
    pub runtime: LocalModelRuntimeConfig,
    pub supports_prompt_context: bool,
    pub asr_prompt_mode: AsrPromptMode,
    /// Provider limit for the complete lexical ASR text field. `None` means
    /// that the provider profile declares no character bound.
    pub asr_context_max_chars: Option<usize>,
    pub supports_vocabulary_bias: bool,
    pub vocabulary_weight: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalModelRuntimeConfig {
    pub context_window_tokens: u32,
    pub max_tokens: u32,
    pub parallel_slots: u16,
}

/// JSON parse/read failures for [`AppConfig`].
#[derive(Debug)]
pub enum ConfigError {
    Read { path: PathBuf, source: io::Error },
    Write { path: PathBuf, source: io::Error },
    Migration(String),
    InvalidJson(serde_json::Error),
    InvalidStructure(serde_json::Error),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(
                    formatter,
                    "cannot read configuration {}: {source}",
                    path.display()
                )
            }
            Self::Write { path, source } => {
                write!(
                    formatter,
                    "cannot write configuration {}: {source}",
                    path.display()
                )
            }
            Self::Migration(message) => formatter.write_str(message),
            Self::InvalidJson(source) => {
                write!(formatter, "config.json is not valid JSON: {source}")
            }
            Self::InvalidStructure(source) => {
                write!(
                    formatter,
                    "config.json has an invalid setting type: {source}"
                )
            }
        }
    }
}

impl Error for ConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Read { source, .. } | Self::Write { source, .. } => Some(source),
            Self::Migration(_) => None,
            Self::InvalidJson(source) | Self::InvalidStructure(source) => Some(source),
        }
    }
}

/// A collection of actionable problems in the native default GGUF route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultGgufValidationError {
    issues: Vec<String>,
}

impl DefaultGgufValidationError {
    pub fn issues(&self) -> &[String] {
        &self.issues
    }
}

impl fmt::Display for DefaultGgufValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("default GGUF configuration is not runnable:")?;
        for issue in &self.issues {
            write!(formatter, "\n- {issue}")?;
        }
        Ok(())
    }
}

impl Error for DefaultGgufValidationError {}

fn default_host() -> String {
    "0.0.0.0".into()
}
const fn default_server_port() -> u16 {
    7654
}
const fn default_pre_buffer_frames() -> usize {
    20
}
const fn default_sample_rate() -> u32 {
    16_000
}
const fn default_tts_sample_rate() -> u32 {
    48_000
}
fn default_asr_provider() -> String {
    "qwen3-gguf".into()
}
const fn default_vad_threshold() -> f64 {
    0.6
}
const fn default_vad_silence_ms() -> u32 {
    320
}
const fn default_vad_adaptive_after_ms() -> u32 {
    4_000
}
const fn default_vad_adaptive_silence_ms() -> u32 {
    128
}
const fn default_vad_max_utterance_ms() -> u32 {
    8_000
}
const fn default_vad_overlap_ms() -> u32 {
    256
}
fn default_denoise_model_path() -> PathBuf {
    PathBuf::from("models/gtcrn/gtcrn_simple.onnx")
}
const fn default_denoise_intra_threads() -> usize {
    1
}
fn default_speaker_model_path() -> PathBuf {
    PathBuf::from("models/3D-Speaker-ERes2NetV2/speaker_embedding.onnx")
}
const fn default_true() -> bool {
    true
}
const fn default_vrcx_snapshot_ttl_seconds() -> u64 {
    60
}
const fn default_vrcx_max_players() -> usize {
    80
}
const fn default_vrcx_poll_interval_ms() -> u64 {
    2_000
}
fn default_log_directory() -> PathBuf {
    PathBuf::from("runtime/logs")
}
const fn default_log_max_bytes() -> u64 {
    2 * 1024 * 1024
}
const fn default_log_retained_files() -> usize {
    2
}
const fn default_speaker_similarity_threshold() -> f64 {
    0.56
}
const fn default_same_speaker_hysteresis() -> f64 {
    0.14
}

fn active_native_provider(
    selected_provider: &str,
    providers: &ProviderConfigs,
    section: &str,
    defaults: LocalModelRuntimeConfig,
    issues: &mut Vec<String>,
) -> Option<NativeProviderConfig> {
    let provider = selected_provider.trim();
    if provider.is_empty() {
        issues.push(format!("{section}.provider must be a non-empty string"));
        return None;
    }
    let path = format!("{section}.providers.{provider}");
    let url = required_provider_url(providers, provider, &format!("{path}.url"), issues);
    let object = providers.get(provider).and_then(Value::as_object);
    let transport = object
        .and_then(|provider| provider.get("transport"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("local")
        .to_owned();
    if !matches!(
        transport.as_str(),
        "local" | "onnx-cpu" | "openai" | "dashscope" | "websocket"
    ) {
        issues.push(format!(
            "{path}.transport must be \"local\", \"onnx-cpu\", \"openai\", \"dashscope\", or \"websocket\""
        ));
    }
    if let Some(url) = url.as_deref() {
        let uses_websocket_url = url.starts_with("ws://") || url.starts_with("wss://");
        if transport == "websocket" && !uses_websocket_url {
            issues.push(format!(
                "{path}.url must use ws:// or wss:// for websocket transport"
            ));
        } else if transport != "websocket" && uses_websocket_url {
            issues.push(format!(
                "{path}.url must use http:// or https:// for {transport} transport"
            ));
        }
        if provider == "qwen-audio-streaming" && !url.starts_with("wss://") {
            issues.push(format!(
                "{path}.url must use wss:// for the Qwen Audio streaming service"
            ));
        }
    }
    let model = object
        .and_then(|provider| provider.get("model"))
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_owned();
    if !matches!(transport.as_str(), "local" | "onnx-cpu") && model.is_empty() {
        issues.push(format!(
            "{path}.model must be a non-empty string for remote providers"
        ));
    }
    let api_key = object
        .and_then(|provider| provider.get("api_key"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    if !matches!(transport.as_str(), "local" | "onnx-cpu") && api_key.is_none() {
        issues.push(format!(
            "{path}.api_key is required for remote API providers"
        ));
    }
    if transport == "openai" && provider == "openai" {
        let official_endpoint = if section == "asr" {
            matches!(
                url.as_deref(),
                Some(
                    "https://api.openai.com/v1/audio/transcriptions"
                        | "https://api.openai.com/v1/chat/completions"
                )
            )
        } else {
            url.as_deref() == Some("https://api.openai.com/v1/chat/completions")
        };
        if !official_endpoint {
            issues.push(format!(
                "{path}.url must use an official OpenAI endpoint for {section}"
            ));
        }
        if section == "asr"
            && (model.contains("transcribe") || model == "whisper-1")
            && url.as_deref() == Some("https://api.openai.com/v1/chat/completions")
        {
            issues.push(format!(
                "{path}.url must use /v1/audio/transcriptions for {model}"
            ));
        }
    }
    let model_asset = object
        .and_then(|provider| provider.get("model_asset"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let runtime = provider_runtime_config(providers, provider, &path, defaults, issues);
    let remote_asr = (section == "asr")
        .then(|| xrtranslate_assets::remote::remote_asr_model(provider, &transport, &model))
        .flatten();
    if section == "asr" && matches!(provider, "qwen" | "qwen-intl") {
        if transport != "dashscope" {
            issues.push(format!("{path}.transport must use dashscope for Qwen Audio ASR; reselect the online preset or configure a native audio endpoint"));
        }
        if model.starts_with("qwen3-asr-") {
            issues.push(format!("{path}.model uses the retired Qwen3 cloud contract; select qwen-audio-3.0-asr-flash with its native DashScope endpoint"));
        }
        if url
            .as_deref()
            .is_some_and(|url| url.trim_end_matches('/').ends_with("/chat/completions"))
        {
            issues.push(format!(
                "{path}.url must use the native multimodal-generation endpoint for Qwen Audio ASR"
            ));
        }
    }
    let supports_prompt_context = object
        .and_then(|provider| provider.get("supports_prompt_context"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let legacy_supports_prompt = object
        .and_then(|provider| provider.get("supports_prompt"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let asr_prompt_mode = match object
        .and_then(|provider| provider.get("asr_prompt_mode"))
        .and_then(Value::as_str)
        .map(str::trim)
    {
        Some("instruction") => AsrPromptMode::Instruction,
        Some("context_bias") => AsrPromptMode::ContextBias,
        Some("none") => AsrPromptMode::None,
        None if !(supports_prompt_context || legacy_supports_prompt) => AsrPromptMode::None,
        None => AsrPromptMode::Instruction,
        Some(value) => {
            issues.push(format!(
                "{path}.asr_prompt_mode must be \"none\", \"instruction\", or \"context_bias\" (found {value:?})"
            ));
            AsrPromptMode::None
        }
    };
    // Known model contracts override stale capability flags in user settings.
    let supports_prompt_context = remote_asr.map_or(supports_prompt_context, |card| {
        card.context_bias || card.instruction
    });
    let asr_prompt_mode = remote_asr.map_or(asr_prompt_mode, |card| {
        if card.context_bias {
            AsrPromptMode::ContextBias
        } else if card.instruction {
            AsrPromptMode::Instruction
        } else {
            AsrPromptMode::None
        }
    });
    let supports_vocabulary_bias = remote_asr.map_or_else(
        || {
            object
                .and_then(|provider| provider.get("supports_vocabulary_bias"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
        },
        |card| card.vocabulary_bias,
    );
    let asr_context_max_chars = remote_asr.map_or_else(
        || {
            object
                .and_then(|provider| provider.get("asr_context_max_chars"))
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
        },
        |card| card.context_max_chars,
    );
    if object
        .and_then(|provider| provider.get("asr_context_max_chars"))
        .is_some_and(|value| {
            value
                .as_u64()
                .and_then(|limit| usize::try_from(limit).ok())
                .is_none_or(|limit| limit == 0)
        })
    {
        issues.push(format!(
            "{path}.asr_context_max_chars must be a positive integer"
        ));
    }
    let vocabulary_weight = object
        .and_then(|provider| provider.get("vocabulary_weight"))
        .and_then(Value::as_u64)
        .and_then(|value| u8::try_from(value).ok())
        .unwrap_or(4);
    if supports_vocabulary_bias && !matches!(vocabulary_weight, 1..=5 | 50) {
        issues.push(format!(
            "{path}.vocabulary_weight must be in 1..=5 or equal to 50"
        ));
    }
    if !issues
        .iter()
        .any(|issue| issue.starts_with(&format!("{path}.")))
    {
        match url {
            Some(url) => Some(NativeProviderConfig {
                provider: provider.to_owned(),
                transport,
                url,
                model,
                api_key,
                model_asset,
                runtime,
                supports_prompt_context,
                asr_prompt_mode,
                asr_context_max_chars,
                supports_vocabulary_bias,
                vocabulary_weight,
            }),
            None => None,
        }
    } else {
        None
    }
}

impl NativeProviderConfig {
    #[must_use]
    pub fn uses_local_runtime(&self) -> bool {
        matches!(self.transport.as_str(), "local" | "onnx-cpu")
    }
}
const fn default_speaker_switch_margin() -> f64 {
    0.04
}
const fn default_max_speakers() -> usize {
    8
}
const fn default_speaker_min_utterance_ms() -> u32 {
    750
}
const fn default_speaker_intra_threads() -> usize {
    2
}
fn default_translation_provider() -> String {
    "hunyuan".into()
}
fn default_source_lang() -> String {
    "auto".into()
}
fn default_target_lang() -> String {
    "zh,en".into()
}
fn default_optional_provider() -> String {
    "none".into()
}
fn default_hunyuan_gguf_repo() -> String {
    "tencent/Hy-MT2-1.8B-GGUF".into()
}
fn default_llama_server_path() -> String {
    "runtime/llama.cpp/llama-server".into()
}
