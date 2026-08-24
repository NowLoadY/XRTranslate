use xr_corpus_protocol::SegmentContext as CorpusSegmentContext;
use xrtranslate_prompt::{PromptTurn, SurroundingSource, TranslationPromptContext};

/// Builds the built-in prompt from neutral context facts. XR Corpus remains a
/// data provider; template ownership stays in the shared translation layer.
pub(crate) fn prompt_context_for_segment(
    source_language: &str,
    target_language: &str,
    context: &CorpusSegmentContext,
) -> TranslationPromptContext {
    TranslationPromptContext {
        language_order: vec![source_language.to_owned(), target_language.to_owned()],
        terminology_rows: context.prompt_terms.iter().map(render_term_row).collect(),
        recent_turns: context
            .context_data
            .recent_turns
            .iter()
            .map(|turn| PromptTurn {
                turn_id: turn.turn_id.clone(),
                speaker_id: turn.speaker_id.clone(),
                source_language: turn.source_language.clone(),
                target_language: turn.target_language.clone(),
                source_text: turn.source_text.clone(),
                translated_text: turn.translated_text.clone(),
            })
            .collect(),
        previous_revision: context
            .context_data
            .previous_revision
            .as_ref()
            .map(|turn| PromptTurn {
                turn_id: turn.turn_id.clone(),
                speaker_id: turn.speaker_id.clone(),
                source_language: turn.source_language.clone(),
                target_language: turn.target_language.clone(),
                source_text: turn.source_text.clone(),
                translated_text: turn.translated_text.clone(),
            }),
        surrounding_source: context
            .context_data
            .surrounding_source
            .as_ref()
            .map(|source| SurroundingSource {
                speaker_id: source.speaker_id.clone(),
                source_language: source.source_language.clone(),
                before: source.before.clone(),
                after: source.after.clone(),
            }),
        mode: xrtranslate_prompt::PromptMode::Ordinary,
    }
}

fn render_term_row(term: &xr_corpus_protocol::CorpusPromptTerm) -> String {
    term.values
        .iter()
        .map(|(_, value)| value.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;
    use xr_corpus_protocol::{
        BilingualContextTurn, CorpusPromptTerm, SurroundingSourceContext, TranslationContextData,
    };
    use xrtranslate_prompt::{PromptNodeGraph, PromptProviderTarget};

    fn context() -> CorpusSegmentContext {
        CorpusSegmentContext {
            corrected_text: "Tell the team.".into(),
            prompt_terms: vec![CorpusPromptTerm {
                values: vec![("zh".into(), "天使".into()), ("en".into(), "Mercy".into())],
                sources: Vec::new(),
            }],
            context_data: TranslationContextData {
                recent_turns: vec![BilingualContextTurn {
                    turn_id: Some("previous".into()),
                    speaker_id: "speaker-01".into(),
                    source_language: "en".into(),
                    target_language: "zh".into(),
                    source_text: "We changed the plan.".into(),
                    translated_text: "我们改计划了。".into(),
                }],
                previous_revision: None,
                surrounding_source: Some(SurroundingSourceContext {
                    speaker_id: "speaker-01".into(),
                    source_language: "en".into(),
                    before: "Before it.".into(),
                    after: "After it.".into(),
                }),
            },
            source_corrections: Vec::new(),
            activation_matches: Vec::new(),
            context_matches: Vec::new(),
        }
    }

    #[test]
    fn structured_context_uses_the_default_template_instead_of_legacy_prompt() {
        let prompt_context = prompt_context_for_segment("en", "zh", &context());
        let prompt = PromptNodeGraph::builtin_default()
            .render(
                PromptProviderTarget::Hunyuan,
                "Tell the team.",
                "English",
                "Chinese",
                &prompt_context,
            )
            .unwrap();
        assert!(
            prompt.messages[0]
                .content
                .contains("## Recent Bilingual History")
        );
        assert!(prompt.messages[0].content.contains("天使,Mercy"));
    }
}
