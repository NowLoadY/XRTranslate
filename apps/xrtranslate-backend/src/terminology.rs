use xr_corpus_protocol::{CorpusPromptTerm, CorpusRecognitionCorrection};
use xrtranslate_protocol::CorpusTermMatch;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TerminologyRewrite {
    pub(crate) translated_text: String,
    pub(crate) term_matches: Vec<CorpusTermMatch>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RecognitionRewrite {
    pub(crate) corrected_text: String,
    pub(crate) term_matches: Vec<CorpusTermMatch>,
}

pub(crate) fn rewrite_recognition_terms(
    source_text: &str,
    corrections: &[CorpusRecognitionCorrection],
) -> RecognitionRewrite {
    if source_text.is_empty() || corrections.is_empty() {
        return RecognitionRewrite {
            corrected_text: source_text.to_owned(),
            term_matches: Vec::new(),
        };
    }

    let mut corrections = corrections
        .iter()
        .filter_map(|correction| {
            let start = usize::try_from(correction.start_byte).ok()?;
            let end = usize::try_from(correction.end_byte).ok()?;
            (start < end
                && end <= source_text.len()
                // Corpus offsets are byte offsets, but Rust string slices must
                // start and end on UTF-8 character boundaries. Treat malformed
                // ranges as unusable data instead of panicking the worker.
                && source_text.is_char_boundary(start)
                && source_text.is_char_boundary(end)
                && !correction.corrected_text.trim().is_empty())
            .then_some((start, end, correction))
        })
        .collect::<Vec<_>>();
    corrections.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| right.1.cmp(&left.1)));

    let mut selected: Vec<(usize, usize, &CorpusRecognitionCorrection)> = Vec::new();
    for correction in corrections {
        if selected
            .iter()
            .any(|existing| correction.0 < existing.1 && correction.1 > existing.0)
        {
            continue;
        }
        selected.push(correction);
    }

    let mut corrected_text = String::with_capacity(source_text.len());
    let mut term_matches = Vec::new();
    let mut cursor = 0usize;
    for (start, end, correction) in selected {
        if cursor < start {
            corrected_text.push_str(&source_text[cursor..start]);
        }
        let match_start = corrected_text.len();
        corrected_text.push_str(&correction.corrected_text);
        let match_end = corrected_text.len();
        if let (Ok(start_byte), Ok(end_byte)) =
            (u32::try_from(match_start), u32::try_from(match_end))
        {
            term_matches.push(CorpusTermMatch {
                start_byte,
                end_byte,
                text: correction.corrected_text.clone(),
                sources: correction.sources.clone(),
            });
        }
        cursor = end;
    }
    if cursor < source_text.len() {
        corrected_text.push_str(&source_text[cursor..]);
    }

    RecognitionRewrite {
        corrected_text,
        term_matches,
    }
}

pub(crate) fn rewrite_translation_terms(
    source_text: &str,
    translated_text: &str,
    target_language: &str,
    terms: &[CorpusPromptTerm],
) -> TerminologyRewrite {
    let target_language = normalized_language_code(target_language);
    if target_language.is_empty() || source_text.is_empty() || translated_text.is_empty() {
        return TerminologyRewrite {
            translated_text: translated_text.to_owned(),
            term_matches: Vec::new(),
        };
    }

    let matched_terms = terms
        .iter()
        .filter_map(|term| {
            let source_values = term
                .values
                .iter()
                .filter(|(language, _)| normalized_language_code(language) != target_language)
                .map(|(_, value)| value.as_str())
                .collect::<Vec<_>>();
            let target_value = term
                .values
                .iter()
                .find(|(language, _)| normalized_language_code(language) == target_language)
                .map(|(_, value)| value.trim())
                .filter(|value| !value.is_empty())?;
            source_values
                .iter()
                .any(|value| contains_term(source_text, value))
                .then_some((source_values, target_value, term))
        })
        .collect::<Vec<_>>();

    if matched_terms.is_empty() {
        return TerminologyRewrite {
            translated_text: translated_text.to_owned(),
            term_matches: Vec::new(),
        };
    }

    let mut replacements = Vec::new();
    for (source_values, target_value, term) in matched_terms {
        let mut candidates = source_values;
        candidates.push(target_value);
        candidates.sort_by_key(|value| std::cmp::Reverse(value.len()));
        candidates.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
        for candidate in candidates {
            for (start, end) in term_spans(translated_text, candidate) {
                replacements.push(Replacement {
                    start,
                    end,
                    text: target_value.to_owned(),
                    sources: term.sources.clone(),
                });
            }
        }
    }
    replacements.sort_by(|left, right| {
        left.start
            .cmp(&right.start)
            .then_with(|| right.end.cmp(&left.end))
    });

    let mut selected: Vec<Replacement> = Vec::new();
    for replacement in replacements {
        if selected
            .iter()
            .any(|existing| replacement.start < existing.end && replacement.end > existing.start)
        {
            continue;
        }
        selected.push(replacement);
    }

    let mut rewritten = String::with_capacity(translated_text.len());
    let mut matches = Vec::new();
    let mut cursor = 0usize;
    for replacement in selected {
        if cursor < replacement.start {
            rewritten.push_str(&translated_text[cursor..replacement.start]);
        }
        let start = rewritten.len();
        rewritten.push_str(&replacement.text);
        let end = rewritten.len();
        if let (Ok(start_byte), Ok(end_byte)) = (u32::try_from(start), u32::try_from(end)) {
            matches.push(CorpusTermMatch {
                start_byte,
                end_byte,
                text: replacement.text,
                sources: replacement.sources,
            });
        }
        cursor = replacement.end;
    }
    if cursor < translated_text.len() {
        rewritten.push_str(&translated_text[cursor..]);
    }

    TerminologyRewrite {
        translated_text: rewritten,
        term_matches: matches,
    }
}

#[derive(Clone)]
struct Replacement {
    start: usize,
    end: usize,
    text: String,
    sources: Vec<xrtranslate_protocol::CorpusTermSource>,
}

fn contains_term(text: &str, term: &str) -> bool {
    !term_spans(text, term).is_empty()
}

fn term_spans(text: &str, term: &str) -> Vec<(usize, usize)> {
    let needle = term.trim();
    if needle.is_empty() {
        return Vec::new();
    }
    let needle_lower = needle.to_lowercase();
    let mut spans = Vec::new();
    for (start, _) in text.char_indices() {
        let mut folded = String::new();
        for (relative, character) in text[start..].char_indices() {
            folded.extend(character.to_lowercase());
            if folded.len() < needle_lower.len() {
                continue;
            }
            if folded.len() > needle_lower.len() {
                break;
            }
            let end = start + relative + character.len_utf8();
            if folded == needle_lower && term_boundary_matches(text, start, end, needle) {
                spans.push((start, end));
            }
            break;
        }
    }
    spans
}

fn term_boundary_matches(text: &str, start: usize, end: usize, value: &str) -> bool {
    if !value.is_ascii() {
        return true;
    }
    let starts_with_word = value
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_alphanumeric());
    let ends_with_word = value
        .chars()
        .next_back()
        .is_some_and(|character| character.is_ascii_alphanumeric());
    let before_is_word = text[..start]
        .chars()
        .next_back()
        .is_some_and(|character| character.is_ascii_alphanumeric());
    let after_is_word = text[end..]
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_alphanumeric());
    (!starts_with_word || !before_is_word) && (!ends_with_word || !after_is_word)
}

fn normalized_language_code(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .replace('_', "-")
        .split('-')
        .next()
        .unwrap_or_default()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use xr_corpus_protocol::CorpusTermSource;

    fn term(values: &[(&str, &str)]) -> CorpusPromptTerm {
        CorpusPromptTerm {
            values: values
                .iter()
                .map(|(language, value)| ((*language).into(), (*value).into()))
                .collect(),
            sources: vec![CorpusTermSource {
                corpus_id: "games.overwatch.heroes".into(),
                domain: "games".into(),
                subdomain: "overwatch".into(),
                title: "Overwatch Heroes".into(),
            }],
        }
    }

    fn correction(
        start: u32,
        end: u32,
        original: &str,
        corrected: &str,
    ) -> CorpusRecognitionCorrection {
        CorpusRecognitionCorrection {
            start_byte: start,
            end_byte: end,
            original_text: original.into(),
            corrected_text: corrected.into(),
            sources: vec![CorpusTermSource {
                corpus_id: "virtual-worlds.vrchat.community-language".into(),
                domain: "virtual-worlds".into(),
                subdomain: "vrchat".into(),
                title: "VRChat 社区用语".into(),
            }],
        }
    }

    #[test]
    fn rewrites_recognition_correction_spans_and_marks_corrected_term() {
        let rewrite = rewrite_recognition_terms(
            "Yeah, you are a fanboy.",
            &[correction(16, 22, "fanboy", "femboy")],
        );

        assert_eq!(rewrite.corrected_text, "Yeah, you are a femboy.");
        assert_eq!(rewrite.term_matches.len(), 1);
        assert_eq!(rewrite.term_matches[0].text, "femboy");
        assert_eq!(rewrite.term_matches[0].start_byte, 16);
        assert_eq!(rewrite.term_matches[0].end_byte, 22);
    }

    #[test]
    fn ignores_recognition_correction_with_invalid_utf8_boundaries() {
        let source = "不会查到一家PG号";
        // Both offsets intentionally fall inside the first three-byte
        // character, as can happen with stale Corpus byte offsets.
        let rewrite = rewrite_recognition_terms(source, &[correction(1, 2, "", "Public")]);

        assert_eq!(rewrite.corrected_text, source);
        assert!(rewrite.term_matches.is_empty());
    }

    #[test]
    fn rewrites_copied_source_term_to_target_value_case_insensitively() {
        let rewrite = rewrite_translation_terms(
            "I love Mercy",
            "\u{6211}\u{559c}\u{6b22}MERCY\u{3002}",
            "zh",
            &[term(&[("en", "Mercy"), ("zh", "\u{5929}\u{4f7f}")])],
        );

        assert_eq!(
            rewrite.translated_text,
            "\u{6211}\u{559c}\u{6b22}\u{5929}\u{4f7f}\u{3002}"
        );
        assert_eq!(rewrite.term_matches.len(), 1);
        assert_eq!(rewrite.term_matches[0].text, "\u{5929}\u{4f7f}");
        assert_eq!(rewrite.term_matches[0].start_byte, 9);
        assert_eq!(rewrite.term_matches[0].end_byte, 15);
    }

    #[test]
    fn keeps_existing_target_term_and_marks_it() {
        let rewrite = rewrite_translation_terms(
            "I love Mercy",
            "\u{6211}\u{559c}\u{6b22}\u{5929}\u{4f7f}\u{3002}",
            "zh",
            &[term(&[("en", "Mercy"), ("zh", "\u{5929}\u{4f7f}")])],
        );

        assert_eq!(
            rewrite.translated_text,
            "\u{6211}\u{559c}\u{6b22}\u{5929}\u{4f7f}\u{3002}"
        );
        assert_eq!(rewrite.term_matches.len(), 1);
        assert_eq!(rewrite.term_matches[0].text, "\u{5929}\u{4f7f}");
        assert_eq!(rewrite.term_matches[0].start_byte, 9);
        assert_eq!(rewrite.term_matches[0].end_byte, 15);
    }

    #[test]
    fn does_not_rewrite_without_source_match_or_target_value() {
        let missing_source = rewrite_translation_terms(
            "Hello",
            "MERCY",
            "zh",
            &[term(&[("en", "Mercy"), ("zh", "\u{5929}\u{4f7f}")])],
        );
        let missing_target =
            rewrite_translation_terms("I love Mercy", "MERCY", "zh", &[term(&[("en", "Mercy")])]);

        assert_eq!(missing_source.translated_text, "MERCY");
        assert!(missing_source.term_matches.is_empty());
        assert_eq!(missing_target.translated_text, "MERCY");
        assert!(missing_target.term_matches.is_empty());
    }

    #[test]
    fn respects_ascii_word_boundaries() {
        let rewrite = rewrite_translation_terms(
            "I love Mercy",
            "SuperMercy and mercy",
            "zh",
            &[term(&[("en", "Mercy"), ("zh", "\u{5929}\u{4f7f}")])],
        );

        assert_eq!(rewrite.translated_text, "SuperMercy and \u{5929}\u{4f7f}");
        assert_eq!(rewrite.term_matches.len(), 1);
    }
}
