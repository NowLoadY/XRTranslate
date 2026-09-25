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
    ModelAssetId, ModelAssetsConfig, ModelCapability, ModelFileRole, ModelLevel, ModelRuntime,
    ResolvedModelAsset, ResolvedModelAssets, TranslationPromptStyle, tier_default_manifest,
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

    pub(crate) fn managed_server_specs(
        &self,
        asr_port: u16,
        translation_port: u16,
    ) -> Result<(Option<LlamaServerSpec>, Option<LlamaServerSpec>), String> {
        if (self.asr_uses_llama_server() || self.translation_uses_llama_server())
            && !self.native_runtime.as_ref().is_some_and(|runtime| {
                runtime.llama_cpp_backend == Some(NativeRuntimeBackend::Cuda)
            })
        {
            return Err(
                "Managed ASR and translation models require a verified CUDA runtime marker; CPU fallback is disabled."
                    .to_owned(),
            );
        }
        let asr = if self.asr_uses_llama_server() {
            Some(self.managed_server_spec(ModelCapability::Asr, asr_port, self.asr_runtime())?)
        } else {
            None
        };
        let translation = if self.translation_uses_llama_server() {
            Some(self.managed_server_spec(
                ModelCapability::Translation,
                translation_port,
                self.translation_runtime(),
            )?)
        } else {
            None
        };
        Ok((asr, translation))
    }

    fn managed_server_spec(
        &self,
        capability: ModelCapability,
        port: u16,
        settings: LocalModelRuntimeConfig,
    ) -> Result<LlamaServerSpec, String> {
        let asset = self.assets.active_asset(capability);
        let (role, mmproj, alias, flash_attention) = match asset.manifest().runtime {
            Some(ModelRuntime::LlamaAudioChat { model_alias, .. })
                if capability == ModelCapability::Asr =>
            {
                (
                    LlamaServerRole::Asr,
                    Some(model_file(asset, ModelFileRole::MultimodalProjection)?),
                    model_alias,
                    false,
                )
            }
            Some(ModelRuntime::LlamaTextChat {
                model_alias,
                flash_attention,
                ..
            }) if capability == ModelCapability::Translation => (
                LlamaServerRole::Translation,
                None,
                model_alias,
                flash_attention,
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
    let Some(key) = provider.model_asset.as_deref() else {
        return tier_default_manifest(&provider.provider, capability, ModelLevel::Normal)
            .map(|manifest| manifest.id)
            .ok_or_else(|| {
                format!(
                    "provider {:?} has no default {capability:?} model card",
                    provider.provider
                )
            });
    };
    let id = ModelAssetId::from_config_key(key).ok_or_else(|| {
        format!(
            "unknown model asset {key:?} for provider {:?}",
            provider.provider
        )
    })?;
    let manifest = xrtranslate_assets::manifest_for(id);
    if manifest.provider != provider.provider || manifest.capability != capability {
        return Err(format!(
            "model asset {key:?} does not belong to provider {:?} for {capability:?}",
            provider.provider
        ));
    }
    Ok(id)
}

fn resolve_model_assets(
    config: &AppConfig,
    project_root: &Path,
    active_asset_ids: impl IntoIterator<Item = ModelAssetId>,
) -> ResolvedModelAssets {
    let mut assets = ModelAssetsConfig::with_directory_overrides(
        config.model_manager.models_directory.clone(),
        config.model_manager.qwen3_asr_gguf_directory.clone(),
        config.model_manager.hunyuan_mt_gguf_directory.clone(),
    );
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn attach_test_cuda_runtime(plan: &mut NativeProviderPlan) {
        plan.native_runtime = Some(ResolvedNativeRuntimeSelection {
            backend: NativeRuntimeBackend::Cuda,
            llama_cpp_backend: Some(NativeRuntimeBackend::Cuda),
            onnx_backend: Some(NativeRuntimeBackend::Cuda),
            cuda_version: Some("13.3".into()),
            provider_dir: None,
            onnx_core_library: None,
            cuda_bin_dir: None,
            cudnn_bin_dir: None,
            preload_libraries: Vec::new(),
            fallback_reason: None,
        });
    }

    #[test]
    fn managed_cuda_directory_is_injected_only_into_llama_child_path() {
        let cuda = std::env::temp_dir().join("xrtranslate-managed-cuda");
        let runtime = ResolvedNativeRuntimeSelection {
            backend: xrtranslate_config::NativeRuntimeBackend::Cuda,
            llama_cpp_backend: Some(xrtranslate_config::NativeRuntimeBackend::Cuda),
            onnx_backend: None,
            cuda_version: Some("13.3".into()),
            provider_dir: None,
            onnx_core_library: None,
            cuda_bin_dir: Some(cuda.clone()),
            cudnn_bin_dir: None,
            preload_libraries: Vec::new(),
            fallback_reason: None,
        };
        let mut spec = LlamaServerSpec::new(
            xrtranslate_supervisor::LlamaServerRole::Translation,
            "llama-server",
            "model.gguf",
            None,
            "text-model",
        );

        apply_managed_runtime_environment(&mut spec, Some(&runtime)).unwrap();

        let path = spec
            .environment
            .iter()
            .find(|(name, _)| {
                name == if cfg!(windows) {
                    "PATH"
                } else {
                    "LD_LIBRARY_PATH"
                }
            })
            .map(|(_, value)| value)
            .unwrap();
        assert_eq!(std::env::split_paths(path).next().as_ref(), Some(&cuda));
    }

    #[test]
    fn big_translation_level_selects_the_7b_model_for_backend_launch() {
        let mut document: serde_json::Value =
            serde_json::from_str(include_str!("../../../config.json")).unwrap();
        document["translation"]["providers"]["hunyuan"]["model_asset"] =
            serde_json::Value::from("hy-mt2-big");
        let config = AppConfig::from_value(document).unwrap();

        let mut plan = NativeProviderPlan::resolve(&config, Path::new("release-root")).unwrap();
        attach_test_cuda_runtime(&mut plan);
        let asset = plan.assets.active_asset(ModelCapability::Translation);

        assert_eq!(asset.manifest().id, ModelAssetId::HunyuanMt7bGguf);
        assert_eq!(
            asset.required_file_path(0),
            PathBuf::from("release-root/models/Hy-MT2-7B-GGUF/Hy-MT2-7B-Q4_K_M.gguf")
        );
    }

    #[test]
    fn r2t2_selection_resolves_both_gguf_files_for_managed_asr() {
        let mut document: serde_json::Value =
            serde_json::from_str(include_str!("../../../config.json")).unwrap();
        document["asr"]["providers"]["qwen3-gguf"]["model_asset"] =
            serde_json::Value::from("confucius4-r2t2-q8-gguf");
        let config = AppConfig::from_value(document).unwrap();
        let mut plan = NativeProviderPlan::resolve(&config, Path::new("release-root")).unwrap();
        attach_test_cuda_runtime(&mut plan);

        assert_eq!(
            plan.assets.active_asset(ModelCapability::Asr).manifest().id,
            ModelAssetId::Confucius4R2t2Q8Gguf
        );
        let (asr, _) = plan.managed_server_specs(8101, 8102).unwrap();
        let asr = asr.unwrap();
        assert_eq!(asr.model_alias, "r2t2-asr");
        assert_eq!(plan.asr_prompt_mode(), AsrPromptMode::ContextBias);
        assert!(plan.asr_supports_vocabulary_bias());
        assert!(asr.model.ends_with("Confucius4-R2T2-Q8_0.gguf"));
        assert!(
            asr.mmproj
                .unwrap()
                .ends_with("mmproj-Confucius4-R2T2-Q8_0.gguf")
        );
    }

    #[test]
    fn runtime_plan_materializes_both_managed_server_specs() {
        let config = AppConfig::from_json_str(include_str!("../../../config.json")).unwrap();
        let mut plan = NativeProviderPlan::resolve(&config, Path::new("release-root")).unwrap();
        attach_test_cuda_runtime(&mut plan);

        assert_eq!(
            plan.llama_server_path(),
            Path::new("release-root")
                .join("runtime/llama.cpp")
                .join(format!("llama-server{}", std::env::consts::EXE_SUFFIX))
        );
        let (asr, translation) = plan.managed_server_specs(8101, 8102).unwrap();
        let asr = asr.unwrap();
        let translation = translation.unwrap();

        assert_eq!(asr.model_alias, "qwen3-asr");
        assert_eq!(translation.model_alias, "hy-mt2");
        assert_eq!(asr.endpoint.port, 8101);
        assert_eq!(translation.endpoint.port, 8102);
        assert_eq!(asr.parallel_slots, Some(1));
        assert_eq!(translation.context_size, 4_096);
        assert_eq!(translation.parallel_slots, Some(2));
    }

    #[test]
    fn managed_port_override_updates_adapter_urls_without_changing_model_selection() {
        let config = AppConfig::from_json_str(include_str!("../../../config.json")).unwrap();
        let mut plan = NativeProviderPlan::resolve(&config, Path::new("release-root")).unwrap();
        let asr_alias = plan.asr_model_alias().to_owned();
        let translation_alias = plan.translation_model_alias().to_owned();

        plan.set_managed_ports(Some(38101), Some(38102)).unwrap();

        assert!(plan.asr_url().contains(":38101/"));
        assert!(plan.translation_url().contains(":38102/"));
        assert_eq!(plan.asr_model_alias(), asr_alias);
        assert_eq!(plan.translation_model_alias(), translation_alias);
    }

    #[test]
    fn runtime_plan_preserves_explicit_external_server_path() {
        let mut document: serde_json::Value =
            serde_json::from_str(include_str!("../../../config.json")).unwrap();
        document["model_manager"]["llama_server_path"] =
            serde_json::Value::from("/opt/llama.cpp/llama-server");
        let config = AppConfig::from_value(document).unwrap();
        let plan = NativeProviderPlan::resolve(&config, Path::new("/srv/xrtranslate")).unwrap();

        assert_eq!(
            plan.llama_server_path(),
            Path::new("/opt/llama.cpp/llama-server")
        );
    }

    #[test]
    fn unsupported_provider_is_rejected_at_the_runtime_factory_boundary() {
        let mut document: serde_json::Value =
            serde_json::from_str(include_str!("../../../config.json")).unwrap();
        document["translation"]["provider"] = serde_json::Value::from("future-provider");
        document["translation"]["providers"]["future-provider"] = serde_json::json!({
            "url": "http://127.0.0.1:8010/v1/chat/completions",
            "model_asset": "hy-mt2"
        });
        let config = AppConfig::from_value(document).unwrap();

        let error = NativeProviderPlan::resolve(&config, Path::new("release-root")).unwrap_err();

        assert!(error.contains("unsupported translation provider"));
    }

    #[test]
    fn legacy_missing_asset_keys_use_provider_profile_defaults() {
        let mut document: serde_json::Value =
            serde_json::from_str(include_str!("../../../config.json")).unwrap();
        document["asr"]["providers"]["qwen3-gguf"]
            .as_object_mut()
            .unwrap()
            .remove("model_asset");
        document["translation"]["providers"]["hunyuan"]
            .as_object_mut()
            .unwrap()
            .remove("model_asset");
        let config = AppConfig::from_value(document).unwrap();

        let plan = NativeProviderPlan::resolve(&config, Path::new("release-root")).unwrap();

        assert_eq!(
            plan.assets.active_asset(ModelCapability::Asr).manifest().id,
            ModelAssetId::Qwen3AsrGguf
        );
        assert_eq!(
            plan.assets
                .active_asset(ModelCapability::Translation)
                .manifest()
                .id,
            ModelAssetId::HunyuanMtGguf
        );
    }

    #[test]
    fn normalized_provider_selection_drives_assets_and_capabilities_once() {
        let mut document: serde_json::Value =
            serde_json::from_str(include_str!("../../../config.json")).unwrap();
        document["translation"]["provider"] = serde_json::Value::from(" hunyuan ");
        document["translation"]["providers"]["hunyuan"]["model_asset"] =
            serde_json::Value::from("hy-mt2-big");
        let config = AppConfig::from_value(document).unwrap();

        let plan = NativeProviderPlan::resolve(&config, Path::new("release-root")).unwrap();

        assert_eq!(
            plan.assets
                .active_asset(ModelCapability::Translation)
                .manifest()
                .id,
            ModelAssetId::HunyuanMt7bGguf
        );
        assert!(plan.translation_supports_reference_context());
    }

    #[test]
    fn remote_routes_skip_native_assets_and_use_configured_models() {
        let mut document: serde_json::Value =
            serde_json::from_str(include_str!("../../../config.json")).unwrap();
        document["asr"]["provider"] = serde_json::Value::from("openai-custom");
        document["translation"]["provider"] = serde_json::Value::from("openai-custom");
        let mut asr_remote = document["asr"]["providers"]["openai"].clone();
        let mut translation_remote = document["translation"]["providers"]["openai"].clone();
        asr_remote["api_key"] = serde_json::Value::from("test-key");
        translation_remote["api_key"] = serde_json::Value::from("test-key");
        document["asr"]["providers"]["openai-custom"] = asr_remote;
        document["translation"]["providers"]["openai-custom"] = translation_remote;
        let config = AppConfig::from_value(document).unwrap();
        let plan = NativeProviderPlan::resolve(&config, Path::new("release-root")).unwrap();

        assert!(!plan.uses_local_runtime());
        assert!(plan.check_assets().is_ok());
        assert_eq!(plan.asr_model_alias(), "gpt-4o-transcribe");
        assert_eq!(plan.translation_model_alias(), "gpt-4o-mini");
        assert!(plan.managed_server_specs(8101, 8102).unwrap().0.is_none());
        assert!(plan.managed_server_specs(8101, 8102).unwrap().1.is_none());
    }

    #[test]
    fn qwen_registers_as_a_remote_context_bias_profile() {
        let mut document: serde_json::Value =
            serde_json::from_str(include_str!("../../../config.json")).unwrap();
        document["asr"]["provider"] = serde_json::Value::from("qwen");
        document["asr"]["providers"]["qwen"]["api_key"] = serde_json::Value::from("dashscope-key");
        let config = AppConfig::from_value(document).unwrap();

        let mut plan = NativeProviderPlan::resolve(&config, Path::new("release-root")).unwrap();
        attach_test_cuda_runtime(&mut plan);

        assert!(!plan.asr_uses_llama_server());
        assert_eq!(plan.asr_model_alias(), "qwen3-asr-flash");
        assert_eq!(plan.asr_prompt_mode(), AsrPromptMode::ContextBias);
        assert!(plan.managed_server_specs(8101, 8102).unwrap().0.is_none());
        assert!(matches!(
            plan.asr_adapter(plan.asr_http_client().unwrap()).unwrap(),
            NativeAsrAdapter::AudioChat(_)
        ));
    }

    #[test]
    fn qwen_registers_as_a_remote_translation_profile() {
        let mut document: serde_json::Value =
            serde_json::from_str(include_str!("../../../config.json")).unwrap();
        document["translation"]["provider"] = serde_json::Value::from("qwen");
        document["translation"]["providers"]["qwen"]["api_key"] =
            serde_json::Value::from("dashscope-key");
        let config = AppConfig::from_value(document).unwrap();

        let mut plan = NativeProviderPlan::resolve(&config, Path::new("release-root")).unwrap();
        attach_test_cuda_runtime(&mut plan);

        assert!(!plan.translation_uses_llama_server());
        assert_eq!(plan.translation_model_alias(), "qwen-mt-flash");
        assert!(plan.managed_server_specs(8101, 8102).unwrap().1.is_none());
        let adapter = plan
            .translation_adapter(plan.translation_http_client().unwrap())
            .unwrap();
        assert_eq!(
            adapter.provider(),
            xrtranslate_inference::TranslationProvider::Qwen
        );
    }

    #[test]
    fn sensevoice_runs_without_a_llama_server_or_cuda_marker() {
        let mut document: serde_json::Value =
            serde_json::from_str(include_str!("../../../config.json")).unwrap();
        document["asr"]["providers"]["qwen3-gguf"]["model_asset"] =
            serde_json::Value::from(ModelAssetId::SenseVoiceSmallInt8Onnx.as_str());
        document["asr"]["providers"]["qwen3-gguf"]["transport"] =
            serde_json::Value::from("onnx-cpu");
        document["translation"]["provider"] = serde_json::Value::from("qwen");
        document["translation"]["providers"]["qwen"]["api_key"] =
            serde_json::Value::from("test-key");
        let config = AppConfig::from_value(document).unwrap();
        assert!(!config.runtime_requirements().llama_cpp);
        let plan = NativeProviderPlan::resolve(&config, Path::new("release-root")).unwrap();
        assert!(!plan.asr_uses_llama_server());
        assert_eq!(plan.asr_prompt_mode(), AsrPromptMode::None);
        assert!(!plan.asr_supports_vocabulary_bias());
        assert!(plan.managed_server_specs(0, 0).unwrap().0.is_none());
    }

    #[test]
    fn haidass_uses_its_own_alias_and_translation_profile() {
        let mut document: serde_json::Value =
            serde_json::from_str(include_str!("../../../config.json")).unwrap();
        document["asr"]["provider"] = serde_json::Value::from("qwen");
        document["asr"]["providers"]["qwen"]["api_key"] = serde_json::Value::from("test-key");
        document["translation"]["providers"]["hunyuan"]["model_asset"] =
            serde_json::Value::from(ModelAssetId::HaidassTranslate143mQ8Gguf.as_str());
        let config = AppConfig::from_value(document).unwrap();
        let mut plan = NativeProviderPlan::resolve(&config, Path::new("release-root")).unwrap();
        attach_test_cuda_runtime(&mut plan);
        assert_eq!(plan.translation_model_alias(), "haidass-translate");
        let spec = plan.managed_server_specs(0, 8102).unwrap().1.unwrap();
        assert_eq!(spec.model_alias, "haidass-translate");
        assert!(spec.model.ends_with("Haidass-Translate-143M.Q8_0.gguf"));
        let adapter = plan
            .translation_adapter(plan.translation_http_client().unwrap())
            .unwrap();
        assert_eq!(adapter.provider(), TranslationProvider::Bilingual);
    }

    #[test]
    fn every_catalog_provider_has_a_backend_runtime_profile() {
        for manifest in xrtranslate_assets::MODEL_ASSET_CATALOG {
            let registered = match manifest.capability {
                ModelCapability::Asr => {
                    AsrProfile::registered(manifest.provider, "local").is_some()
                }
                ModelCapability::Translation => {
                    TranslationProfile::registered(manifest.provider, "local").is_some()
                }
                ModelCapability::Tts => TtsProfile::registered(manifest.provider, "onnx").is_some(),
            };
            assert!(
                registered,
                "catalog provider {} has no backend runtime profile",
                manifest.provider
            );
        }
    }

    #[test]
    fn generic_pipeline_does_not_name_a_concrete_model_provider() {
        let pipeline = include_str!("pipeline.rs");
        for concrete in ["Qwen3", "Hunyuan", "TranslationProvider"] {
            assert!(
                !pipeline.contains(concrete),
                "pipeline must consume provider-neutral adapters, found {concrete}"
            );
        }
    }
}
