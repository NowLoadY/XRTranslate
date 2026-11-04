//! Scan locally; send locations and counts, never the whole subtitle document.
use super::super::subtitles::SubtitleTimeline;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use unicode_segmentation::UnicodeSegmentation;

pub(super) const KINDS: &[&str] = &[
    "too_long",
    "untranslated",
    "invalid_timing",
    "short_source",
    "short_translation",
    "repeated_source_boundary",
    "repeated_translation_boundary",
];

#[derive(Serialize)]
struct Finding<'a> {
    cue: &'a str,
    index: usize,
    characters: usize,
    problems: Vec<&'static str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    hints: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_index: Option<usize>,
}

fn problems(timeline: &SubtitleTimeline, index: usize, limit: usize) -> Vec<&'static str> {
    let cue = &timeline.cues()[index];
    let mut result = Vec::new();
    if cue.character_count() > limit {
        result.push("too_long");
    }
    if cue
        .translated_text
        .as_deref()
        .is_none_or(|text| text.trim().is_empty())
    {
        result.push("untranslated");
    }
    if cue.start_ms < 0 || timeline.effective_end_at(index) <= cue.start_ms {
        result.push("invalid_timing");
    }
    result
}

pub(super) fn issues(timeline: &SubtitleTimeline, limit: usize) -> Vec<Value> {
    timeline.cues().iter().enumerate().filter_map(|(index, cue)| {
        let problems = problems(timeline, index, limit);
        (!problems.is_empty()).then(|| json!({"cue":cue.id,"index":index,"characters":cue.character_count(),"problems":problems}))
    }).collect()
}

fn words(text: &str) -> Vec<String> {
    text.unicode_words().map(str::to_lowercase).collect()
}

fn repeated_boundary(previous: &[String], current: &[String]) -> bool {
    // Whole Unicode words avoid matching "he" with "the". Bound work per pair.
    (1..=previous.len().min(current.len()).min(32))
        .rev()
        .any(|size| {
            previous[previous.len() - size..] == current[..size]
                && current[..size]
                    .iter()
                    .map(|word| word.chars().count())
                    .sum::<usize>()
                    >= 2
        })
}

pub(super) fn report(
    timeline: &SubtitleTimeline,
    limit: usize,
    revision: u64,
    offset: usize,
    page_size: usize,
    kind: Option<&str>,
) -> Value {
    let mut findings = Vec::new();
    let mut counts = BTreeMap::<&str, usize>::new();
    let mut error_count = 0;
    let mut review_count = 0;
    let mut previous_words = [Vec::new(), Vec::new()];
    for (index, cue) in timeline.cues().iter().enumerate() {
        let current_words = [
            words(&cue.original_text),
            words(cue.translated_text.as_deref().unwrap_or("")),
        ];
        let mut hints = Vec::new();
        for (words, hint) in current_words
            .iter()
            .zip(["short_source", "short_translation"])
        {
            if (1..=2).contains(&words.len()) {
                hints.push(hint);
            }
        }
        let mut previous_index = None;
        if index > 0
            && cue
                .start_ms
                .saturating_sub(timeline.effective_end_at(index - 1))
                <= 2_000
        {
            for ((previous, current), hint) in previous_words
                .iter()
                .zip(&current_words)
                .zip(["repeated_source_boundary", "repeated_translation_boundary"])
            {
                if repeated_boundary(previous, current) {
                    hints.push(hint);
                    previous_index = Some(index - 1);
                }
            }
        }
        previous_words = current_words;
        let problems = problems(timeline, index, limit);
        error_count += usize::from(!problems.is_empty());
        review_count += usize::from(!hints.is_empty());
        for problem in problems.iter().chain(&hints) {
            *counts.entry(problem).or_default() += 1;
        }
        if problems.is_empty() && hints.is_empty() {
            continue;
        }
        if kind.is_some_and(|kind| !problems.contains(&kind) && !hints.contains(&kind)) {
            continue;
        }
        findings.push(Finding {
            cue: &cue.id,
            index,
            characters: cue.character_count(),
            problems,
            hints,
            previous_index,
        });
    }
    let matched = findings.len();
    let page: Vec<_> = findings.into_iter().skip(offset).take(page_size).collect();
    let next = offset.saturating_add(page.len());
    json!({"revision":revision,"cue_count":timeline.count(),"count":error_count,"review_count":review_count,"counts":counts,"matched_count":matched,"issues":page,"next_offset":(next < matched).then_some(next)})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn movie_length_index_is_paged_without_text_and_hints_do_not_become_errors() {
        let cues: Vec<_> = (0..2_000)
            .map(|index| {
                json!({
                    "id":format!("cue-{index}"),"start_ms":index*4_000,"end_ms":index*4_000+3_900,
                    "original_text":"Yes.","translated_text":"好的。","speaker_name":null,
                })
            })
            .collect();
        let timeline: SubtitleTimeline =
            serde_json::from_value(json!({"enabled":true,"cues":cues})).unwrap();
        let page = report(&timeline, 200, 3, 1_900, 12, Some("short_source"));
        assert_eq!(page["cue_count"], 2_000);
        assert_eq!(page["count"], 0, "short dialogue is not a structural error");
        assert_eq!(page["matched_count"], 2_000);
        assert_eq!(page["issues"][0]["index"], 1_900);
        assert_eq!(page["issues"].as_array().unwrap().len(), 12);
        assert_eq!(page["next_offset"], 1_912);
        assert!(!page.to_string().contains("original_text"));
        assert!(!page.to_string().contains("translated_text"));
        assert!(page.to_string().len() < 8_000);
        assert!(issues(&timeline, 200).is_empty());

        assert!(repeated_boundary(
            &words("We crossed the bridge."),
            &words("THE bridge was quiet.")
        ));
        assert!(repeated_boundary(
            &words("我们走到桥上。"),
            &words("桥上没有人。")
        ));
        assert!(!repeated_boundary(
            &words("I spoke with he"),
            &words("the next day")
        ));
        let mut cue = timeline.cues()[1].clone();
        cue.original_text = "A very long subtitle ".repeat(30);
        let mut timeline = timeline;
        timeline.edit_cue(cue).unwrap();
        let long = report(&timeline, 200, 4, 0, 12, Some("too_long"));
        assert_eq!(long["count"], 1);
        assert_eq!(long["matched_count"], 1);
        assert_eq!(long["issues"][0]["index"], 1);
    }
}
