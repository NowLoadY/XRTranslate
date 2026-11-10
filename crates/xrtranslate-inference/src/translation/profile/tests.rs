use super::*;
use xrtranslate_prompt::{PromptMode, PromptTurn, TranslationPromptContext};

fn context_error(
    profile: TranslationPromptProfile,
    text: &str,
    options: &TranslationOptions,
) -> String {
    match build_translation_messages(profile, text, options).unwrap_err() {
        InferenceError::InvalidConfiguration {
            field: "context_window_tokens",
            message,
        } => message,
        error => panic!("unexpected error: {error}"),
    }
}

#[test]
fn builtin_styles_fit_short_inputs_in_both_modes_and_delivery_paths() {
    let mut options = TranslationOptions::new("English", "Chinese");
    let presets = options
        .prompt_graph
        .translation_style
        .as_ref()
        .unwrap()
        .presets
        .clone();
    for preset in presets {
        assert!(options.prompt_graph.set_style_text(&preset.text));
        for mode in [PromptMode::Ordinary, PromptMode::PseudoStreaming] {
            options.prompt_context.mode = mode;
            for profile in [
                TranslationPromptProfile::Contextual,
                TranslationPromptProfile::ChatMessages,
            ] {
                assert!(
                    build_translation_messages(profile, "Hello.", &options).is_ok(),
                    "{} {mode:?} {profile:?}",
                    preset.name
                );
            }
        }
    }
}

#[test]
fn oversized_style_is_identified_even_with_a_short_input() {
    let mut options = TranslationOptions::new("English", "Chinese");
    let style = "Preserve the original meaning and tone. ".repeat(200);
    assert!(options.prompt_graph.set_style_text(&style));
    let message = context_error(TranslationPromptProfile::Contextual, "Hello.", &options);
    assert!(message.contains("prompt/style is too long even for a short input"));
    assert!(message.contains(&format!(
        "Active style: about {} tokens",
        estimated_tokens(&style)
    )));
    assert!(message.contains("Prompt Studio"));
    assert!(message.contains("Settings > Models > Translation Model > Context length"));
}

#[test]
fn long_input_reports_combined_input_and_output_requirements_without_blaming_the_style() {
    let options = TranslationOptions::new("Chinese", "English");
    let message = context_error(
        TranslationPromptProfile::Contextual,
        &"中".repeat(1928),
        &options,
    );
    assert!(message.contains("current input is too long"));
    assert!(message.contains("2384 input + 256 output = 2640 tokens"));
    assert!(message.contains("configured window is 2048"));
    assert!(message.contains("split or shorten the current input"));
}

#[test]
fn recommendations_follow_style_input_references_and_output_and_preserve_the_full_request() {
    let profile = TranslationPromptProfile::Contextual;
    let mut options = TranslationOptions::new("English", "Chinese");
    assert!(
        options
            .prompt_graph
            .set_style_text(&"Preserve the meaning. ".repeat(400))
    );
    let mut text = "Hello.".to_owned();
    let mut previous = 0;
    for step in 0..5 {
        match step {
            1 => {
                options
                    .prompt_graph
                    .set_style_text(&"Preserve the meaning. ".repeat(800));
            }
            2 => text = "Translate this sentence. ".repeat(200),
            3 => {
                options.prompt_context.terminology_rows =
                    vec!["reference term,参考术语".repeat(200)]
            }
            4 => options.max_tokens = 1024,
            _ => {}
        }
        let render = options
            .prompt_graph
            .render(
                profile.target(),
                &text,
                &options.source_language,
                &options.target_language,
                &options.prompt_context,
            )
            .unwrap();
        let recommendation = render.context_window_recommendation(options.max_tokens);
        assert!(recommendation.recommended_tokens > previous);
        assert!(recommendation.recommended_tokens > recommendation.required_tokens);
        previous = recommendation.recommended_tokens;
        let message = context_error(profile, &text, &options);
        assert!(message.contains(&format!(
            "Suggested context length for this full request: {} tokens",
            recommendation.recommended_tokens
        )));

        let mut expanded = options.clone();
        expanded.context_window_tokens = recommendation.recommended_tokens.try_into().unwrap();
        let messages = build_translation_messages(profile, &text, &expanded).unwrap();
        // Following the suggestion must preserve all originally selected facts,
        // not merely make the stripped-down prompt fit.
        assert_eq!(messages, serde_json::json!(render.messages));
    }
}

#[test]
fn excess_history_is_removed_before_reporting_a_window_error() {
    let mut options = TranslationOptions::new("English", "Chinese");
    options.prompt_context = TranslationPromptContext {
        recent_turns: vec![PromptTurn {
            turn_id: None,
            speaker_id: "previous speaker".into(),
            source_language: "English".into(),
            target_language: "Chinese".into(),
            source_text: "old context ".repeat(2000),
            translated_text: "旧上下文".repeat(2000),
        }],
        ..Default::default()
    };
    let messages =
        build_translation_messages(TranslationPromptProfile::Contextual, "Hello.", &options)
            .unwrap();
    let encoded = messages.to_string();
    assert!(encoded.contains("Hello."));
    assert!(!encoded.contains("old context"));
    assert_eq!(options.prompt_context.recent_turns.len(), 1);
}

#[test]
fn bilingual_models_do_not_attribute_errors_to_an_unused_custom_style() {
    let mut options = TranslationOptions::new("Chinese", "English");
    assert!(
        options
            .prompt_graph
            .set_style_text(&"Unused custom style. ".repeat(2000))
    );
    assert!(
        build_translation_messages(TranslationPromptProfile::Bilingual, "你好", &options).is_ok()
    );
    let message = context_error(
        TranslationPromptProfile::Bilingual,
        &"中".repeat(2200),
        &options,
    );
    assert!(message.contains("current input is too long"));
    assert!(!message.contains("Active style:"));
}
