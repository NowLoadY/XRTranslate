use serde::{Deserialize, Serialize};
pub const DEFAULT_PAUSE_TOLERANCE: f32 = 0.15;
pub use xrtranslate_plugin::translation::CaptureSource;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RecognitionSettings {
    /// Missing in older settings: inherit the application's existing default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preprocessing: Option<xrtranslate_protocol::AudioPreprocessing>,
    #[serde(default = "default_background_noise")]
    pub background_noise: f32,
    #[serde(default = "default_pause_tolerance")]
    pub pause_tolerance: f32,
    #[serde(default)]
    /// Explicit pseudo-streaming opt-in; false selects the model’s native delivery.
    pub continuous_recognition: bool,
}

impl Default for RecognitionSettings {
    fn default() -> Self {
        Self {
            preprocessing: None,
            background_noise: default_background_noise(),
            pause_tolerance: default_pause_tolerance(),
            continuous_recognition: false,
        }
    }
}

pub const fn default_background_noise() -> f32 {
    0.30
}
pub const fn default_pause_tolerance() -> f32 {
    DEFAULT_PAUSE_TOLERANCE
}

pub fn pause_tolerance_to_ms(value: f32) -> u32 {
    (240.0 + value.clamp(0.0, 1.0) * 960.0).round() as u32
}
