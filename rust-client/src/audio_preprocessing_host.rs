//! Shared model-resource adapter for live inputs and media preprocessing.
use crate::{
    CaptureSource, XRTranslateApp,
    model_install::NativeModelTaskState,
    ui::components::{
        AudioPreprocessingAction, AudioPreprocessingModelState, AudioPreprocessingModels,
    },
};
use xrtranslate_assets::ModelAssetId;
use xrtranslate_protocol::AudioPreprocessing;

pub(crate) fn preprocessing_asset(mode: AudioPreprocessing) -> Option<ModelAssetId> {
    match mode {
        AudioPreprocessing::None => None,
        AudioPreprocessing::Gtcrn => Some(ModelAssetId::GtcrnOnnx),
        AudioPreprocessing::VocalSeparation => Some(ModelAssetId::UvrMdxnet9482Onnx),
    }
}

impl XRTranslateApp {
    pub(crate) fn audio_preprocessing_models(&self) -> AudioPreprocessingModels {
        AudioPreprocessingModels {
            gtcrn: self.audio_preprocessing_model_state(ModelAssetId::GtcrnOnnx),
            vocal: self.audio_preprocessing_model_state(ModelAssetId::UvrMdxnet9482Onnx),
            can_delete: !self.model_task_manager.is_busy(),
            use_mirror: self.model_task_manager.use_mirror(),
        }
    }

    fn audio_preprocessing_model_state(&self, id: ModelAssetId) -> AudioPreprocessingModelState {
        use AudioPreprocessingModelState as State;
        let manager = &self.model_task_manager;
        if manager.is_model_ready(id) {
            return State::Ready;
        }
        let (downloaded_bytes, total_bytes) = manager.model_download_progress(id).unwrap_or((0, 0));
        if manager.is_model_paused(id) {
            return State::Paused {
                downloaded_bytes,
                total_bytes,
            };
        }
        if matches!(manager.state(), NativeModelTaskState::Installing { asset_id, .. } if *asset_id == id)
        {
            return State::Downloading {
                downloaded_bytes,
                total_bytes,
            };
        }
        if manager.is_model_queued(id) {
            return State::Queued;
        }
        match manager.state() {
            NativeModelTaskState::Failed(error)
                if manager
                    .batch_snapshot()
                    .is_some_and(|batch| batch.failed_asset_id == Some(id)) =>
            {
                State::Failed(error.clone())
            }
            NativeModelTaskState::Idle | NativeModelTaskState::Discovering => State::Checking,
            _ => State::Missing,
        }
    }

    pub(crate) fn apply_audio_preprocessing_action(&mut self, action: AudioPreprocessingAction) {
        use AudioPreprocessingAction as Action;
        let mode = match action {
            Action::Download(mode)
            | Action::Resume(mode)
            | Action::Pause(mode)
            | Action::Delete(mode) => mode,
            Action::SetMirror(enabled) => {
                if let Err(error) = self
                    .model_task_manager
                    .switch_download_source(self.project_root(), enabled)
                {
                    self.last_error = Some(error);
                }
                return;
            }
        };
        let Some(id) = preprocessing_asset(mode) else {
            return;
        };
        let result = match action {
            Action::Download(_) | Action::Resume(_) => self
                .model_task_manager
                .enqueue_many(self.project_root(), [id]),
            Action::Pause(_) => self.model_task_manager.pause(id),
            Action::Delete(_) => {
                self.request_model_resource_deletion(id);
                return;
            }
            Action::SetMirror(_) => unreachable!(),
        };
        if let Err(error) = result {
            self.last_error = Some(error);
        }
    }

    pub(crate) fn audio_preprocessing_ready(&self, selection: Option<AudioPreprocessing>) -> bool {
        preprocessing_asset(selection.unwrap_or(self.service_config.default_audio_preprocessing()))
            .is_none_or(|id| self.model_task_manager.is_model_ready(id))
    }

    /// A pending optional download must not interrupt an existing live session.
    /// The saved selection is applied when the model manager reports readiness.
    pub(super) fn effective_audio_preprocessing(
        &self,
        selection: Option<AudioPreprocessing>,
    ) -> Option<AudioPreprocessing> {
        Some(if self.audio_preprocessing_ready(selection) {
            selection.unwrap_or(self.service_config.default_audio_preprocessing())
        } else {
            AudioPreprocessing::None
        })
    }

    pub(super) fn forget_audio_preprocessing_resource(&mut self, id: ModelAssetId) {
        let default = self.service_config.default_audio_preprocessing();
        for source in [CaptureSource::Microphone, CaptureSource::SystemAudio] {
            let recognition = self.recognition_settings_mut(source);
            if preprocessing_asset(recognition.preprocessing.unwrap_or(default)) == Some(id) {
                recognition.preprocessing = Some(AudioPreprocessing::None);
            }
        }
        let controller = &mut self.media_plugin.controller;
        if let Some(task) = controller
            .active_task_id
            .as_deref()
            .and_then(|id| controller.store.get_mut(id))
            && preprocessing_asset(task.recognition.preprocessing.unwrap_or(default)) == Some(id)
        {
            task.recognition.preprocessing = Some(AudioPreprocessing::None);
            controller.save_active();
        }
        self.save_settings();
    }
}
