use xr_corpus_protocol::SegmentContext as CorpusSegmentContext;
use xrtranslate_engine::TranslationSegmentPair;
use xrtranslate_prompt::{SurroundingSource, TranslationPromptContext, estimated_tokens};

/// Combines task-owned facts and retrieved terms within one request budget.
/// Segment positions identify neighboring source, including repeated sentences.
pub(crate) fn prompt_context_for_segment(
    source_language: &str,
    target_language: &str,
    corpus: &CorpusSegmentContext,
    reference: &TranslationPromptContext,
    segments: &[TranslationSegmentPair],
    index: usize,
    token_budget: usize,
) -> TranslationPromptContext {
    let current_tokens = segments
        .get(index)
        .map_or(0, |segment| estimated_tokens(&segment.translation_text));
    let token_budget = token_budget.saturating_sub(current_tokens);
    let mut context = TranslationPromptContext {
        current_speaker_id: reference.current_speaker_id.clone(),
        language_order: vec![source_language.to_owned(), target_language.to_owned()],
        mode: reference.mode,
        ..TranslationPromptContext::default()
    };
    if !within_budget(&context, token_budget) {
        return context.without_reference_context();
    }

    if let Some(turn) = reference.previous_revision.as_ref() {
        context.previous_revision = Some(turn.clone());
        if !within_budget(&context, token_budget) {
            context.previous_revision = None;
        }
    }

    // The current input and reserved output take priority. Reference text uses
    // the remaining tokens, without a second fixed character ceiling.
    let mut low = 0;
    let mut high = token_budget.saturating_mul(3);
    while low < high {
        let size = (low + high).div_ceil(2);
        context.surrounding_source =
            surrounding_source(source_language, reference, segments, index, size);
        if within_budget(&context, token_budget) {
            low = size;
        } else {
            high = size - 1;
        }
    }
    context.surrounding_source =
        surrounding_source(source_language, reference, segments, index, low);

    for term in &corpus.prompt_terms {
        let row = render_term_row(term);
        if row.is_empty() {
            continue;
        }
        context.terminology_rows.push(row);
        if !within_budget(&context, token_budget) {
            context.terminology_rows.pop();
        }
    }
    for turn in reference.recent_turns.iter().rev() {
        context.recent_turns.push(turn.clone());
        if !within_budget(&context, token_budget) {
            context.recent_turns.pop();
        }
    }
    context.recent_turns.reverse();
    context
}

fn surrounding_source(
    source_language: &str,
    reference: &TranslationPromptContext,
    segments: &[TranslationSegmentPair],
    index: usize,
    max_chars: usize,
) -> Option<SurroundingSource> {
    if max_chars == 0 {
        return None;
    }
    let external = reference.surrounding_source.as_ref();
    let previous = index.checked_sub(1).and_then(|index| segments.get(index));
    let next = segments.get(index.saturating_add(1));
    let before = join_bounded(
        external.map_or("", |source| source.before.as_str()),
        previous.map_or("", |segment| segment.source_text.as_str()),
        max_chars,
        true,
    );
    let after = join_bounded(
        next.map_or("", |segment| segment.source_text.as_str()),
        external.map_or("", |source| source.after.as_str()),
        max_chars,
        false,
    );
    if before.is_empty() && after.is_empty() {
        return None;
    }
    let before_limit = if after.is_empty() {
        max_chars
    } else {
        max_chars / 2
    };
    let after_limit = if before.is_empty() {
        max_chars
    } else {
        max_chars - before_limit
    };
    Some(SurroundingSource {
        speaker_id: external.map_or_else(
            || reference.current_speaker_id.clone(),
            |source| source.speaker_id.clone(),
        ),
        source_language: external.map_or_else(
            || source_language.to_owned(),
            |source| source.source_language.clone(),
        ),
        before: excerpt(&before, before_limit, true),
        after: excerpt(&after, after_limit, false),
    })
}

fn join_bounded(first: &str, second: &str, max_chars: usize, tail: bool) -> String {
    let first = excerpt(first, max_chars, tail);
    let second = excerpt(second, max_chars, tail);
    let joined = match (first.is_empty(), second.is_empty()) {
        (true, _) => second,
        (_, true) => first,
        _ => format!("{first}\n{second}"),
    };
    excerpt(&joined, max_chars, tail)
}

fn excerpt(text: &str, max_chars: usize, tail: bool) -> String {
    if tail {
        text.chars()
            .rev()
            .take(max_chars)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    } else {
        text.chars().take(max_chars).collect()
    }
}

fn within_budget(context: &TranslationPromptContext, token_budget: usize) -> bool {
    let Some(reference) = context.reference_text_for_quality_checks() else {
        return true;
    };
    let rendered = format!("# Translation Context\n\n{reference}");
    estimated_tokens(&rendered) <= token_budget
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

    #[test]
    fn repeated_sentences_use_their_segment_position() {
        let segments =
            ["Repeat.", "Between.", "Repeat.", "Last."].map(|text| TranslationSegmentPair {
                source_text: text.into(),
                translation_text: text.into(),
            });
        let reference = TranslationPromptContext::default();
        let source = surrounding_source("en", &reference, &segments, 2, 500).unwrap();
        assert_eq!(source.before, "Between.");
        assert_eq!(source.after, "Last.");
        assert_eq!(segments[2].translation_text, "Repeat.");
    }

    #[test]
    fn neighboring_text_is_bounded_and_keeps_nearest_edges() {
        let segments = ["x".repeat(10_000), "Current".into(), "y".repeat(10_000)].map(|text| {
            TranslationSegmentPair {
                translation_text: text.clone(),
                source_text: text,
            }
        });
        let reference = TranslationPromptContext {
            surrounding_source: Some(SurroundingSource {
                speaker_id: "Alice".into(),
                source_language: String::new(),
                before: "Earlier external text".into(),
                after: "Later external text".into(),
            }),
            ..TranslationPromptContext::default()
        };
        let source = surrounding_source("en", &reference, &segments, 1, 20).unwrap();
        assert_eq!(source.before, "x".repeat(10));
        assert_eq!(source.after, "y".repeat(10));
        assert_eq!(source.speaker_id, "Alice");
        assert!(source.source_language.is_empty());
    }

    #[test]
    fn reference_budget_never_truncates_current_input() {
        let current = "当前输入不能截断。".repeat(20);
        let segments = [TranslationSegmentPair {
            source_text: current.clone(),
            translation_text: current.clone(),
        }];
        let corpus = CorpusSegmentContext {
            corrected_text: current.clone(),
            prompt_terms: Vec::new(),
            source_corrections: Vec::new(),
            activation_matches: Vec::new(),
            context_matches: Vec::new(),
        };
        let reference = TranslationPromptContext {
            surrounding_source: Some(SurroundingSource {
                speaker_id: String::new(),
                source_language: "zh".into(),
                before: "前文。".repeat(1_000),
                after: "后文。".repeat(1_000),
            }),
            ..TranslationPromptContext::default()
        };
        let budget = estimated_tokens(&current) + 100;
        let result =
            prompt_context_for_segment("zh", "en", &corpus, &reference, &segments, 0, budget);
        assert!(result.surrounding_source.is_some());
        assert!(within_budget(&result, 100));
        assert_eq!(segments[0].translation_text, current);
    }

    #[test]
    fn recent_history_uses_the_available_window_instead_of_a_character_ceiling() {
        let current = "Translate this input.";
        let segments = [TranslationSegmentPair {
            source_text: current.into(),
            translation_text: current.into(),
        }];
        let corpus = CorpusSegmentContext {
            corrected_text: current.into(),
            prompt_terms: Vec::new(),
            source_corrections: Vec::new(),
            activation_matches: Vec::new(),
            context_matches: Vec::new(),
        };
        let reference = TranslationPromptContext {
            recent_turns: (0..20)
                .map(|index| xrtranslate_prompt::PromptTurn {
                    turn_id: Some(index.to_string()),
                    speaker_id: "text".into(),
                    source_language: "en".into(),
                    target_language: "zh".into(),
                    source_text: "Earlier complete sentence. ".repeat(25),
                    translated_text: "较早的完整句子。".repeat(25),
                })
                .collect(),
            ..Default::default()
        };
        let select = |budget| {
            prompt_context_for_segment("en", "zh", &corpus, &reference, &segments, 0, budget)
        };
        let small = select(700);
        let large = select(4000);
        assert!(!small.recent_turns.is_empty());
        assert!(large.recent_turns.len() > small.recent_turns.len());
        assert_eq!(
            large.recent_turns.last().unwrap().turn_id.as_deref(),
            Some("19")
        );
        assert!(
            large
                .reference_text_for_quality_checks()
                .unwrap()
                .chars()
                .count()
                > 1200
        );
        assert!(within_budget(&large, 4000 - estimated_tokens(current)));
        assert!(select(estimated_tokens(current)).recent_turns.is_empty());
        assert_eq!(segments[0].translation_text, current);
    }
}
