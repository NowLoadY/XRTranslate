//! Shared preprocessing resource state and actions, independent of UI.
use crate::AudioPreprocessing;

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum AudioPreprocessingModelState {
    #[default]
    Missing,
    Checking,
    Queued,
    Downloading {
        downloaded_bytes: u64,
        total_bytes: u64,
    },
    Paused {
        downloaded_bytes: u64,
        total_bytes: u64,
    },
    Ready,
    Failed(String),
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct AudioPreprocessingModels {
    pub gtcrn: AudioPreprocessingModelState,
    pub vocal: AudioPreprocessingModelState,
    pub can_delete: bool,
    pub use_mirror: bool,
}

impl AudioPreprocessingModels {
    pub fn state(&self, mode: AudioPreprocessing) -> Option<&AudioPreprocessingModelState> {
        match mode {
            AudioPreprocessing::None => None,
            AudioPreprocessing::Gtcrn => Some(&self.gtcrn),
            AudioPreprocessing::VocalSeparation => Some(&self.vocal),
        }
    }

    pub fn is_ready(&self, mode: AudioPreprocessing) -> bool {
        self.state(mode)
            .is_none_or(|state| matches!(state, AudioPreprocessingModelState::Ready))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum AudioPreprocessingAction {
    Download(AudioPreprocessing),
    Pause(AudioPreprocessing),
    Resume(AudioPreprocessing),
    Delete(AudioPreprocessing),
    SetMirror(bool),
}
