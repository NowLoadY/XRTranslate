//! Resolves configured model providers into one backend runtime plan.
//!
//! Provider-specific knowledge belongs here: the transport entrypoint and
//! session pipeline consume this plan without branching on model names.

mod asr;
mod onnx;
mod translation;
mod tts;

use std::{
    ffi::OsString,
    net::{IpAddr, Ipv4Addr},
    path::Path,
};

use xrtranslate_assets::{
    ModelAssetId, ModelCapability, ModelFileRole, ModelRuntime, ResolvedModelAsset,
    ResolvedModelAssets, TranslationPromptStyle,
};
use xrtranslate_config::{
    AppConfig, AsrPromptMode, LocalModelRuntimeConfig, NativeModelRouteConfig,
    NativeRuntimeBackend, NativeRuntimeSelection, ResolvedNativeRuntimeSelection, RuntimeLayout,
};
use xrtranslate_inference::{
    InferenceError, ReqwestClient, TranslationAdapter, TranslationProvider,
};
use xrtranslate_supervisor::{
    FlashAttention, LlamaServerEndpoint, LlamaServerRole, LlamaServerSpec,
};

use asr::AsrProfile;
pub(crate) use asr::{NativeAsrAdapter, NativeAsrOptions};
pub(crate) use onnx::{OnnxRuntimeDiagnostic, initialize_managed_onnx_runtime, runtime_diagnostic};
use translation::TranslationProfile;
pub(crate) use tts::NativeTtsAdapter;
use tts::TtsProfile;

#[derive(Clone, Debug)]
pub(crate) struct NativeProviderPlan {
    route: NativeModelRouteConfig,
    pub(crate) language_capabilities: xrtranslate_engine::language::LanguageCapabilities,
    assets: ResolvedModelAssets,
    asr_profile: AsrProfile,
    translation_profile: TranslationProfile,
    tts_profile: Option<TtsProfile>,
    translation_supports_reference_context: bool,
    native_runtime: Option<ResolvedNativeRuntimeSelection>,
}

impl NativeProviderPlan {
    pub(crate) fn resolve(config: &AppConfig, project_root: &Path) -> Result<Self, String> {
        let mut route = config
            .native_model_route()
            .map_err(|error| error.to_string())?;
        let runtime_layout = config.runtime_layout(project_root);
        route.llama_server_path =
            runtime_layout.resolve_llama_server_path(&route.llama_server_path);
        let native_runtime = load_native_runtime_selection(&runtime_layout)?;
        let asr_profile = AsrProfile::registered(&route.asr.provider, &route.asr.transport)
            .ok_or_else(|| format!("unsupported ASR provider {:?}", route.asr.provider))?;
        let translation_profile = TranslationProfile::registered(
            &route.translation.provider,
            &route.translation.transport,
        )
        .ok_or_else(|| {
            format!(
                "unsupported translation provider {:?}",
                route.translation.provider
            )
        })?;
        let tts_profile = TtsProfile::selected(config)?;
        let asr_asset_id = route
            .asr
            .uses_local_runtime()
            .then(|| route_asset_id(&route.asr, ModelCapability::Asr))
            .transpose()?;
        let translation_asset_id = route
            .translation
            .uses_local_runtime()
            .then(|| route_asset_id(&route.translation, ModelCapability::Translation))
            .transpose()?;
        let tts_asset_ids = tts_profile
            .map(|profile| profile.configured_assets(config))
            .transpose()?
            .unwrap_or_default();
        let assets = resolve_model_assets(
            config,
            project_root,
            asr_asset_id
                .into_iter()
                .chain(translation_asset_id)
                .chain(tts_asset_ids),
        );
        let translation_supports_reference_context = translation_asset_id
            .and_then(|id| xrtranslate_assets::manifest_for(id).runtime)
            .and_then(|runtime| match runtime {
                ModelRuntime::LlamaTextChat {
                    allow_reference_context,
                    ..
                } => Some(allow_reference_context),
                _ => None,
            })
            .unwrap_or(route.translation.supports_prompt_context);

        Ok(Self {
            language_capabilities: route.language_capabilities()?,
            route,
            assets,
            asr_profile,
            translation_profile,
            tts_profile,
            translation_supports_reference_context,
            native_runtime,
        })
    }

    pub(crate) fn check_assets(&self) -> Result<(), String> {
        if !self.uses_local_runtime() {
            return Ok(());
        }
        self.assets
            .check()
            .into_result()
            .map_err(|error| error.to_string())
    }

    pub(crate) fn check_capability_assets(
        &self,
        capability: ModelCapability,
    ) -> Result<(), String> {
        let diagnostics = self
            .assets
            .active_assets_for(capability)
            .flat_map(ResolvedModelAsset::check)
            .collect::<Vec<_>>();
        if diagnostics.is_empty() {
            Ok(())
        } else {
            Err(diagnostics
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n"))
        }
    }

    pub(crate) fn managed_server_for(
        &self,
        capability: ModelCapability,
    ) -> Result<Option<LlamaServerSpec>, String> {
        let (uses_llama, url, runtime) = match capability {
            ModelCapability::Asr => (
                self.asr_uses_llama_server(),
                self.asr_url(),
                self.asr_runtime(),
            ),
            ModelCapability::Translation => (
                self.translation_uses_llama_server(),
                self.translation_url(),
                self.translation_runtime(),
            ),
            _ => return Ok(None),
        };
        if !uses_llama {
            return Ok(None);
        }
        if !self.native_runtime.as_ref().is_some_and(|runtime| {
            matches!(
                runtime.llama_cpp_backend,
                Some(NativeRuntimeBackend::Cuda | NativeRuntimeBackend::Vulkan)
            ) || (cfg!(target_os = "android")
                && runtime.llama_cpp_backend == Some(NativeRuntimeBackend::Cpu))
        }) {
            return Err("Managed models require a verified CUDA or Vulkan runtime marker; CPU fallback is disabled.".into());
        }
        self.managed_server_spec(capability, crate::local_endpoint_port(url)?, runtime)
            .map(Some)
    }

    pub(crate) fn uses_local_runtime(&self) -> bool {
        self.route.uses_local_runtime() || self.tts_profile.is_some()
    }

    pub(crate) fn asr_uses_llama_server(&self) -> bool {
        self.route.asr.uses_local_runtime()
            && self
                .assets
                .active_asset(ModelCapability::Asr)
                .manifest()
                .runtime
                .is_some_and(ModelRuntime::uses_llama_cpp)
    }

    pub(crate) fn translation_uses_llama_server(&self) -> bool {
        self.route.translation.uses_local_runtime()
            && self
                .assets
                .active_asset(ModelCapability::Translation)
                .manifest()
                .runtime
                .is_some_and(ModelRuntime::uses_llama_cpp)
    }

    pub(crate) fn asr_http_client(&self) -> Result<ReqwestClient, String> {
        if self.asr_uses_llama_server() {
            ReqwestClient::with_default_direct_timeout().map_err(|error| error.to_string())
        } else {
            ReqwestClient::with_default_timeout().map_err(|error| error.to_string())
        }
    }

    pub(crate) fn translation_http_client(&self) -> Result<ReqwestClient, String> {
        if self.translation_uses_llama_server() {
            ReqwestClient::with_default_direct_timeout().map_err(|error| error.to_string())
        } else {
            ReqwestClient::with_default_timeout().map_err(|error| error.to_string())
        }
    }

    pub(crate) fn llama_server_path(&self) -> &Path {
        &self.route.llama_server_path
    }

    pub(crate) fn asr_runtime(&self) -> LocalModelRuntimeConfig {
        self.route.asr.runtime
    }

    pub(crate) fn translation_runtime(&self) -> LocalModelRuntimeConfig {
        self.route.translation.runtime
    }

    pub(crate) fn asr_url(&self) -> &str {
        &self.route.asr.url
    }

    pub(crate) fn translation_url(&self) -> &str {
        &self.route.translation.url
    }

    pub(crate) fn set_managed_ports(
        &mut self,
        asr_port: Option<u16>,
        translation_port: Option<u16>,
    ) -> Result<(), String> {
        fn set_port(url: &mut String, port: u16) -> Result<(), String> {
            let mut parsed = url::Url::parse(url)
                .map_err(|error| format!("invalid managed model URL {url:?}: {error}"))?;
            parsed
                .set_host(Some("127.0.0.1"))
                .map_err(|_| format!("cannot set local host for managed model URL {url:?}"))?;
            parsed
                .set_port(Some(port))
                .map_err(|_| format!("cannot set port for managed model URL {url:?}"))?;
            *url = parsed.into();
            Ok(())
        }

        if let Some(port) = asr_port {
            set_port(&mut self.route.asr.url, port)?;
        }
        if let Some(port) = translation_port {
            set_port(&mut self.route.translation.url, port)?;
        }
        Ok(())
    }

    pub(crate) fn asr_model_alias(&self) -> &str {
        if self.route.asr.uses_local_runtime() {
            let manifest = self.assets.active_asset(ModelCapability::Asr).manifest();
            return manifest
                .runtime
                .and_then(ModelRuntime::model_alias)
                .unwrap_or(manifest.id.as_str());
        }
        &self.route.asr.model
    }

    pub(crate) fn asr_prompt_mode(&self) -> AsrPromptMode {
        if self.route.asr.uses_local_runtime() {
            return match self
                .assets
                .active_asset(ModelCapability::Asr)
                .manifest()
                .runtime
            {
                Some(ModelRuntime::LlamaAudioChat {
                    context_bias: true, ..
                }) => AsrPromptMode::ContextBias,
                _ => AsrPromptMode::None,
            };
        }
        self.route.asr.asr_prompt_mode
    }

    pub(crate) fn asr_supports_vocabulary_bias(&self) -> bool {
        if self.route.asr.uses_local_runtime() {
            return matches!(
                self.assets
                    .active_asset(ModelCapability::Asr)
                    .manifest()
                    .runtime,
                Some(ModelRuntime::LlamaAudioChat {
                    vocabulary_bias: true,
                    ..
                })
            );
        }
        self.route.asr.supports_vocabulary_bias
    }

    pub(crate) fn asr_context_max_chars(&self) -> Option<usize> {
        self.route.asr.asr_context_max_chars
    }

    pub(crate) fn asr_vocabulary_weight(&self) -> u8 {
        self.route.asr.vocabulary_weight
    }

    pub(crate) fn translation_model_alias(&self) -> &str {
        if self.route.translation.uses_local_runtime() {
            let manifest = self
                .assets
                .active_asset(ModelCapability::Translation)
                .manifest();
            return manifest
                .runtime
                .and_then(ModelRuntime::model_alias)
                .unwrap_or(manifest.id.as_str());
        }
        &self.route.translation.model
    }

    pub(crate) fn translation_supports_reference_context(&self) -> bool {
        self.translation_supports_reference_context
    }

    pub(crate) fn asr_adapter(
        &self,
        http: ReqwestClient,
    ) -> Result<NativeAsrAdapter, InferenceError> {
        if self.route.asr.uses_local_runtime() {
            let asset = self.assets.active_asset(ModelCapability::Asr);
            let runtime =
                asset
                    .manifest()
                    .runtime
                    .ok_or_else(|| InferenceError::InvalidConfiguration {
                        field: "asr.model_asset.runtime",
                        message: "selected ASR model card has no runtime".into(),
                    })?;
            return asr::local_adapter(runtime, asset, http, self.asr_url());
        }
        self.asr_profile.adapter(
            http,
            self.asr_url(),
            self.asr_model_alias(),
            self.route.asr.api_key.as_deref(),
        )
    }

    pub(crate) fn translation_adapter(
        &self,
        http: ReqwestClient,
    ) -> Result<TranslationAdapter<ReqwestClient>, InferenceError> {
        if self.route.translation.uses_local_runtime() {
            let runtime = self
                .assets
                .active_asset(ModelCapability::Translation)
                .manifest()
                .runtime;
            let provider = match runtime {
                Some(ModelRuntime::LlamaTextChat {
                    prompt_style: TranslationPromptStyle::Contextual,
                    ..
                }) => TranslationProvider::Contextual,
                Some(ModelRuntime::LlamaTextChat {
                    prompt_style: TranslationPromptStyle::Bilingual,
                    ..
                }) => TranslationProvider::Bilingual,
                _ => {
                    return Err(InferenceError::InvalidConfiguration {
                        field: "translation.model_asset.runtime",
                        message: "selected translation model card has no supported runtime".into(),
                    });
                }
            };
            return TranslationAdapter::new(
                http,
                self.translation_url(),
                self.translation_model_alias(),
                provider,
            );
        }
        self.translation_profile.adapter(
            http,
            self.translation_url(),
            self.translation_model_alias(),
            self.route.translation.api_key.as_deref(),
        )
    }

    pub(crate) fn tts_adapter(
        &self,
        config: &AppConfig,
    ) -> Result<Option<NativeTtsAdapter>, String> {
        if self.tts_profile.is_some()
            && !self
                .native_runtime
                .as_ref()
                .is_some_and(|runtime| runtime.onnx_backend == Some(NativeRuntimeBackend::Cuda))
        {
            return Err(
                "Managed TTS models require a verified CUDA/cuDNN runtime marker; CPU fallback is disabled."
                    .to_owned(),
            );
        }
        self.tts_profile
            .map(|profile| {
                let assets = self
                    .assets
                    .active_assets_for(ModelCapability::Tts)
                    .collect::<Vec<_>>();
                profile.adapter(config, &assets)
            })
            .transpose()
    }

    fn managed_server_spec(
        &self,
        capability: ModelCapability,
        port: u16,
        settings: LocalModelRuntimeConfig,
    ) -> Result<LlamaServerSpec, String> {
        let asset = self.assets.active_asset(capability);
        let (role, mmproj, alias, flash_attention, extra_args) = match asset.manifest().runtime {
            Some(ModelRuntime::LlamaAudioChat {
                model_alias,
                extra_args,
                ..
            }) if capability == ModelCapability::Asr => (
                LlamaServerRole::Asr,
                Some(model_file(asset, ModelFileRole::MultimodalProjection)?),
                model_alias,
                false,
                extra_args,
            ),
            Some(ModelRuntime::LlamaTextChat {
                model_alias,
                flash_attention,
                extra_args,
                ..
            }) if capability == ModelCapability::Translation => (
                LlamaServerRole::Translation,
                None,
                model_alias,
                flash_attention,
                extra_args,
            ),
            _ => {
                return Err(format!(
                    "model card {} has no {capability:?} llama.cpp runtime",
                    asset.manifest().id
                ));
            }
        };
        let mut spec = LlamaServerSpec::new(
            role,
            self.llama_server_path(),
            model_file(asset, ModelFileRole::Weights)?,
            mmproj,
            alias,
        )
        .with_endpoint(LlamaServerEndpoint::new(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            port,
        ));
        if flash_attention {
            spec.flash_attention = Some(FlashAttention::On);
        }
        for arg in extra_args {
            spec.extra_args.push(OsString::from(arg));
        }
        apply_model_runtime(&mut spec, settings)?;
        apply_managed_runtime_environment(&mut spec, self.native_runtime.as_ref())?;
        Ok(spec)
    }
}

fn load_native_runtime_selection(
    layout: &RuntimeLayout,
) -> Result<Option<ResolvedNativeRuntimeSelection>, String> {
    let path = layout.native_runtime_selection_file();
    if !path.is_file() {
        return Ok(None);
    }
    let selection: NativeRuntimeSelection = serde_json::from_slice(
        &std::fs::read(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?,
    )
    .map_err(|error| format!("invalid {}: {error}", path.display()))?;
    if selection.schema_version != 1 {
        return Err(format!(
            "unsupported native runtime marker schema {} in {}",
            selection.schema_version,
            path.display()
        ));
    }
    Ok(Some(layout.resolve_native_runtime_selection(&selection)))
}

fn apply_managed_runtime_environment(
    spec: &mut LlamaServerSpec,
    runtime: Option<&ResolvedNativeRuntimeSelection>,
) -> Result<(), String> {
    let Some(runtime) = runtime else {
        return Ok(());
    };
    #[cfg(target_os = "android")]
    if runtime.llama_cpp_backend == Some(NativeRuntimeBackend::Cpu) {
        spec.gpu_layers = xrtranslate_supervisor::GpuLayers::None;
        spec.extra_args.extend(["--device".into(), "none".into()]);
        return Ok(());
    }
    if runtime.llama_cpp_backend == Some(NativeRuntimeBackend::Vulkan) {
        let device = runtime
            .vulkan_device
            .ok_or("Managed Vulkan runtime has no selected GPU; run hardware detection again.")?;
        spec.environment
            .push(("GGML_VK_VISIBLE_DEVICES".into(), device.to_string().into()));
        // A required device makes llama.cpp fail clearly if the driver changes;
        // it must not silently run a GPU model on CPU. The visible list has one GPU.
        spec.extra_args
            .extend(["--device".into(), "Vulkan0".into()]);
        return Ok(());
    }
    let Some(cuda_directory) = runtime.cuda_bin_dir.as_ref() else {
        return Ok(());
    };
    #[cfg(windows)]
    let variable = "PATH";
    #[cfg(not(windows))]
    let variable = "LD_LIBRARY_PATH";
    let mut paths = vec![cuda_directory.clone()];
    if let Some(existing) = std::env::var_os(variable) {
        paths.extend(std::env::split_paths(&existing));
    }
    let joined = std::env::join_paths(paths)
        .map_err(|error| format!("cannot build managed CUDA {variable}: {error}"))?;
    spec.environment.push((OsString::from(variable), joined));
    Ok(())
}

fn model_file(
    asset: &ResolvedModelAsset,
    role: ModelFileRole,
) -> Result<std::path::PathBuf, String> {
    asset.file_path(role).ok_or_else(|| {
        format!(
            "model asset {} does not declare required file role {role:?}",
            asset.manifest().id
        )
    })
}

fn route_asset_id(
    provider: &xrtranslate_config::NativeProviderConfig,
    capability: ModelCapability,
) -> Result<ModelAssetId, String> {
    xrtranslate_assets::language::provider_model(
        &provider.provider,
        provider.model_asset.as_deref(),
        capability,
    )
    .map(|model| model.id)
}

fn resolve_model_assets(
    config: &AppConfig,
    project_root: &Path,
    active_asset_ids: impl IntoIterator<Item = ModelAssetId>,
) -> ResolvedModelAssets {
    let mut assets = config.model_asset_paths();
    for id in active_asset_ids {
        assets.select_asset(id);
    }
    assets.resolve(project_root)
}

fn apply_model_runtime(
    spec: &mut LlamaServerSpec,
    runtime: LocalModelRuntimeConfig,
) -> Result<(), String> {
    spec.context_size = runtime
        .context_window_tokens
        .checked_mul(u32::from(runtime.parallel_slots))
        .ok_or("model context_window_tokens × parallel_slots exceeds u32")?;
    // Keep the child server's slot count identical to the backend scheduler.
    // Omitting --parallel for one slot makes llama-server fall back to its
    // own default (currently four slots), which can create hidden GPU work
    // and queueing that the backend cannot account for.
    spec.parallel_slots = Some(runtime.parallel_slots);
    Ok(())
}
