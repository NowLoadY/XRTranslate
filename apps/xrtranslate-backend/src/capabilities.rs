//! Shared resources are prepared by the first consumer that needs them.
use crate::{
    BackendState, NativeTtsAdapter, OnnxRuntimeDiagnostic, RuntimeLayout, VoiceLibrary,
    initialize_managed_onnx_runtime, runtime_diagnostic, start_llama_servers,
    wait_for_model_servers,
};
use std::sync::Arc;
use xrtranslate_assets::ModelCapability;

pub(crate) struct SpeechResources {
    pub tts: Option<NativeTtsAdapter>,
    pub voices: Arc<VoiceLibrary>,
    pub diagnostic: OnnxRuntimeDiagnostic,
}

impl BackendState {
    async fn prepare_onnx(&self) -> Result<(), String> {
        self.onnx_runtime
            .get_or_try_init(|| async {
                initialize_managed_onnx_runtime(&self.project_root, &self.config)
            })
            .await
            .copied()
    }

    pub(crate) async fn prepare_audio(&self) -> Result<(), String> {
        self.audio_runtime
            .get_or_try_init(|| async {
                self.model_plan
                    .check_capability_assets(ModelCapability::Asr)?;
                self.prepare_onnx().await?;
                let mut processes = if self.manage_models {
                    start_llama_servers(&self.model_plan, ModelCapability::Asr)?
                } else {
                    Vec::new()
                };
                if self.manage_models {
                    wait_for_model_servers(
                        &self.model_plan,
                        self.model_start_timeout,
                        &mut processes,
                        ModelCapability::Asr,
                    )
                    .await?;
                }
                Ok::<_, String>(processes)
            })
            .await
            .map(|_| ())
    }

    pub(crate) async fn prepare_speech(&self) -> Result<&SpeechResources, String> {
        self.speech_runtime
            .get_or_try_init(|| async {
                let tts = self.model_plan.tts_adapter(&self.config)?;
                let diagnostic = if let Some(adapter) = &tts {
                    self.model_plan
                        .check_capability_assets(ModelCapability::Tts)?;
                    self.prepare_onnx().await?;
                    let device = adapter.prepare().await.map_err(|error| error.to_string())?;
                    runtime_diagnostic(&self.project_root, &self.config, device)
                } else {
                    OnnxRuntimeDiagnostic::default()
                };
                let directory =
                    RuntimeLayout::for_config(&self.project_root, &self.config.model_manager)
                        .voice_clones_directory();
                let voices = Arc::new(
                    VoiceLibrary::open(directory, self.config.tts.provider.clone(), tts.clone())
                        .await,
                );
                Ok::<_, String>(SpeechResources {
                    tts,
                    voices,
                    diagnostic,
                })
            })
            .await
    }
}
