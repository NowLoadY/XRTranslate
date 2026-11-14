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

#[test]
fn history_fits_each_window_without_changing_style_input_or_graph_order() {
    let mut options = TranslationOptions::new("English", "Chinese");
    let style = "Use natural wording and preserve the speaker's tone. ".repeat(8);
    assert!(options.prompt_graph.set_style_text(&style));
    options.prompt_context.recent_turns = (0..48)
        .map(|index| PromptTurn {
            turn_id: Some(index.to_string()),
            speaker_id: "speaker".into(),
            source_language: "en".into(),
            target_language: "zh".into(),
            source_text: format!("Earlier turn {index}. {}", "Complete sentence. ".repeat(6)),
            translated_text: "之前的完整对话。".repeat(6),
        })
        .collect();
    let original = options.prompt_context.recent_turns.clone();
    let text = "Where did she go?";

    for profile in [
        TranslationPromptProfile::ChatMessages,
        TranslationPromptProfile::Contextual,
    ] {
        for mode in [PromptMode::Ordinary, PromptMode::PseudoStreaming] {
            options.prompt_context.mode = mode;
            let mut previous_count = 0;
            for window in [256, 512, 1_024, 2_048, 4_700, 8_192, 32_768, 1_048_576] {
                options.context_window_tokens = window;
                let rendered = registered(profile).build_prompt(text, &options);
                let minimal = options
                    .prompt_graph
                    .render(
                        profile.target(),
                        text,
                        "English",
                        "Chinese",
                        &options.prompt_context.without_reference_context(),
                    )
                    .unwrap();
                if minimal.estimated_tokens() + options.max_tokens as usize > window as usize {
                    assert!(rendered.is_err());
                    continue;
                }
                let rendered = rendered.unwrap();
                let count = rendered.context.recent_turns.len();
                assert!(count >= previous_count);
                previous_count = count;
                assert_eq!(
                    rendered.context.recent_turns,
                    original[original.len() - count..]
                );
                let expected = options
                    .prompt_graph
                    .render(
                        profile.target(),
                        text,
                        "English",
                        "Chinese",
                        &rendered.context,
                    )
                    .unwrap();
                assert_eq!(rendered.messages, expected.messages);
                assert!(
                    expected.estimated_tokens() + options.max_tokens as usize <= window as usize
                );
                let joined = rendered
                    .messages
                    .iter()
                    .map(|message| message.content.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                assert!(joined.contains(style.trim()));
                assert!(joined.contains(text));
                assert_eq!(options.prompt_context.recent_turns, original);
            }
            assert_eq!(previous_count, original.len());
        }
    }
}

#[test]
fn translated_prompt_expansion_is_rejected_even_without_history() {
    let output = "的真实含义，然后结合背景确定指代对象、语气、说话关系及意图。保持原意不变，不要随意添加、删除或改变原意。仅翻译当前英文输入，不解释、不重复、不总结。除非明确说明，否则只输出最终中文翻译。\n\n--- 参考背景结束 ---\n\n当前输入：毫光";
    assert!(is_probable_translation_context_leak("毫光", output, None));
    assert!(
        output::translation_output_rejection(
            "毫光",
            output,
            &[],
            &TranslationPromptContext::default(),
        )
        .is_some()
    );
}

#[test]
fn length_gate_allows_normal_cross_language_and_style_expansion() {
    for (source, translated) in [
        ("毫光", "A faint glimmer of light."),
        ("辛苦了", "Thank you so much for all your hard work today!"),
        ("お疲れ様です", "Thank you for your hard work."),
        ("Would you mind giving me a hand?", "能麻烦你帮我一下吗？"),
        (
            "不见不散",
            "I will see you there, and I will wait for you until you arrive.",
        ),
    ] {
        assert!(
            output::translation_output_rejection(
                source,
                translated,
                &[],
                &TranslationPromptContext::default(),
            )
            .is_none(),
            "{source} -> {translated}"
        );
    }
    // Non-ASCII scripts weigh more in the shared estimate. Long translations
    // must still fit without depending on the allowance for short phrases.
    assert!(!is_probable_translation_context_leak(
        &"Let's catch up tomorrow. ".repeat(20),
        &"Давай завтра встретимся и обсудим последние новости. ".repeat(20),
        None,
    ));
}
