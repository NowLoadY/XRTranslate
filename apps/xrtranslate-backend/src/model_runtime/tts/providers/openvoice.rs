//! OpenVoice-specific configuration mapping.

use std::path::Path;

use xrtranslate_config::AppConfig;
use xrtranslate_inference::{OnnxExecutionDevice, OpenVoiceOnnxAdapter, OpenVoiceSynthesisOptions};

use super::{provider_object, supported_languages, threads};
use crate::model_runtime::tts::NativeTtsAdapter;

pub(in crate::model_runtime::tts) fn build(
    config: &AppConfig,
    model_directory: &Path,
) -> Result<NativeTtsAdapter, String> {
    let provider = provider_object(config, "openvoice")?;
    let device = provider
        .get("device")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("auto");
    let defaults = OpenVoiceSynthesisOptions::default();
    let speed = provider
        .get("speed")
        .and_then(serde_json::Value::as_f64)
        .map_or(defaults.speed, |value| value as f32);
    let adapter = OpenVoiceOnnxAdapter::with_synthesis_options(
        model_directory,
        OnnxExecutionDevice::from_config(device),
        threads(provider),
        OpenVoiceSynthesisOptions { speed },
    )
    .map_err(|error| error.to_string())?;
    Ok(NativeTtsAdapter::openvoice(
        adapter,
        supported_languages(provider, "openvoice")?,
    ))
}
