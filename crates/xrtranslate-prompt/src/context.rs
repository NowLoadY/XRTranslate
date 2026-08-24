use serde::{Deserialize, Serialize};

use crate::PromptMode;

/// Provider-neutral facts available before speech recognition. The prompt
/// graph may render these terms into a free-form instruction prompt or a
/// lexical context field, while weighted vocabulary delivery stays separate.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AsrPromptContext {
    pub vocabulary: Vec<String>,
    pub mode: PromptMode,
}

impl AsrPromptContext {
    pub fn has_recognition_context(&self) -> bool {
        self.vocabulary.iter().any(|term| !term.trim().is_empty())
    }

    pub fn recognition_context_text(&self) -> String {
        self.vocabulary
            .iter()
            .map(|term| term.trim())
            .filter(|term| !term.is_empty())
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn without_recognition_context(&self) -> Self {
        Self {
            vocabulary: Vec::new(),
            mode: self.mode,
        }
    }

    /// Selects complete recognition terms whose rendered comma-separated text
    /// fits a provider-declared character limit. Weighted vocabulary delivery
    /// continues to use the original, unbounded structured terms.
    pub fn bounded_recognition_context(&self, max_chars: usize) -> Self {
        let mut vocabulary = Vec::new();
        let mut used_chars = 0usize;
        for term in self
            .vocabulary
            .iter()
            .map(|term| term.trim())
            .filter(|term| !term.is_empty())
        {
            let separator_chars = usize::from(!vocabulary.is_empty()) * 2;
            let term_chars = term.chars().count();
            if used_chars + separator_chars + term_chars > max_chars {
                continue;
            }
            used_chars += separator_chars + term_chars;
            vocabulary.push(term.to_owned());
        }
        Self {
            vocabulary,
            mode: self.mode,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TranslationPromptContext {
    pub language_order: Vec<String>,
    pub terminology_rows: Vec<String>,
    pub recent_turns: Vec<PromptTurn>,
    pub previous_revision: Option<PromptTurn>,
    pub surrounding_source: Option<SurroundingSource>,
    pub mode: PromptMode,
}

impl TranslationPromptContext {
    pub fn has_reference_context(&self) -> bool {
        TranslationPromptBlock::builtin_reference_blocks()
            .iter()
            .any(|block| render_block(block, self).is_some())
    }

    pub fn without_reference_context(&self) -> Self {
        Self {
            mode: self.mode,
            ..Self::default()
        }
    }

    pub fn reference_text_for_quality_checks(&self) -> Option<String> {
        let values = self.reference_blocks_for_quality_checks();
        (!values.is_empty()).then(|| values.join("\n\n"))
    }

    /// Renders each runtime reference block independently so downstream
    /// quality checks can exclude dynamic values regardless of graph order or
    /// separators without reconstructing prompt composition.
    pub fn reference_blocks_for_quality_checks(&self) -> Vec<String> {
        TranslationPromptBlock::builtin_reference_blocks()
            .iter()
            .filter_map(|block| render_block(block, self))
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptTurn {
    pub turn_id: Option<String>,
    pub speaker_id: String,
    pub source_language: String,
    pub target_language: String,
    pub source_text: String,
    pub translated_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurroundingSource {
    pub speaker_id: String,
    pub source_language: String,
    pub before: String,
    pub after: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TranslationPromptBlock {
    LanguageOrder,
    Terminology,
    RecentTurns { limit: Option<usize> },
    PreviousRevision,
    SurroundingSource,
    CustomText { text: String },
}

impl TranslationPromptBlock {
    pub fn builtin_reference_blocks() -> [Self; 5] {
        [
            Self::LanguageOrder,
            Self::Terminology,
            Self::RecentTurns { limit: None },
            Self::PreviousRevision,
            Self::SurroundingSource,
        ]
    }

    pub fn preview_name(&self) -> &'static str {
        match self {
            Self::LanguageOrder => "LANGUAGE ORDER",
            Self::Terminology => "TERMINOLOGY",
            Self::RecentTurns { .. } => "RECENT TURNS",
            Self::PreviousRevision => "PREVIOUS REVISION",
            Self::SurroundingSource => "SURROUNDING SOURCE",
            Self::CustomText { .. } => "CUSTOM TEXT",
        }
    }
}

pub(crate) fn render_block(
    block: &TranslationPromptBlock,
    context: &TranslationPromptContext,
) -> Option<String> {
    match block {
        TranslationPromptBlock::LanguageOrder => {
            let languages = context
                .language_order
                .iter()
                .map(|language| language.trim())
                .filter(|language| !language.is_empty())
                .collect::<Vec<_>>();
            (!languages.is_empty()).then(|| format!("## Language Order\n\n{}", languages.join(",")))
        }
        TranslationPromptBlock::Terminology => {
            let rows = context
                .terminology_rows
                .iter()
                .map(|row| row.trim())
                .filter(|row| !row.is_empty())
                .collect::<Vec<_>>();
            (!rows.is_empty()).then(|| format!("## Terminology\n\n{}", rows.join("\n")))
        }
        TranslationPromptBlock::RecentTurns { limit } => {
            let start = limit
                .map(|limit| context.recent_turns.len().saturating_sub(limit))
                .unwrap_or_default();
            let turns = context
                .recent_turns
                .iter()
                .skip(start)
                .map(render_turn)
                .collect::<Vec<_>>();
            (!turns.is_empty())
                .then(|| format!("## Recent Bilingual History\n\n{}", turns.join("\n\n")))
        }
        TranslationPromptBlock::PreviousRevision => {
            context.previous_revision.as_ref().map(|turn| {
                format!(
                    "## Previous Revision of Current Speech\n\n{}",
                    render_turn(turn)
                )
            })
        }
        TranslationPromptBlock::SurroundingSource => {
            let source = context.surrounding_source.as_ref()?;
            let mut lines = Vec::new();
            append_source_line(&mut lines, source, "Before current input", &source.before);
            append_source_line(&mut lines, source, "After current input", &source.after);
            (!lines.is_empty()).then(|| {
                format!(
                    "## Current Utterance Context (context only; do not translate)\n\n{}",
                    lines.join("\n")
                )
            })
        }
        TranslationPromptBlock::CustomText { text } => {
            let text = text.trim();
            (!text.is_empty()).then(|| format!("## Custom Reference Text\n\n{text}"))
        }
    }
}

fn render_turn(turn: &PromptTurn) -> String {
    let speaker = if turn.speaker_id.trim().is_empty() {
        String::new()
    } else {
        format!("{} ", turn.speaker_id.trim())
    };
    format!(
        "{speaker}{}: {}\n{speaker}{}: {}",
        turn.source_language.trim(),
        turn.source_text.trim(),
        turn.target_language.trim(),
        turn.translated_text.trim()
    )
}

fn append_source_line(
    lines: &mut Vec<String>,
    source: &SurroundingSource,
    label: &str,
    text: &str,
) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    let speaker = if source.speaker_id.trim().is_empty() {
        String::new()
    } else {
        format!("{} ", source.speaker_id.trim())
    };
    lines.push(format!(
        "{label}: {speaker}{} / {text}",
        source.source_language.trim()
    ));
}

#[cfg(test)]
mod tests {
    use super::AsrPromptContext;
    use crate::PromptMode;

    #[test]
    fn recognition_context_bounds_complete_terms_and_exact_separator_cost() {
        let context = AsrPromptContext {
            vocabulary: vec!["Alpha".into(), "Beta".into(), "TooLongForGap".into()],
            mode: PromptMode::Ordinary,
        };

        let bounded = context.bounded_recognition_context(11);

        assert_eq!(bounded.vocabulary, vec!["Alpha", "Beta"]);
        assert_eq!(bounded.recognition_context_text(), "Alpha, Beta");
        assert_eq!(context.vocabulary.len(), 3);
    }
}
