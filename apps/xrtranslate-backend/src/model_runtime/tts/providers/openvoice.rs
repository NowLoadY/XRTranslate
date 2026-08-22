//! OpenVoice-specific configuration mapping.

use std::path::Path;

use xrtranslate_assets::ModelAssetId;
use xrtranslate_config::AppConfig;
use xrtranslate_inference::{
    OnnxExecutionDevice, OpenVoiceBaseVoice, OpenVoiceOnnxAdapter, OpenVoiceSynthesisOptions,
};

use super::{provider_object, threads};
use crate::model_runtime::tts::NativeTtsAdapter;

pub(in crate::model_runtime::tts) fn build(
    config: &AppConfig,
    model_asset: ModelAssetId,
    model_directory: &Path,
    supported_languages: Vec<String>,
) -> Result<NativeTtsAdapter, String> {
    let provider = provider_object(config, "openvoice")?;
    let configured_device = provider
        .get("device")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("auto");
    if !matches!(configured_device, "auto" | "cuda") {
        return Err("OpenVoice managed models require CUDA; CPU is not supported.".to_owned());
    }
    let defaults = OpenVoiceSynthesisOptions::default();
    let speed = provider
        .get("speed")
        .and_then(serde_json::Value::as_f64)
        .map_or(defaults.speed, |value| value as f32);
    let configured_voice = provider
        .get("voices")
        .and_then(serde_json::Value::as_object)
        .and_then(|voices| voices.get("en"))
        .and_then(serde_json::Value::as_str);
    let base_voice = base_voice_for(model_asset, configured_voice)?;
    let adapter = OpenVoiceOnnxAdapter::with_synthesis_options(
        model_directory,
        OnnxExecutionDevice::Cuda,
        threads(provider),
        OpenVoiceSynthesisOptions { speed, base_voice },
    )
    .map_err(|error| error.to_string())?;
    Ok(NativeTtsAdapter::openvoice(adapter, supported_languages))
}

fn base_voice_for(
    model_asset: ModelAssetId,
    configured_voice: Option<&str>,
) -> Result<OpenVoiceBaseVoice, String> {
    let base_voice = match model_asset {
        ModelAssetId::OpenVoiceV3OnnxFp16 => OpenVoiceBaseVoice::EnglishNewest,
        ModelAssetId::OpenVoiceV2OnnxFp16 => match configured_voice.unwrap_or("en-us") {
            "en-us" => OpenVoiceBaseVoice::EnglishAmerican,
            "en-british" => OpenVoiceBaseVoice::EnglishBritish,
            "en-india" => OpenVoiceBaseVoice::EnglishIndian,
            "en-au" => OpenVoiceBaseVoice::EnglishAustralian,
            "en-default" => OpenVoiceBaseVoice::EnglishDefault,
            value => {
                return Err(format!(
                    "Unknown OpenVoice English base voice {value:?} for {}.",
                    model_asset.as_str()
                ));
            }
        },
        _ => {
            return Err(format!(
                "Model package {} is not an OpenVoice asset.",
                model_asset.as_str()
            ));
        }
    };
    Ok(base_voice)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v2_voice_keys_select_the_matching_base_speaker() {
        assert_eq!(
            base_voice_for(ModelAssetId::OpenVoiceV2OnnxFp16, Some("en-british")).unwrap(),
            OpenVoiceBaseVoice::EnglishBritish
        );
        assert_eq!(
            base_voice_for(ModelAssetId::OpenVoiceV2OnnxFp16, Some("en-default")).unwrap(),
            OpenVoiceBaseVoice::EnglishDefault
        );
    }

    #[test]
    fn v3_always_uses_its_only_packaged_voice() {
        assert_eq!(
            base_voice_for(ModelAssetId::OpenVoiceV3OnnxFp16, Some("en-us")).unwrap(),
            OpenVoiceBaseVoice::EnglishNewest
        );
    }
}
