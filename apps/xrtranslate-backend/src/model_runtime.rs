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
    ModelAssetId, ModelAssetsConfig, ModelCapability, ModelFileRole, ResolvedModelAsset,
    ResolvedModelAssets,
};
use xrtranslate_config::{
    AppConfig, AsrPromptMode, LocalModelRuntimeConfig, NativeModelRouteConfig,
    NativeRuntimeBackend, NativeRuntimeSelection, ResolvedNativeRuntimeSelection, RuntimeLayout,
};
use xrtranslate_inference::{InferenceError, ReqwestClient, TranslationAdapter};
use xrtranslate_supervisor::{LlamaServerEndpoint, LlamaServerSpec};

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
        route.llama_server_path = runtime_layout.resolve_configured_path(&route.llama_server_path);
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
            .then(|| {
                route_asset_id(
                    &route.asr,
                    asr_profile.default_asset(),
                    ModelCapability::Asr,
                )
            })
            .transpose()?;
        let translation_asset_id = route
            .translation
            .uses_local_runtime()
            .then(|| {
                route_asset_id(
                    &route.translation,
                    translation_profile.default_asset(),
                    ModelCapability::Translation,
                )
            })
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
        let translation_supports_reference_context = route.translation.supports_prompt_context;

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

    pub(crate) fn asr_uses_local_runtime(&self) -> bool {
        self.route.asr.uses_local_runtime()
    }

    pub(crate) fn translation_uses_local_runtime(&self) -> bool {
        self.route.translation.uses_local_runtime()
    }

    pub(crate) fn asr_http_client(&self) -> Result<ReqwestClient, String> {
        if self.asr_uses_local_runtime() {
            ReqwestClient::with_default_direct_timeout().map_err(|error| error.to_string())
        } else {
            ReqwestClient::with_default_timeout().map_err(|error| error.to_string())
        }
    }

    pub(crate) fn translation_http_client(&self) -> Result<ReqwestClient, String> {
        if self.translation_uses_local_runtime() {
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

    pub(crate) fn asr_model_alias(&self) -> &str {
        self.asr_profile.model_alias(&self.route.asr.model)
    }

    pub(crate) fn asr_prompt_mode(&self) -> AsrPromptMode {
        self.route.asr.asr_prompt_mode
    }

    pub(crate) fn asr_supports_vocabulary_bias(&self) -> bool {
        self.route.asr.supports_vocabulary_bias
    }

    pub(crate) fn asr_context_max_chars(&self) -> Option<usize> {
        self.route.asr.asr_context_max_chars
    }

    pub(crate) fn asr_vocabulary_weight(&self) -> u8 {
        self.route.asr.vocabulary_weight
    }

    pub(crate) fn translation_model_alias(&self) -> &str {
        self.translation_profile
            .model_alias(&self.route.translation.model)
    }

    pub(crate) fn translation_supports_reference_context(&self) -> bool {
        self.translation_supports_reference_context
    }

    pub(crate) fn asr_adapter(
        &self,
        http: ReqwestClient,
    ) -> Result<NativeAsrAdapter, InferenceError> {
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
        if self.route.uses_local_runtime()
            && !self.native_runtime.as_ref().is_some_and(|runtime| {
                runtime.llama_cpp_backend == Some(NativeRuntimeBackend::Cuda)
            })
        {
            return Err(
                "Managed ASR and translation models require a verified CUDA runtime marker; CPU fallback is disabled."
                    .to_owned(),
            );
        }
        let bind = |port| LlamaServerEndpoint::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
        let asr = if self.asr_uses_local_runtime() {
            let asset = self.assets.active_asset(ModelCapability::Asr);
            let mut spec = LlamaServerSpec::qwen3_asr_gguf(
                self.llama_server_path(),
                model_file(asset, ModelFileRole::Weights)?,
                model_file(asset, ModelFileRole::MultimodalProjection)?,
            )
            .with_endpoint(bind(asr_port));
            apply_model_runtime(&mut spec, self.asr_runtime())?;
            apply_managed_runtime_environment(&mut spec, self.native_runtime.as_ref())?;
            Some(spec)
        } else {
            None
        };
        let translation = if self.translation_uses_local_runtime() {
            let asset = self.assets.active_asset(ModelCapability::Translation);
            let mut spec = LlamaServerSpec::hunyuan_mt_gguf(
                self.llama_server_path(),
                model_file(asset, ModelFileRole::Weights)?,
            )
            .with_endpoint(bind(translation_port));
            apply_model_runtime(&mut spec, self.translation_runtime())?;
            apply_managed_runtime_environment(&mut spec, self.native_runtime.as_ref())?;
            Some(spec)
        } else {
            None
        };
        Ok((asr, translation))
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
    let mut paths = vec![cuda_directory.clone()];
    if let Some(existing) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    let joined = std::env::join_paths(paths)
        .map_err(|error| format!("cannot build managed CUDA PATH: {error}"))?;
    spec.environment.push((OsString::from("PATH"), joined));
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
    fallback: ModelAssetId,
    capability: ModelCapability,
) -> Result<ModelAssetId, String> {
    let Some(key) = provider.model_asset.as_deref() else {
        return Ok(fallback);
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
    spec.parallel_slots = (runtime.parallel_slots > 1).then_some(runtime.parallel_slots);
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
        let mut spec = LlamaServerSpec::hunyuan_mt_gguf("llama-server", "model.gguf");

        apply_managed_runtime_environment(&mut spec, Some(&runtime)).unwrap();

        let path = spec
            .environment
            .iter()
            .find(|(name, _)| name == "PATH")
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
        assert_eq!(translation.context_size, 4_096);
        assert_eq!(translation.parallel_slots, Some(2));
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
    fn qwen_audio_streaming_registers_as_a_remote_context_bias_profile() {
        let mut document: serde_json::Value =
            serde_json::from_str(include_str!("../../../config.json")).unwrap();
        document["asr"]["provider"] = serde_json::Value::from("qwen-audio-streaming");
        document["asr"]["providers"]["qwen-audio-streaming"]["api_key"] =
            serde_json::Value::from("dashscope-key");
        let config = AppConfig::from_value(document).unwrap();

        let mut plan = NativeProviderPlan::resolve(&config, Path::new("release-root")).unwrap();
        attach_test_cuda_runtime(&mut plan);

        assert!(!plan.asr_uses_local_runtime());
        assert_eq!(plan.asr_model_alias(), "qwen-audio-3.0-asr-flash-streaming");
        assert_eq!(plan.asr_prompt_mode(), AsrPromptMode::ContextBias);
        assert_eq!(plan.asr_context_max_chars(), Some(400));
        assert!(plan.asr_supports_vocabulary_bias());
        assert_eq!(plan.asr_vocabulary_weight(), 4);
        assert!(plan.managed_server_specs(8101, 8102).unwrap().0.is_none());
        assert!(matches!(
            plan.asr_adapter(plan.asr_http_client().unwrap()).unwrap(),
            NativeAsrAdapter::QwenAudioStreaming(_)
        ));
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
