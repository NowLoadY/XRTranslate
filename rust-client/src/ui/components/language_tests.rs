use super::*;
use xrtranslate_engine::language::{LanguageCapabilities, LanguageSet};

#[test]
fn opening_a_saved_task_never_changes_its_languages_when_models_change() {
    let ctx = egui::Context::default();
    let mut source = "ja".to_owned();
    let mut target = "ko".to_owned();
    for languages in [
        LanguageSet::ALL,
        LanguageSet::from_codes(&["zh", "en"], false),
        LanguageSet::EMPTY,
    ] {
        let mut output = ctx.run_ui(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                assert!(!translation_language_selector(
                    ui,
                    "saved_task",
                    &mut source,
                    &mut target,
                    LanguageCapabilities {
                        recognition: Some(languages),
                        translation: Some(languages)
                    },
                    crate::i18n::UiLanguage::English
                ));
            });
        });
        output.textures_delta.clear();
        assert_eq!((source.as_str(), target.as_str()), ("ja", "ko"));
    }
}

#[test]
fn rendering_automatic_modes_does_not_convert_one_into_the_other() {
    let ctx = egui::Context::default();
    for saved_target in ["zh", "ja,ko"] {
        let mut source = "auto".to_owned();
        let mut target = saved_target.to_owned();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                assert!(!translation_language_selector(
                    ui,
                    "automatic_task",
                    &mut source,
                    &mut target,
                    LanguageCapabilities::default(),
                    crate::i18n::UiLanguage::English
                ));
            });
        });
        output.textures_delta.clear();
        assert_eq!(target, saved_target);
    }
}
