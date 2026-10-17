use xrtranslate_prompt::{
    ContextFacts, PromptNodeGraph, PromptNodeKind, PromptNodePage, PromptSystemValue, PromptTarget,
    PromptTemplateLibrary, PromptTurn, TaskContext, TranslationPromptContext,
};

#[test]
fn task_context_defaults_and_round_trips() {
    let empty: TaskContext = serde_json::from_str(r#"{"kind":"snapshot"}"#).unwrap();
    assert_eq!(empty, TaskContext::default());
    let context = TaskContext::Conversation {
        scope: "meeting-1".into(),
        seed: ContextFacts {
            recent_turns: vec![previous_turn()],
            ..ContextFacts::default()
        },
    };
    let encoded = serde_json::to_string(&context).unwrap();
    assert_eq!(
        serde_json::from_str::<TaskContext>(&encoded).unwrap(),
        context
    );
}

#[test]
fn delivery_paths_use_one_shared_context() {
    let graph = PromptNodeGraph::builtin_default();
    graph.validate_for_activation().unwrap();
    let history_nodes = graph
        .nodes
        .iter()
        .filter(|node| {
            matches!(
                node.kind,
                PromptNodeKind::SystemValue {
                    value: PromptSystemValue::RecentTurns { .. }
                }
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(history_nodes.len(), 1);
    assert_eq!(history_nodes[0].page, PromptNodePage::Shared);
    let context = TranslationPromptContext {
        recent_turns: vec![previous_turn()],
        ..TranslationPromptContext::default()
    };
    for target in [
        PromptTarget::ChatMessages,
        PromptTarget::TranslationInstruction,
    ] {
        let execution = graph
            .render_with_trace(target, "Where is it?", "English", "Japanese", &context)
            .unwrap();
        let reference = &execution.trace.node("reference-context").unwrap().output;
        assert!(reference.contains("The entrance is on the right."));
        assert!(reference.contains("入口は右側です。"));
        assert_eq!(reference.matches("## Recent Bilingual History").count(), 1);
    }
}

#[test]
fn normalization_updates_only_known_speech_instruction() {
    let mut library = PromptTemplateLibrary::default();
    let mut profile = PromptTemplateLibrary::editable_copy_of(&library.profiles[0], "custom");
    let old = "You are a real-time speech translator. If input is already {0}, output it unchanged. Otherwise translate it into {0}. Output only the translation.";
    let authored = "Translate this document and preserve all section titles.";
    for (id, value) in [
        ("openai-explicit-instruction-ordinary", old),
        ("openai-auto-instruction-ordinary", authored),
    ] {
        let node = profile
            .graph
            .nodes
            .iter_mut()
            .find(|node| node.id == id)
            .unwrap();
        node.kind = PromptNodeKind::Compose { text: value.into() };
    }
    library.profiles.push(profile);
    library.normalize();
    let graph = &library
        .profiles
        .iter()
        .find(|profile| profile.id == "custom")
        .unwrap()
        .graph;
    assert_eq!(
        graph
            .static_text("openai-auto-instruction-ordinary")
            .as_deref(),
        Some(authored)
    );
    let node = graph
        .nodes
        .iter()
        .find(|node| node.id == "openai-explicit-instruction-ordinary")
        .unwrap();
    assert_eq!(
        node.kind,
        PromptNodeKind::Compose {
            text: old.replace("real-time speech translator", "translator")
        }
    );
}

fn previous_turn() -> PromptTurn {
    PromptTurn {
        turn_id: Some("1".into()),
        speaker_id: "speaker-1".into(),
        source_language: "English".into(),
        target_language: "Japanese".into(),
        source_text: "The entrance is on the right.".into(),
        translated_text: "入口は右側です。".into(),
    }
}
