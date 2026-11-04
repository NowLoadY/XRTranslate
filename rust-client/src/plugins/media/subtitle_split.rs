//! Exact source partitioning shared by subtitle tools; local code owns timing.
use super::subtitles::SubtitleCue;
#[cfg(test)]
use super::subtitles::SubtitleTimeline;
use unicode_segmentation::UnicodeSegmentation;
const MIN_CUE_MS: i64 = 1_000;
const MAX_PARTS: usize = 16;

/// Accept only an exact partition, never replacement wording or model timings.
pub(super) fn validated_split(
    cue: &SubtitleCue,
    end_ms: i64,
    parts: &[&str],
) -> Option<Vec<SubtitleCue>> {
    let duration = end_ms.checked_sub(cue.start_ms)?;
    if cue.start_ms < 0
        || !(2..=MAX_PARTS).contains(&parts.len())
        || duration < MIN_CUE_MS * parts.len() as i64
    {
        return None;
    }
    let source = cue.original_text.trim();
    let boundaries: Vec<_> = source
        .split_word_bound_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(source.len()))
        .collect();
    let mut offset = 0;
    let mut weights = Vec::with_capacity(parts.len());
    for part in parts {
        let part = part.trim();
        let rest = source.get(offset..)?.trim_start();
        offset = source.len() - rest.len();
        if part.is_empty()
            || !part.chars().any(char::is_alphanumeric)
            || !rest.starts_with(part)
            || boundaries.binary_search(&offset).is_err()
        {
            return None;
        }
        offset += part.len();
        if boundaries.binary_search(&offset).is_err() {
            return None;
        }
        weights.push(part.chars().filter(|c| !c.is_whitespace()).count() as i128);
    }
    if !source.get(offset..)?.trim().is_empty() {
        return None;
    }
    let total: i128 = weights.iter().sum();
    let mut consumed = 0;
    let mut start_ms = cue.start_ms;
    let mut result = Vec::with_capacity(parts.len());
    for (part, weight) in parts.iter().zip(weights) {
        consumed += weight;
        let end_ms = cue.start_ms + (duration as i128 * consumed / total) as i64;
        if end_ms - start_ms < MIN_CUE_MS {
            return None;
        }
        result.push(SubtitleCue {
            id: uuid::Uuid::new_v4().to_string(),
            start_ms,
            end_ms,
            original_text: part.trim().into(),
            translated_text: None,
            ..cue.clone()
        });
        start_ms = end_ms;
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use xrtranslate_protocol::SegmentTiming;

    fn timeline(text: &str) -> SubtitleTimeline {
        SubtitleTimeline::parse(&format!(
            "WEBVTT\n\ncaption\n00:00:01.000 --> 00:00:13.000 align:start\n{text}\n"
        ))
        .unwrap()
    }

    #[test]
    fn split_preserves_source_coverage_timing_and_export_settings() {
        let mut timeline = timeline("One. Two. Three.");
        timeline.set_translation("srt_0", "一。二。三。".into(), "en", "zh");
        let source = &timeline.cues()[0];
        let parts = validated_split(source, 13_000, &["One. Two.", "Three."]).unwrap();
        assert_eq!((parts[0].start_ms, parts[0].end_ms), (1_000, 7_857));
        assert_eq!((parts[1].start_ms, parts[1].end_ms), (7_857, 13_000));
        assert_ne!(parts[0].id, parts[1].id);
        assert!(
            parts
                .iter()
                .all(|cue| cue.translated_text.is_none() && cue.source_language == "en")
        );
        timeline.replace_split(0, parts);
        assert!(timeline.cues().iter().all(|cue| {
            let metadata = timeline.metadata_for(&cue.id);
            metadata.timing == SegmentTiming::EstimatedTextPartition
                && metadata.finalized
                && !metadata.revisable
        }));
        let vtt = timeline.export_vtt();
        assert_eq!(vtt.matches("align:start").count(), 2);
        let roundtrip = SubtitleTimeline::parse(&vtt).unwrap();
        assert_eq!(
            roundtrip
                .cues()
                .iter()
                .map(|cue| cue.original_text.as_str())
                .collect::<Vec<_>>(),
            ["One. Two.", "Three."]
        );
        assert_eq!(roundtrip.cues()[1].end_ms, 13_000);
        assert_eq!(
            SubtitleTimeline::parse(&timeline.export_srt())
                .unwrap()
                .count(),
            2
        );
    }

    #[test]
    fn rejects_rewrites_omissions_duplicates_word_splits_and_impossible_timings() {
        let timeline = timeline("Test 12 robots. Then deploy.");
        let source = &timeline.cues()[0];
        for parts in [
            &["Test twelve robots.", "Then deploy."][..],
            &["Test 12 robots.", "deploy."],
            &["Test 12 robots.", "Test 12 robots."],
            &["Test 12 rob", "ots. Then deploy."],
            &["Test 12 robots. Then deploy."],
            &["", "Test 12 robots. Then deploy."],
        ] {
            assert!(
                validated_split(source, 13_000, parts).is_none(),
                "{parts:?}"
            );
        }
        let parts = &["Test 12 robots.", "Then deploy."];
        assert!(validated_split(source, 13_000, parts).is_some());
        assert!(validated_split(source, 2_900, parts).is_none());
        let mut unicode = source.clone();
        unicode.original_text = "Cafe\u{301} is open. 欢迎光临。".into();
        assert!(
            validated_split(&unicode, 13_000, &["Cafe", "\u{301} is open. 欢迎光临。"]).is_none()
        );
        assert!(
            validated_split(&unicode, 13_000, &["Cafe\u{301} is open.", "欢迎光临。"]).is_some()
        );
    }
}
