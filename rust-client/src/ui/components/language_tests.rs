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

#[test]
fn fixed_extra_language_choices_respect_models_and_the_primary_route() {
    let capabilities = LanguageCapabilities {
        recognition: Some(LanguageSet::from_codes(&["ja", "ko"], false)),
        translation: Some(LanguageSet::from_codes(&["ja", "ko", "fr"], false)),
    };
    for (source, target) in [("auto", "ja,ko"), ("ja", "ko")] {
        assert_eq!(
            additional_language_options(source, target, capabilities).options(),
            vec![("fr", "French")]
        );
    }
    assert!(
        additional_language_options(
            "ja",
            "ko",
            LanguageCapabilities {
                translation: Some(LanguageSet::from_codes(&["ja", "ko"], false)),
                ..capabilities
            }
        )
        .is_empty()
    );
}

#[test]
fn primary_mode_changes_keep_the_fixed_extra_language_available() {
    let capabilities = LanguageCapabilities {
        recognition: Some(LanguageSet::from_codes(&["ja", "ko", "fr"], false)),
        translation: Some(LanguageSet::from_codes(&["ja", "ko", "fr"], false)),
    };
    let primary = primary_language_capabilities(capabilities, Some("fr"));
    let french = xrtranslate_engine::language::SupportedLanguage::parse("fr").unwrap();
    assert!(!primary.sources().contains(french));
    assert!(!primary.targets().contains(french));
    for (source, previous_target, bidirectional) in [
        ("auto", "fr", false),
        ("auto", "fr,ko", true),
        ("ko", "fr", false),
    ] {
        let (source, target) = primary
            .change_input(source, previous_target, bidirectional)
            .unwrap()
            .wire();
        assert!(
            capabilities
                .select_with_options(&source, &target, Some("fr"), false)
                .is_ok()
        );
    }
    // An automatically detected input can still match this output at runtime.
    assert!(
        capabilities
            .select_with_options("fr", "ja", Some("fr"), false)
            .is_ok()
    );
}

#[test]
fn rendering_extended_options_preserves_saved_intent_when_models_change() {
    let ctx = egui::Context::default();
    for asr_mode in [false, true] {
        let mut source = "ja".to_owned();
        let mut target = "ko".to_owned();
        let mut additional = Some("fr".to_owned());
        let mut asr_only = asr_mode;
        for translation in [LanguageSet::ALL, LanguageSet::EMPTY] {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    assert!(!translation_language_selector_with_options(
                        ui,
                        "extended_task",
                        &mut source,
                        &mut target,
                        &mut additional,
                        &mut asr_only,
                        LanguageCapabilities {
                            recognition: Some(LanguageSet::from_codes(&["ja"], false)),
                            translation: Some(translation),
                        },
                        crate::i18n::UiLanguage::English,
                    ));
                });
            });
            output.textures_delta.clear();
            assert_eq!((source.as_str(), target.as_str()), ("ja", "ko"));
            assert_eq!(additional.as_deref(), Some("fr"));
            assert_eq!(asr_only, asr_mode);
        }
    }
}

#[test]
fn primary_controls_support_asr_and_hide_the_fixed_extra_output_setting() {
    let ctx = egui::Context::default();
    for asr_mode in [false, true] {
        let mut source = "ja".to_owned();
        let mut target = "ko".to_owned();
        let mut asr_only = asr_mode;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                assert!(!translation_primary_language_selector(
                    ui,
                    "primary_controls",
                    &mut source,
                    &mut target,
                    Some("fr"),
                    &mut asr_only,
                    LanguageCapabilities::default(),
                    crate::i18n::UiLanguage::English,
                ));
            });
        });
        output.textures_delta.clear();
        assert_eq!((source.as_str(), target.as_str()), ("ja", "ko"));
        assert_eq!(asr_only, asr_mode);
        let labels = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.text()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            !labels
                .iter()
                .any(|label| label.contains("Fixed extra language"))
        );
        assert_eq!(
            labels
                .iter()
                .any(|label| label.contains("Input only (ASR)")),
            asr_mode
        );
    }
}
