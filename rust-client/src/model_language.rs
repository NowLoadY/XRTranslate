//! Translation route choices derived from the selected local model cards.

use xrtranslate_assets::{ModelAssetManifest, ModelCapability, manifest_for};

use crate::{LANGUAGE_OPTIONS, service_config::ServiceConfigEditor};

fn selected_model(
    config: &ServiceConfigEditor,
    capability: ModelCapability,
) -> Option<&'static ModelAssetManifest> {
    config
        .selected_model_asset_ids()
        .into_iter()
        .map(manifest_for)
        .find(|model| model.capability == capability)
}

pub(crate) fn supports(model: Option<&ModelAssetManifest>, code: &str) -> bool {
    let Some(model) = model else {
        return true;
    };
    let code = if code == "zh-TW" { "zh-Hant" } else { code };
    model.languages.contains(&code)
        || (model.capability == ModelCapability::Asr
            && code == "zh-Hant"
            && model.languages.contains(&"zh"))
}

pub(crate) fn source_options(config: &ServiceConfigEditor) -> Vec<(&'static str, &'static str)> {
    let asr = selected_model(config, ModelCapability::Asr);
    let translation = selected_model(config, ModelCapability::Translation);
    LANGUAGE_OPTIONS
        .iter()
        .copied()
        .filter(|(code, _)| supports(asr, code) && supports(translation, code))
        .collect()
}

pub(crate) fn target_options(config: &ServiceConfigEditor) -> Vec<(&'static str, &'static str)> {
    let translation = selected_model(config, ModelCapability::Translation);
    LANGUAGE_OPTIONS
        .iter()
        .copied()
        .filter(|(code, _)| supports(translation, code))
        .collect()
}

pub(crate) fn normalize_route(
    config: &ServiceConfigEditor,
    source: &mut String,
    target: &mut String,
) {
    let sources = source_options(config);
    let targets = target_options(config);
    normalize_route_for_options(&sources, &targets, source, target);
}

pub(crate) fn normalize_route_for_options(
    sources: &[(&str, &str)],
    targets: &[(&str, &str)],
    source: &mut String,
    target: &mut String,
) {
    let contains =
        |options: &[(&str, &str)], code: &str| options.iter().any(|(value, _)| *value == code);
    if *source == "auto" {
        let pair = target.split_once(',').filter(|(a, b)| {
            contains(&sources, a) && contains(&sources, b) && !crate::languages_conflict(a, b)
        });
        if pair.is_none() {
            let first = sources
                .iter()
                .find(|(code, _)| *code == "zh")
                .or_else(|| sources.first())
                .map(|item| item.0)
                .unwrap_or("zh");
            let second = sources
                .iter()
                .find(|(code, _)| *code == "en" && !crate::languages_conflict(code, first))
                .or_else(|| {
                    sources
                        .iter()
                        .find(|(code, _)| !crate::languages_conflict(code, first))
                })
                .map(|item| item.0)
                .unwrap_or("en");
            *target = format!("{first},{second}");
        }
    } else {
        if !contains(&sources, source) {
            *source = sources.first().map(|item| item.0).unwrap_or("zh").into();
        }
        if !contains(&targets, target) || crate::languages_conflict(source, target) {
            *target = targets
                .iter()
                .find(|(code, _)| !crate::languages_conflict(code, source))
                .map(|item| item.0)
                .unwrap_or("en")
                .into();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xrtranslate_assets::{ModelAssetId, manifest_for};

    #[test]
    fn bilingual_model_rejects_unsupported_routes() {
        let haidass = manifest_for(ModelAssetId::HaidassTranslate143mQ8Gguf);
        assert!(supports(Some(haidass), "zh"));
        assert!(supports(Some(haidass), "en"));
        assert!(!supports(Some(haidass), "ja"));
        assert!(!supports(Some(haidass), "zh-Hant"));
    }

    #[test]
    fn sensevoice_declares_five_spoken_languages() {
        let sensevoice = manifest_for(ModelAssetId::SenseVoiceSmallInt8Onnx);
        assert_eq!(sensevoice.languages, &["zh", "en", "ja", "ko", "yue"]);
    }

    #[test]
    fn route_normalization_stays_within_selected_language_sets() {
        let sources = [("zh", "Chinese"), ("en", "English")];
        let targets = sources;
        let mut source = "ja".to_owned();
        let mut target = "ko".to_owned();
        normalize_route_for_options(&sources, &targets, &mut source, &mut target);
        assert_eq!((source.as_str(), target.as_str()), ("zh", "en"));

        source = "auto".into();
        target = "ja,ko".into();
        normalize_route_for_options(&sources, &targets, &mut source, &mut target);
        assert_eq!(target, "zh,en");
    }
}
