//! Native configuration reader for the project-root `config.json`.
//!
//! The typed fields cover the settings needed by the native backend. The full
//! parsed document remains available through [`AppConfig::raw`], so optional
//! provider settings can evolve without forcing this crate to model each one.

#![forbid(unsafe_code)]

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
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedNativeRuntimeSelection {
    pub backend: NativeRuntimeBackend,
    pub llama_cpp_backend: Option<NativeRuntimeBackend>,
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
    pub const ONNX_CORE_LIBRARY: &'static str = "onnxruntime.dll";
    pub const NATIVE_RUNTIME_SELECTION_FILE: &'static str = "runtime/native-runtime.json";

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
    pub fn native_runtime_selection_file(&self) -> PathBuf {
        self.runtime_root.join("native-runtime.json")
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
    /// settings. Debug builds keep it inside the ignored project runtime;
    /// packaged builds use the platform user configuration directory so an
    /// application update never replaces personal settings.
    #[must_use]
    pub fn user_config_path(project_root: impl AsRef<Path>) -> PathBuf {
        if cfg!(debug_assertions) {
            return project_root
                .as_ref()
                .join("runtime")
                .join("user-config.json");
        }

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
    let base_path = base_path.as_ref();
    let contents = fs::read_to_string(base_path).map_err(|source| ConfigError::Read {
        path: base_path.to_path_buf(),
        source,
    })?;
    let mut document: Value = serde_json::from_str(&contents).map_err(ConfigError::InvalidJson)?;
    let override_path = RuntimeLayout::user_config_path(project_root);
    if override_path.is_file() {
        let contents = fs::read_to_string(&override_path).map_err(|source| ConfigError::Read {
            path: override_path.clone(),
            source,
        })?;
        let mut overlay: Value =
            serde_json::from_str(&contents).map_err(ConfigError::InvalidJson)?;
        migrate_legacy_openai_asr_model(&mut overlay);
        merge_config_values(&mut document, overlay);
    }
    Ok(document)
}

fn migrate_legacy_openai_asr_model(document: &mut Value) -> bool {
    let path = "/asr/providers/openai/model";
    if document.pointer(path).and_then(Value::as_str) != Some("gpt-4o-audio-preview") {
        return false;
    }
    *document
        .pointer_mut(path)
        .expect("model path was resolved above") = Value::from("gpt-4o-transcribe");
    true
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
    /// The selected TTS provider uses the in-process ONNX runtime. CUDA is an
    /// optional host acceleration for this requirement; CPU remains valid.
    pub onnx_tts: bool,
    /// The selected ONNX TTS device allows CUDA. `device = "cpu"` keeps this
    /// false so the host does not probe or download NVIDIA runtime assets.
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
        for section in root.values().filter_map(Value::as_object) {
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
                Some("onnx") => {
                    requirements.onnx_tts = true;
                    requirements.onnx_cuda |= provider
                        .get("device")
                        .and_then(Value::as_str)
                        .is_none_or(|device| device != "cpu");
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
    /// selected ASR and translation provider objects.  The UI and installer
    /// use these keys to build their model catalogue; they never hard-code a
    /// model name or provider pair.
    #[must_use]
    pub fn active_native_model_assets(&self) -> Vec<String> {
        let mut keys = Vec::new();
        for (provider, providers) in [
            (&self.asr.provider, &self.asr.providers),
            (&self.translation.provider, &self.translation.providers),
            (&self.tts.provider, &self.tts.providers),
        ] {
            let Some(model_asset) = providers
                .get(provider.trim())
                .and_then(Value::as_object)
                .and_then(|model| model.get("model_asset"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|key| !key.is_empty())
            else {
                continue;
            };
            if !keys.iter().any(|existing| existing == model_asset) {
                keys.push(model_asset.to_owned());
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

/// TTS selection and optional future-provider options.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TtsConfig {
    #[serde(default = "default_tts_provider")]
    pub provider: String,
    #[serde(default)]
    pub providers: ProviderConfigs,
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            provider: default_tts_provider(),
            providers: ProviderConfigs::new(),
        }
    }
}

impl TtsConfig {
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
    /// Optional CUDA execution-provider archives for in-process ONNX models.
    /// CPU inference uses the compact core supplied by the native release and
    /// requires no end-user download.
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
    pub sha256: String,
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
    #[serde(default)]
    pub bytes: u64,
    #[serde(default)]
    pub sha256: String,
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
    /// `local` selects a managed llama.cpp route; `openai` selects a remote
    /// OpenAI Chat Completions-compatible endpoint.
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
            Self::Read { source, .. } => Some(source),
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
    if !matches!(transport.as_str(), "local" | "openai" | "websocket") {
        issues.push(format!(
            "{path}.transport must be \"local\", \"openai\", or \"websocket\""
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
    if transport != "local" && model.is_empty() {
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
    if transport != "local" && api_key.is_none() {
        issues.push(format!(
            "{path}.api_key is required for remote API providers"
        ));
    }
    if transport == "openai" && provider == "openai" {
        if url.as_deref() != Some("https://api.openai.com/v1/chat/completions") {
            issues.push(format!(
                "{path}.url must use the official OpenAI Chat Completions endpoint"
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
    let supports_vocabulary_bias = object
        .and_then(|provider| provider.get("supports_vocabulary_bias"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let asr_context_max_chars = object
        .and_then(|provider| provider.get("asr_context_max_chars"))
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok());
    if object
        .and_then(|provider| provider.get("asr_context_max_chars"))
        .is_some()
        && asr_context_max_chars.is_none_or(|limit| limit == 0)
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
        self.transport == "local"
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
fn default_tts_provider() -> String {
    "none".into()
}
fn default_hunyuan_gguf_repo() -> String {
    "tencent/Hy-MT2-1.8B-GGUF".into()
}
fn default_llama_server_path() -> String {
    "runtime/llama.cpp/llama-server".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_layout_resolves_and_persists_managed_paths_relative_to_root() {
        let root = PathBuf::from("/tmp/xrtranslate-release");
        let layout = RuntimeLayout::for_project_root(&root);
        let configured = layout.resolve_configured_path("runtime/llama.cpp/llama-server");
        assert_eq!(configured, layout.managed_llama_server("llama-server"));
        assert_eq!(
            layout.config_path_for(&configured),
            PathBuf::from("runtime/llama.cpp/llama-server")
        );
    }

    #[test]
    fn runtime_layout_recovers_executable_persisted_as_runtime_directory() {
        let root = PathBuf::from("/tmp/xrtranslate-release");
        let relative = RuntimeLayout::new(&root, Some("runtime/llama.cpp/llama-server.exe"));
        assert_eq!(relative.runtime_root(), root.join("runtime"));
        assert_eq!(
            relative.onnx_cpu_core_library(),
            root.join("runtime/onnxruntime/cpu/onnxruntime.dll")
        );

        let custom_root = std::env::temp_dir().join("xrtranslate-custom-runtime");
        let absolute =
            RuntimeLayout::new(&root, Some(custom_root.join("llama.cpp/llama-server.exe")));
        assert_eq!(absolute.runtime_root(), custom_root);

        let unrelated = RuntimeLayout::new(&root, Some("custom/llama-server.exe"));
        assert_eq!(
            unrelated.runtime_root(),
            root.join("custom/llama-server.exe")
        );
    }

    #[test]
    fn runtime_layout_keeps_external_manual_paths_absolute() {
        let layout = RuntimeLayout::for_project_root("/tmp/xrtranslate-release");
        let external = PathBuf::from("/opt/llama.cpp/llama-server");
        assert_eq!(layout.config_path_for(&external), external);
    }

    #[test]
    fn runtime_layout_with_custom_directory_resolves_all_subdirs() {
        let root = PathBuf::from("/tmp/xrtranslate-release");
        let layout = RuntimeLayout::new(&root, Some("custom_runtime"));
        assert_eq!(
            layout.runtime_root(),
            Path::new("/tmp/xrtranslate-release/custom_runtime")
        );
        assert_eq!(
            layout.llama_cpp_directory(),
            PathBuf::from("/tmp/xrtranslate-release/custom_runtime/llama.cpp")
        );
        assert_eq!(
            layout.cuda_runtime_directory("13.3"),
            PathBuf::from("/tmp/xrtranslate-release/custom_runtime/cuda/13.3")
        );
        assert_eq!(
            layout.cudnn_runtime_directory("13"),
            PathBuf::from("/tmp/xrtranslate-release/custom_runtime/cudnn/13")
        );
        assert_eq!(
            layout.onnx_runtime_directory("13"),
            PathBuf::from("/tmp/xrtranslate-release/custom_runtime/onnxruntime/cuda-13")
        );
        assert_eq!(
            layout.native_runtime_selection_file(),
            PathBuf::from("/tmp/xrtranslate-release/custom_runtime/native-runtime.json")
        );

        let external_layout = RuntimeLayout::new(&root, Some("/mnt/ai/runtime"));
        assert_eq!(external_layout.runtime_root(), Path::new("/mnt/ai/runtime"));
        assert_eq!(
            external_layout.llama_cpp_directory(),
            PathBuf::from("/mnt/ai/runtime/llama.cpp")
        );
    }

    #[test]
    fn root_config_is_read_with_optional_sections_preserved() {
        let config = AppConfig::from_json_str(include_str!("../../../config.json")).unwrap();

        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 7654);
        assert_eq!(config.audio.sample_rate, 16_000);
        assert_eq!(config.audio.tts_sample_rate, 48_000);
        assert_eq!(config.asr.provider, "qwen3-gguf");
        assert_eq!(config.asr.vad_silence_ms, 320);
        assert_eq!(config.asr.vad_adaptive_after_ms, 4_000);
        assert_eq!(config.asr.vad_adaptive_silence_ms, 128);
        assert_eq!(config.asr.vad_max_utterance_ms, 8_000);
        assert_eq!(config.asr.vad_overlap_ms, 256);
        assert!(config.speaker.enabled);
        assert_eq!(config.speaker.max_speakers, 8);
        assert_eq!(config.speaker.min_utterance_ms, 750);
        assert_eq!(config.speaker.same_speaker_hysteresis, 0.12);
        assert_eq!(config.speaker.speaker_switch_margin, 0.04);
        assert!(config.prompt_context.enabled);
        assert_eq!(config.prompt_context.max_entries, 6);
        assert_eq!(config.prompt_context.asr_max_chars, 800);
        assert_eq!(config.prompt_context.asr_history_entries, 1);
        assert_eq!(config.prompt_context.translation_history_entries, 6);
        assert_eq!(config.prompt_context.translation_max_chars, 1200);
        assert_eq!(
            config.prompt_context.corpora_directory,
            PathBuf::from("XR-Corpus/corpora/v1")
        );
        assert_eq!(config.storage.log_dir, PathBuf::from("runtime/logs"));
        assert_eq!(config.storage.log_max_bytes, 2 * 1024 * 1024);
        assert_eq!(config.storage.log_retained_files, 2);
        assert_eq!(config.translation.provider, "hunyuan");
        assert_eq!(config.tts.provider, "none");
        assert_eq!(config.model_manager.llama_cpp.release, "b10333");
        assert_eq!(config.model_manager.llama_cpp.downloads.len(), 8);
        assert_eq!(config.model_manager.onnxruntime.release, "1.28.0");
        assert_eq!(config.model_manager.onnxruntime.downloads.len(), 2);
        assert_eq!(config.model_manager.onnxruntime.cudnn_downloads.len(), 2);
        assert_eq!(
            config.model_manager.llama_cpp.downloads[0].name,
            "llama-b10333-bin-win-cpu-x64.zip"
        );
        assert_eq!(
            config
                .raw
                .pointer("/osc/listen_port")
                .and_then(Value::as_u64),
            Some(9001)
        );
    }

    #[test]
    fn legacy_speaker_config_gets_the_safe_switch_margin() {
        let speaker: SpeakerConfig = serde_json::from_str(
            r#"{"enabled":true,"similarity_threshold":0.56,"same_speaker_hysteresis":0.16}"#,
        )
        .unwrap();

        assert_eq!(speaker.speaker_switch_margin, 0.04);
    }

    #[test]
    fn root_config_passes_default_gguf_validation() {
        let config = AppConfig::from_json_str(include_str!("../../../config.json")).unwrap();
        let gguf = config.default_gguf().unwrap();

        assert!(
            gguf.llama_server_path == PathBuf::from("runtime/llama.cpp/llama-server")
                || gguf.llama_server_path == PathBuf::from("runtime/llama.cpp/llama-server.exe")
        );
        assert_eq!(gguf.hunyuan_gguf_repo, "tencent/Hy-MT2-1.8B-GGUF");
        assert_eq!(gguf.asr_url, "http://127.0.0.1:8001/v1/chat/completions");
        assert_eq!(
            gguf.translation_url,
            "http://127.0.0.1:8002/v1/chat/completions"
        );
        assert_eq!(gguf.asr_runtime.context_window_tokens, 4_800);
        assert_eq!(gguf.asr_runtime.max_tokens, 128);
        assert_eq!(gguf.asr_runtime.parallel_slots, 1);
        assert_eq!(gguf.translation_runtime.context_window_tokens, 2_048);
        assert_eq!(gguf.translation_runtime.max_tokens, 256);
        assert_eq!(gguf.translation_runtime.parallel_slots, 2);
    }

    #[test]
    fn native_model_route_uses_the_selected_provider_contract() {
        let config = AppConfig::from_json_str(include_str!("../../../config.json")).unwrap();
        let route = config.native_model_route().unwrap();

        assert_eq!(route.asr.provider, "qwen3-gguf");
        assert_eq!(route.asr.model_asset.as_deref(), Some("qwen3-asr-gguf"));
        assert_eq!(route.asr.runtime.context_window_tokens, 4_800);
        assert_eq!(route.translation.provider, "hunyuan");
        assert_eq!(route.translation.model_asset.as_deref(), Some("hy-mt2"));
        assert_eq!(route.translation.runtime.parallel_slots, 2);
    }

    #[test]
    fn native_model_route_does_not_assume_a_provider_family() {
        let mut document: Value =
            serde_json::from_str(include_str!("../../../config.json")).unwrap();
        document["translation"]["provider"] = Value::from("future-local-provider");
        document["translation"]["providers"]["future-local-provider"] = serde_json::json!({
            "url": "http://127.0.0.1:8010/v1/chat/completions",
            "model_asset": "future-translation-model",
            "context_window_tokens": 4096,
            "max_tokens": 512,
            "parallel_slots": 3
        });

        let route = AppConfig::from_value(document)
            .unwrap()
            .native_model_route()
            .unwrap();

        assert_eq!(route.translation.provider, "future-local-provider");
        assert_eq!(
            route.translation.model_asset.as_deref(),
            Some("future-translation-model")
        );
        assert_eq!(route.translation.runtime.parallel_slots, 3);
    }

    #[test]
    fn native_model_route_accepts_legacy_providers_without_an_asset_key() {
        let mut document: Value =
            serde_json::from_str(include_str!("../../../config.json")).unwrap();
        document["asr"]["providers"]["qwen3-gguf"]
            .as_object_mut()
            .unwrap()
            .remove("model_asset");
        document["translation"]["providers"]["hunyuan"]
            .as_object_mut()
            .unwrap()
            .remove("model_asset");

        let route = AppConfig::from_value(document)
            .unwrap()
            .native_model_route()
            .unwrap();

        assert_eq!(route.asr.model_asset, None);
        assert_eq!(route.translation.model_asset, None);
    }

    #[test]
    fn native_model_route_accepts_remote_models_without_local_assets() {
        let mut document: Value =
            serde_json::from_str(include_str!("../../../config.json")).unwrap();
        document["asr"]["provider"] = Value::from("openai");
        document["translation"]["provider"] = Value::from("openai");
        document["asr"]["providers"]["openai"]["api_key"] = Value::from("test-key");
        document["translation"]["providers"]["openai"]["api_key"] = Value::from("test-key");
        document["model_manager"]["llama_server_path"] = Value::from("");
        let config = AppConfig::from_value(document).unwrap();
        let route = config.native_model_route().unwrap();

        assert!(!route.uses_local_runtime());
        assert_eq!(route.asr.model, "gpt-4o-transcribe");
        assert_eq!(route.translation.model, "gpt-4o-mini");
        assert_eq!(route.asr.api_key.as_deref(), Some("test-key"));
    }

    #[test]
    fn qwen_audio_streaming_keeps_text_and_weighted_bias_capabilities_distinct() {
        let mut document: Value =
            serde_json::from_str(include_str!("../../../config.json")).unwrap();
        document["asr"]["provider"] = Value::from("qwen-audio-streaming");
        document["asr"]["providers"]["qwen-audio-streaming"]["api_key"] =
            Value::from("dashscope-key");

        let config = AppConfig::from_value(document).unwrap();
        let route = config.native_model_route().unwrap();

        assert!(!route.asr.uses_local_runtime());
        assert_eq!(route.asr.transport, "websocket");
        assert_eq!(route.asr.asr_prompt_mode, AsrPromptMode::ContextBias);
        assert_eq!(route.asr.asr_context_max_chars, Some(400));
        assert!(route.asr.supports_vocabulary_bias);
        assert_eq!(route.asr.vocabulary_weight, 4);
        assert_eq!(route.asr.model, "qwen-audio-3.0-asr-flash-streaming");
    }

    #[test]
    fn weighted_vocabulary_rejects_unsupported_weights() {
        let mut document: Value =
            serde_json::from_str(include_str!("../../../config.json")).unwrap();
        document["asr"]["provider"] = Value::from("qwen-audio-streaming");
        document["asr"]["providers"]["qwen-audio-streaming"]["api_key"] =
            Value::from("dashscope-key");
        document["asr"]["providers"]["qwen-audio-streaming"]["vocabulary_weight"] = Value::from(6);

        let error = AppConfig::from_value(document)
            .unwrap()
            .native_model_route()
            .unwrap_err();

        assert!(error.to_string().contains("vocabulary_weight"));
    }

    #[test]
    fn qwen_audio_streaming_rejects_insecure_remote_endpoint() {
        let mut document: Value =
            serde_json::from_str(include_str!("../../../config.json")).unwrap();
        document["asr"]["provider"] = Value::from("qwen-audio-streaming");
        document["asr"]["providers"]["qwen-audio-streaming"]["api_key"] =
            Value::from("dashscope-key");
        document["asr"]["providers"]["qwen-audio-streaming"]["url"] =
            Value::from("ws://example.com/api-ws/v1/inference");

        let error = AppConfig::from_value(document)
            .unwrap()
            .native_model_route()
            .unwrap_err();

        assert!(error.to_string().contains("must use wss://"));
    }

    #[test]
    fn qwen_audio_streaming_rejects_an_empty_context_character_budget() {
        let mut document: Value =
            serde_json::from_str(include_str!("../../../config.json")).unwrap();
        document["asr"]["provider"] = Value::from("qwen-audio-streaming");
        document["asr"]["providers"]["qwen-audio-streaming"]["api_key"] =
            Value::from("dashscope-key");
        document["asr"]["providers"]["qwen-audio-streaming"]["asr_context_max_chars"] =
            Value::from(0);

        let error = AppConfig::from_value(document)
            .unwrap()
            .native_model_route()
            .unwrap_err();

        assert!(error.to_string().contains("asr_context_max_chars"));
    }

    #[test]
    fn runtime_requirements_cover_future_provider_sections() {
        let mut document: Value =
            serde_json::from_str(include_str!("../../../config.json")).unwrap();
        document["asr"]["provider"] = Value::from("openai");
        document["translation"]["provider"] = Value::from("openai");
        document["asr"]["providers"]["openai"]["api_key"] = Value::from("asr-key");
        document["translation"]["providers"]["openai"]["api_key"] = Value::from("translation-key");

        let remote = AppConfig::from_value(document.clone())
            .unwrap()
            .runtime_requirements();
        assert_eq!(remote, RuntimeRequirements::default());

        document["future_model"] = serde_json::json!({
            "provider": "future-local",
            "providers": {
                "future-local": {"transport": "local"}
            }
        });
        let local = AppConfig::from_value(document.clone())
            .unwrap()
            .runtime_requirements();
        assert!(local.llama_cpp);
        assert!(!local.missing_api_key);

        document["future_model"]["provider"] = Value::from("future-api");
        document["future_model"]["providers"]["future-api"] =
            serde_json::json!({"transport": "openai", "api_key": ""});
        let missing_key = AppConfig::from_value(document)
            .unwrap()
            .runtime_requirements();
        assert!(!missing_key.llama_cpp);
        assert!(missing_key.missing_api_key);
    }

    #[test]
    fn onnx_tts_does_not_imply_llama_cpp() {
        let mut document: Value =
            serde_json::from_str(include_str!("../../../config.json")).unwrap();
        document["asr"]["provider"] = Value::from("openai");
        document["asr"]["providers"]["openai"]["api_key"] = Value::from("asr-key");
        document["translation"]["provider"] = Value::from("openai");
        document["translation"]["providers"]["openai"]["api_key"] = Value::from("translation-key");
        document["tts"]["provider"] = Value::from("audio8");

        let requirements = AppConfig::from_value(document)
            .unwrap()
            .runtime_requirements();

        assert!(!requirements.llama_cpp);
        assert!(requirements.onnx_tts);
        assert!(requirements.onnx_cuda);
        assert!(!requirements.missing_api_key);
    }

    #[test]
    fn explicit_cpu_tts_does_not_request_cuda_assets() {
        let mut document: Value =
            serde_json::from_str(include_str!("../../../config.json")).unwrap();
        document["tts"]["provider"] = Value::from("audio8");
        document["tts"]["providers"]["audio8"]["device"] = Value::from("cpu");
        let requirements = AppConfig::from_value(document)
            .unwrap()
            .runtime_requirements();
        assert!(requirements.onnx_tts);
        assert!(!requirements.onnx_cuda);
    }

    #[test]
    fn native_runtime_paths_resolve_from_a_movable_marker() {
        let layout = RuntimeLayout::for_project_root("release-root");
        let marker = NativeRuntimeSelection {
            schema_version: 1,
            backend: NativeRuntimeBackend::Cuda,
            llama_cpp_backend: Some(NativeRuntimeBackend::Cuda),
            onnx_backend: Some(NativeRuntimeBackend::Cuda),
            cuda_version: Some("13.3".into()),
            provider_dir: Some(PathBuf::from("runtime/onnxruntime/cuda-13")),
            onnx_core_library: Some(PathBuf::from("runtime/onnxruntime/cuda-13/onnxruntime.dll")),
            cuda_bin_dir: Some(PathBuf::from("runtime/cuda/13.3")),
            cudnn_bin_dir: None,
            preload_libraries: vec![PathBuf::from("runtime/cuda/13.3/cudart64_13.dll")],
            fallback_reason: None,
        };

        let resolved = layout.resolve_native_runtime_selection(&marker);
        assert_eq!(
            resolved.provider_dir.as_deref(),
            Some(Path::new("release-root/runtime/onnxruntime/cuda-13"))
        );
        assert_eq!(
            resolved.onnx_core_library.as_deref(),
            Some(Path::new(
                "release-root/runtime/onnxruntime/cuda-13/onnxruntime.dll"
            ))
        );
        assert_eq!(
            resolved.preload_libraries[0],
            PathBuf::from("release-root/runtime/cuda/13.3/cudart64_13.dll")
        );
    }

    #[test]
    fn local_provider_selection_can_be_saved_before_llama_cpp_is_installed() {
        let mut document: Value =
            serde_json::from_str(include_str!("../../../config.json")).unwrap();
        document["model_manager"]["llama_server_path"] = Value::from("");

        let route = AppConfig::from_value(document)
            .unwrap()
            .native_model_route()
            .unwrap();
        assert!(route.uses_local_runtime());
        assert!(route.llama_server_path.as_os_str().is_empty());
    }

    #[test]
    fn user_config_overlay_preserves_defaults_and_round_trips_changes() {
        let root =
            std::env::temp_dir().join(format!("xrtranslate-config-overlay-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let base_path = root.join("config.json");
        let base = serde_json::json!({
            "translation": {"provider": "hunyuan", "providers": {"hunyuan": {"max_tokens": 256}}},
            "model_manager": {"llama_server_path": "runtime/llama.cpp/llama-server"}
        });
        fs::write(&base_path, serde_json::to_vec_pretty(&base).unwrap()).unwrap();

        let mut effective = load_user_config_document(&base_path, &root).unwrap();
        effective["translation"]["providers"]["hunyuan"]["max_tokens"] = Value::from(512);
        save_user_config_document(&base_path, &root, &effective).unwrap();

        let persisted = load_user_config_document(&base_path, &root).unwrap();
        assert_eq!(
            persisted["translation"]["providers"]["hunyuan"]["max_tokens"],
            512
        );
        assert_eq!(
            persisted["model_manager"]["llama_server_path"],
            "runtime/llama.cpp/llama-server"
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(&base_path).unwrap()).unwrap(),
            base
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn user_config_migrates_only_the_deprecated_openai_asr_default() {
        let root = std::env::temp_dir().join(format!(
            "xrtranslate-config-openai-migration-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("runtime")).unwrap();
        let base_path = root.join("config.json");
        fs::write(
            &base_path,
            include_bytes!("../../../config.json").as_slice(),
        )
        .unwrap();
        let override_path = RuntimeLayout::user_config_path(&root);
        fs::write(
            &override_path,
            r#"{"asr":{"providers":{"openai":{"model":"gpt-4o-audio-preview"}}}}"#,
        )
        .unwrap();

        let migrated = load_user_config_document(&base_path, &root).unwrap();
        assert_eq!(
            migrated.pointer("/asr/providers/openai/model"),
            Some(&Value::from("gpt-4o-transcribe"))
        );
        saved_custom_model_is_preserved(&base_path, &root, &override_path);
        fs::remove_dir_all(root).unwrap();
    }

    fn saved_custom_model_is_preserved(base_path: &Path, root: &Path, override_path: &Path) {
        fs::write(
            override_path,
            r#"{"asr":{"providers":{"openai":{"model":"custom-transcribe-model"}}}}"#,
        )
        .unwrap();
        let loaded = load_user_config_document(base_path, root).unwrap();
        assert_eq!(
            loaded.pointer("/asr/providers/openai/model"),
            Some(&Value::from("custom-transcribe-model"))
        );
    }

    #[test]
    fn model_runtime_rejects_output_that_leaves_no_input_budget() {
        let mut document: Value =
            serde_json::from_str(include_str!("../../../config.json")).unwrap();
        document["translation"]["providers"]["hunyuan"]["context_window_tokens"] = Value::from(256);
        document["translation"]["providers"]["hunyuan"]["max_tokens"] = Value::from(256);
        let config = AppConfig::from_value(document).unwrap();
        assert!(
            config
                .default_gguf()
                .unwrap_err()
                .to_string()
                .contains("must leave at least 128 input tokens")
        );
    }

    #[test]
    fn gguf_validation_reports_all_actionable_fields() {
        let config = AppConfig::from_json_str(
            r#"{
                "asr": {"provider": "sensevoice"},
                "translation": {"provider": "groq"},
                "tts": {"provider": "index"},
                "model_manager": {"llama_server_path": "", "hunyuan_gguf_repo": ""}
            }"#,
        )
        .unwrap();

        let error = config.default_gguf().unwrap_err();
        let message = error.to_string();
        assert!(message.contains("asr.provider must be \"qwen3-gguf\""));
        assert!(message.contains("translation.provider must be \"hunyuan\""));
        assert!(message.contains("model_manager.llama_server_path must be a non-empty string"));
        assert!(message.contains("asr.providers.qwen3-gguf.url is missing"));
        assert!(message.contains("translation.providers.hunyuan.url is missing"));
    }
}
