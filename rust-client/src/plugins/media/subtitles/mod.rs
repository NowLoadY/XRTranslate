mod formats;

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use xrtranslate_protocol::{SegmentBoundary, SegmentTiming};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubtitleCue {
    pub id: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub speaker_name: Option<String>,
    pub original_text: String,
    pub translated_text: Option<String>,
}

/// Neutral translation facts converted into a player-owned subtitle cue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranslationCueInput {
    pub turn_id: String,
    pub segment_index: u32,
    pub stream_id: Option<u64>,
    pub start_ms: i64,
    pub end_ms: i64,
    pub speaker_id: String,
    pub source: String,
    pub translated: String,
    pub timing: SegmentTiming,
    pub boundary: SegmentBoundary,
    pub revisable: bool,
    pub finalized: bool,
}

/// Applies player subtitle identity and presentation metadata to a generic
/// translation result. Audio-source/session ownership decisions stay in host
/// coordination; this function is deterministic and platform-independent.
pub fn cue_from_translation(input: TranslationCueInput) -> (SubtitleCue, SubtitleMetadata) {
    let id = if !input.turn_id.is_empty() {
        let turn = format!("turn_{}_segment_{}", input.turn_id, input.segment_index);
        match input.stream_id {
            Some(stream) => format!("stream_{stream}_{turn}"),
            None => turn,
        }
    } else if let Some(stream_id) = input.stream_id {
        format!("stream_{}_{}", stream_id, input.start_ms)
    } else {
        format!("cue_{}", input.start_ms)
    };
    let speaker_name = (!input.speaker_id.is_empty()).then_some(input.speaker_id);
    let metadata = SubtitleMetadata {
        timing: input.timing,
        boundary: input.boundary,
        revisable: input.revisable,
        finalized: input.finalized,
    };
    (
        SubtitleCue {
            id,
            start_ms: input.start_ms,
            end_ms: input.end_ms,
            speaker_name,
            original_text: input.source,
            translated_text: Some(input.translated),
        },
        metadata,
    )
}

/// Generic recognition metadata used to turn a transcript window into a
/// display cue. It deliberately contains no player-specific or backend-model
/// details, so other subtitle-producing plugins can apply their own policy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubtitleMetadata {
    pub timing: SegmentTiming,
    pub boundary: SegmentBoundary,
    pub revisable: bool,
    pub finalized: bool,
}

impl SubtitleMetadata {
    pub const fn authored() -> Self {
        Self {
            timing: SegmentTiming::Authored,
            boundary: SegmentBoundary::InputBoundary,
            revisable: false,
            finalized: true,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SubtitleTimeline {
    cues: Vec<SubtitleCue>,
    #[serde(default)]
    metadata: BTreeMap<String, SubtitleMetadata>,
    #[serde(skip)]
    snapshot_cues: BTreeMap<u64, BTreeSet<String>>,
    #[serde(default)]
    origins: BTreeMap<String, formats::CueOrigin>,
    #[serde(skip)]
    revision: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    vtt_prelude: Vec<String>,
    pub enabled: bool,
}

impl SubtitleTimeline {
    pub fn new() -> Self {
        Self {
            cues: Vec::new(),
            metadata: BTreeMap::new(),
            snapshot_cues: BTreeMap::new(),
            origins: BTreeMap::new(),
            revision: 0,
            vtt_prelude: Vec::new(),
            enabled: true,
        }
    }

    pub fn clear(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.vtt_prelude.clear();
        self.origins.clear();
        self.cues.clear();
        self.metadata.clear();
        self.snapshot_cues.clear();
    }

    pub(crate) fn finalize_live_cues(&mut self, stream_id: u64) {
        let prefix = format!("stream_{stream_id}_");
        for (id, metadata) in &mut self.metadata {
            if id.starts_with(&prefix) {
                metadata.finalized = true;
            }
        }
    }

    pub(crate) fn finish_stream(&mut self, stream_id: u64) {
        self.finalize_live_cues(stream_id);
        self.snapshot_cues.remove(&stream_id);
    }

    pub(crate) fn replace_stream_cues(
        &mut self,
        stream_id: u64,
        cues: impl IntoIterator<Item = (SubtitleCue, SubtitleMetadata)>,
    ) {
        self.revision = self.revision.wrapping_add(1);
        let previous = self.snapshot_cues.remove(&stream_id).unwrap_or_default();
        let prefix = format!("stream_{stream_id}_");
        self.cues.retain(|cue| {
            let metadata = self.metadata.get(&cue.id).copied().unwrap_or_default();
            let replace = metadata.timing != SegmentTiming::Authored
                && (previous.contains(&cue.id)
                    || (cue.id.starts_with(&prefix) && !metadata.finalized));
            if replace {
                self.metadata.remove(&cue.id);
            }
            !replace
        });
        let mut ids = BTreeSet::new();
        for (cue, metadata) in cues {
            ids.insert(cue.id.clone());
            self.add_cue_with_metadata(cue, metadata);
        }
        self.snapshot_cues.insert(stream_id, ids);
    }

    /// Keeps completed translations only while their recognized sentence still
    /// belongs to this turn. Other turns and authored subtitles are independent.
    pub(crate) fn retain_source_snapshot<'a>(
        &mut self,
        stream_id: u64,
        turn_id: &str,
        sources: impl IntoIterator<Item = (u32, &'a str)>,
    ) {
        self.revision = self.revision.wrapping_add(1);
        let sources = sources.into_iter().collect::<BTreeMap<_, _>>();
        let prefix = format!("stream_{stream_id}_turn_{turn_id}_segment_");
        self.cues.retain(|cue| {
            let Some(index) = cue
                .id
                .strip_prefix(&prefix)
                .and_then(|index| index.parse::<u32>().ok())
            else {
                return true;
            };
            if self
                .metadata
                .get(&cue.id)
                .is_some_and(|metadata| metadata.timing == SegmentTiming::Authored)
                || sources
                    .get(&index)
                    .is_some_and(|source| *source == cue.original_text)
            {
                return true;
            }
            self.metadata.remove(&cue.id);
            if let Some(snapshot) = self.snapshot_cues.get_mut(&stream_id) {
                snapshot.remove(&cue.id);
            }
            false
        });
    }

    #[cfg(test)]
    fn add_cue(&mut self, cue: SubtitleCue) -> bool {
        self.add_cue_with_metadata(cue, SubtitleMetadata::default())
    }

    pub fn add_cue_with_metadata(&mut self, cue: SubtitleCue, metadata: SubtitleMetadata) -> bool {
        self.revision = self.revision.wrapping_add(1);
        if cue.original_text.trim().is_empty()
            && cue
                .translated_text
                .as_ref()
                .is_none_or(|t| t.trim().is_empty())
        {
            return false;
        }

        // 1. Check exact ID match (turn_id or stream_id based)
        if let Some(index) = self.cues.iter().position(|c| c.id == cue.id) {
            return self.replace_cue(index, cue, metadata);
        }

        // 2. Check semantic & temporal duplication. Proximity alone is not a
        // duplicate: fast dialogue legitimately produces distinct nearby cues.
        let cue_orig = cue.original_text.trim();
        let cue_translation = cue.translated_text.as_deref().map(str::trim);
        if let Some(index) = self.cues.iter().position(|c| {
            if has_stable_identity(&cue.id) && has_stable_identity(&c.id) {
                return false;
            }
            let time_diff = (c.start_ms - cue.start_ms).abs();
            let same_content = if cue_orig.is_empty() {
                cue_translation.is_some_and(|translation| {
                    !translation.is_empty()
                        && c.translated_text.as_deref().map(str::trim) == Some(translation)
                })
            } else {
                c.original_text.trim() == cue_orig
            };
            time_diff <= 3500 && same_content
        }) {
            return self.replace_cue(index, cue, metadata);
        }

        // 3. New distinct subtitle
        self.metadata.insert(cue.id.clone(), metadata);
        self.cues.push(cue);
        self.cues.sort_by_key(|c| c.start_ms);
        true
    }

    fn replace_cue(&mut self, index: usize, cue: SubtitleCue, metadata: SubtitleMetadata) -> bool {
        let previous_id = self.cues[index].id.clone();
        if self.metadata_for(&previous_id).timing == SegmentTiming::Authored
            && metadata.timing != SegmentTiming::Authored
        {
            return false;
        }
        let cue_unchanged = self.cues[index] == cue;
        let metadata_unchanged =
            self.metadata.get(&previous_id).copied().unwrap_or_default() == metadata;
        if cue_unchanged && metadata_unchanged {
            return false;
        }
        if previous_id != cue.id {
            self.metadata.remove(&previous_id);
        }
        self.metadata.insert(cue.id.clone(), metadata);
        self.cues[index] = cue;
        self.cues.sort_by_key(|cue| cue.start_ms);
        true
    }

    pub fn active_cue_at(&self, current_ms: i64) -> Option<&SubtitleCue> {
        if !self.enabled {
            return None;
        }
        // Subtitle lead-in time: display subtitles slightly ahead of speech onset
        // (~250ms advance) to match natural human reading rhythm and visual perception.
        const SUBTITLE_LEAD_IN_MS: i64 = 250;
        let query_ms = current_ms + SUBTITLE_LEAD_IN_MS;
        let end = self.cues.partition_point(|cue| cue.start_ms <= query_ms);
        self.cues[..end]
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, cue)| {
                let effective_end = self.effective_end_at(index);
                (query_ms >= cue.start_ms && current_ms <= effective_end).then_some(cue)
            })
    }

    fn effective_end_at(&self, index: usize) -> i64 {
        let cue = &self.cues[index];
        let supplied_end = if cue.end_ms <= cue.start_ms {
            cue.start_ms + 3000
        } else {
            cue.end_ms
        };
        let metadata = self.metadata_for(&cue.id);
        let padded_end = if metadata.timing == SegmentTiming::Unknown {
            supplied_end.max(cue.start_ms + 2000)
        } else {
            supplied_end
        };
        self.cues
            .get(index + 1)
            .map_or(padded_end, |next| padded_end.min(next.start_ms))
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    pub fn count(&self) -> usize {
        self.cues.len()
    }

    pub fn cues(&self) -> &[SubtitleCue] {
        &self.cues
    }

    pub(crate) fn set_translation(&mut self, id: &str, translated: String) {
        self.revision = self.revision.wrapping_add(1);
        if let Some(cue) = self.cues.iter_mut().find(|cue| cue.id == id) {
            cue.translated_text = Some(translated);
        }
    }

    pub(crate) fn clear_translations(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        for cue in &mut self.cues {
            cue.translated_text = None;
        }
    }

    pub fn metadata_for(&self, cue_id: &str) -> SubtitleMetadata {
        self.metadata.get(cue_id).copied().unwrap_or_default()
    }

    pub fn export_lrc(&self, title: Option<&str>) -> String {
        let mut out = String::new();
        if let Some(t) = title {
            let clean = t.trim();
            if !clean.is_empty() {
                out.push_str(&format!("[ti:{}]\n", clean));
            }
        }
        for cue in self.cues() {
            let time_tag = format_timestamp_lrc(cue.start_ms);
            let orig = cue.original_text.trim();
            if !orig.is_empty() {
                out.push_str(&time_tag);
                out.push_str(orig);
                out.push('\n');
            }
            if let Some(trans) = &cue.translated_text {
                let trans_trim = trans.trim();
                if trans_trim != orig && !trans_trim.is_empty() {
                    out.push_str(&time_tag);
                    out.push_str(trans_trim);
                    out.push('\n');
                }
            }
        }
        out
    }
}

fn has_stable_identity(id: &str) -> bool {
    id.starts_with("turn_") || id.starts_with("stream_") || id.starts_with("srt_")
}

fn format_timestamp_lrc(ms: i64) -> String {
    let ms_max = ms.max(0);
    let mins = ms_max / 60000;
    let secs = (ms_max % 60000) / 1000;
    let hundredths = (ms_max % 1000) / 10;
    format!("[{:02}:{:02}.{:02}]", mins, secs, hundredths)
}

#[cfg(test)]
mod tests {
    #[test]
    fn translation_cue_identity_prefers_turn_and_segment() {
        let base = super::TranslationCueInput {
            turn_id: "turn-a".into(),
            segment_index: 2,
            stream_id: Some(7),
            start_ms: 1_234,
            end_ms: 2_345,
            speaker_id: String::new(),
            source: "hello".into(),
            translated: "你好".into(),
            timing: xrtranslate_protocol::SegmentTiming::UtteranceWindow,
            boundary: xrtranslate_protocol::SegmentBoundary::Silence,
            revisable: true,
            finalized: false,
        };
        let (cue, metadata) = super::cue_from_translation(base.clone());
        assert_eq!(cue.id, "stream_7_turn_turn-a_segment_2");
        assert_eq!(cue.speaker_name, None);
        assert_eq!(cue.translated_text.as_deref(), Some("你好"));
        assert!(metadata.revisable);
        assert!(!metadata.finalized);

        let mut timeline = SubtitleTimeline::new();
        for (turn, stream, index, source) in [
            ("turn-a", 7, 1, "first"),
            ("turn-a", 7, 2, "hello"),
            ("turn-a", 7, 3, "removed"),
            ("turn-b", 7, 1, "another turn"),
            ("turn-a", 8, 1, "another stream"),
        ] {
            let (cue, metadata) = cue_from_translation(TranslationCueInput {
                turn_id: turn.into(),
                stream_id: Some(stream),
                segment_index: index,
                source: source.into(),
                revisable: false,
                finalized: true,
                ..base.clone()
            });
            timeline.add_cue_with_metadata(cue, metadata);
        }
        timeline.retain_source_snapshot(7, "turn-a", [(1, "merged first"), (2, "hello")]);
        assert_eq!(timeline.count(), 3);
        assert!(timeline.cues().iter().any(|cue| {
            cue.id == "stream_7_turn_turn-a_segment_2"
                && cue.translated_text.as_deref() == Some("你好")
        }));
        assert!(
            timeline
                .cues()
                .iter()
                .any(|cue| cue.original_text == "another turn")
        );
        assert!(
            timeline
                .cues()
                .iter()
                .any(|cue| cue.original_text == "another stream")
        );
    }

    #[test]
    fn translation_cue_identity_falls_back_to_stream_or_time() {
        let base = super::TranslationCueInput {
            turn_id: String::new(),
            segment_index: 0,
            stream_id: Some(9),
            start_ms: 1_234,
            end_ms: 2_000,
            speaker_id: "speaker-1".into(),
            source: "hello".into(),
            translated: "你好".into(),
            timing: Default::default(),
            boundary: Default::default(),
            revisable: false,
            finalized: true,
        };
        let (stream_cue, _) = super::cue_from_translation(base.clone());
        assert_eq!(stream_cue.id, "stream_9_1234");
        assert_eq!(stream_cue.speaker_name.as_deref(), Some("speaker-1"));
        let (time_cue, _) = super::cue_from_translation(super::TranslationCueInput {
            stream_id: None,
            ..base
        });
        assert_eq!(time_cue.id, "cue_1234");
    }

    use super::*;

    #[test]
    fn timeline_active_cues_and_srt_export() {
        let mut timeline = SubtitleTimeline::new();
        timeline.add_cue(SubtitleCue {
            id: "1".into(),
            start_ms: 1000,
            end_ms: 3000,
            speaker_name: Some("Speaker".into()),
            original_text: "Hello world".into(),
            translated_text: Some("你好世界".into()),
        });
        assert_eq!(timeline.count(), 1);
        assert_eq!(timeline.cues().len(), 1);
        // With 250ms lead-in, cue starting at 1000ms is visible from 750ms through 3000ms
        assert!(timeline.active_cue_at(740).is_none());
        assert!(timeline.active_cue_at(750).is_some());
        assert!(timeline.active_cue_at(2000).is_some());
        assert!(timeline.active_cue_at(3000).is_some());
        assert!(timeline.active_cue_at(3001).is_none());
        let srt = timeline.export_srt();
        assert!(srt.contains("Hello world"));
        assert!(srt.contains("你好世界"));
        assert!(!srt.contains("[Speaker]"));
    }

    #[test]
    fn test_streaming_cue_in_place_revision() {
        let mut timeline = SubtitleTimeline::new();
        timeline.add_cue(SubtitleCue {
            id: "stream_1_56000".into(),
            start_ms: 56000,
            end_ms: 57200,
            speaker_name: None,
            original_text: "に願いを。".into(),
            translated_text: Some("将愿望寄托在缝隙中。".into()),
        });
        assert_eq!(timeline.count(), 1);

        // Streaming update arrives for the same sentence
        timeline.add_cue(SubtitleCue {
            id: "stream_1_56000".into(),
            start_ms: 56000,
            end_ms: 58100,
            speaker_name: None,
            original_text: "に願い愛を。".into(),
            translated_text: Some("将愿望寄托在爱。".into()),
        });
        assert_eq!(timeline.count(), 1);
        assert_eq!(timeline.cues()[0].original_text, "に願い愛を。");
        assert_eq!(
            timeline.cues()[0].translated_text.as_deref(),
            Some("将愿望寄托在爱。")
        );

        // Final streaming revision arrives
        timeline.add_cue(SubtitleCue {
            id: "stream_1_56000".into(),
            start_ms: 56000,
            end_ms: 59000,
            speaker_name: None,
            original_text: "に願い愛一つ。".into(),
            translated_text: Some("将愿望寄托在爱中。".into()),
        });
        assert_eq!(timeline.count(), 1);
        assert_eq!(timeline.cues()[0].original_text, "に願い愛一つ。");
    }

    #[test]
    fn test_lrc_export() {
        let mut timeline = SubtitleTimeline::new();
        timeline.add_cue(SubtitleCue {
            id: "1".into(),
            start_ms: 13504,
            end_ms: 16480,
            speaker_name: None,
            original_text: "Now I'm seventeen.".into(),
            translated_text: Some("我现在十七岁了。".into()),
        });
        timeline.add_cue(SubtitleCue {
            id: "2".into(),
            start_ms: 16544,
            end_ms: 18544,
            speaker_name: None,
            original_text: "My sky.".into(),
            translated_text: Some("我的天空。".into()),
        });

        let lrc = timeline.export_lrc(Some("17 - 椎名林檎"));
        assert!(lrc.contains("[ti:17 - 椎名林檎]"));
        assert!(lrc.contains("[00:13.50]Now I'm seventeen."));
        assert!(lrc.contains("[00:13.50]我现在十七岁了。"));
        assert!(lrc.contains("[00:16.54]My sky."));
        assert!(lrc.contains("[00:16.54]我的天空。"));
    }

    #[test]
    fn test_subtitles_deduplication_on_repeated_events() {
        let mut timeline = SubtitleTimeline::new();
        // Turn 1 first interim
        timeline.add_cue(SubtitleCue {
            id: "turn_1".into(),
            start_ms: 109000,
            end_ms: 111000,
            speaker_name: Some("speaker-04".into()),
            original_text: "You uh talked".into(),
            translated_text: Some("你刚才讨论了".into()),
        });
        assert_eq!(timeline.count(), 1);

        // Turn 1 second interim with slightly refined start timestamp
        timeline.add_cue(SubtitleCue {
            id: "turn_1".into(),
            start_ms: 109200,
            end_ms: 112000,
            speaker_name: Some("speaker-04".into()),
            original_text: "You uh talked to Louis about Sunday.".into(),
            translated_text: Some("你刚才和路易斯讨论了周日的事。".into()),
        });
        assert_eq!(timeline.count(), 1);
        assert_eq!(
            timeline.cues()[0].original_text,
            "You uh talked to Louis about Sunday."
        );

        // Semantic duplicate with different transient ID but same text and timestamp within 3.5s
        timeline.add_cue(SubtitleCue {
            id: "cue_109200".into(),
            start_ms: 109200,
            end_ms: 112000,
            speaker_name: Some("speaker-04".into()),
            original_text: "You uh talked to Louis about Sunday.".into(),
            translated_text: Some("你刚才和路易斯讨论了周日的事。".into()),
        });
        assert_eq!(timeline.count(), 1);
    }

    #[test]
    fn nearby_distinct_cues_are_not_merged() {
        let mut timeline = SubtitleTimeline::new();
        timeline.add_cue(SubtitleCue {
            id: "turn_1_segment_1".into(),
            start_ms: 1_000,
            end_ms: 2_000,
            speaker_name: None,
            original_text: "First".into(),
            translated_text: Some("第一句".into()),
        });
        timeline.add_cue(SubtitleCue {
            id: "turn_1_segment_2".into(),
            start_ms: 1_500,
            end_ms: 2_500,
            speaker_name: None,
            original_text: "First".into(),
            translated_text: Some("第二句".into()),
        });

        assert_eq!(timeline.count(), 2);
    }

    #[test]
    fn revised_start_time_keeps_timeline_sorted() {
        let mut timeline = SubtitleTimeline::new();
        for (id, start) in [("first", 1_000), ("second", 2_000)] {
            timeline.add_cue(SubtitleCue {
                id: id.into(),
                start_ms: start,
                end_ms: start + 1_000,
                speaker_name: None,
                original_text: id.into(),
                translated_text: None,
            });
        }
        timeline.add_cue(SubtitleCue {
            id: "second".into(),
            start_ms: 500,
            end_ms: 1_500,
            speaker_name: None,
            original_text: "second revised".into(),
            translated_text: None,
        });

        assert_eq!(timeline.cues()[0].id, "second");
        assert_eq!(timeline.cues()[1].id, "first");
    }

    #[test]
    fn observed_timing_is_not_stretched_during_export() {
        let mut timeline = SubtitleTimeline::new();
        timeline.add_cue_with_metadata(
            SubtitleCue {
                id: "turn_1_segment_1".into(),
                start_ms: 1_000,
                end_ms: 1_450,
                speaker_name: None,
                original_text: "Short phrase".into(),
                translated_text: None,
            },
            SubtitleMetadata {
                timing: SegmentTiming::EstimatedTextPartition,
                boundary: SegmentBoundary::Silence,
                revisable: false,
                finalized: true,
            },
        );

        let srt = timeline.export_srt();
        assert!(srt.contains("00:00:01,000 --> 00:00:01,450"));
    }

    #[test]
    fn a_new_cue_is_not_hidden_by_legacy_minimum_duration() {
        let mut timeline = SubtitleTimeline::new();
        for (id, start, end) in [("first", 1_000, 1_200), ("second", 1_500, 1_700)] {
            timeline.add_cue(SubtitleCue {
                id: id.into(),
                start_ms: start,
                end_ms: end,
                speaker_name: None,
                original_text: id.into(),
                translated_text: None,
            });
        }

        assert_eq!(
            timeline.active_cue_at(1_500).map(|cue| cue.id.as_str()),
            Some("second")
        );
    }

    #[test]
    fn finalization_metadata_updates_without_changing_caption_text() {
        let mut timeline = SubtitleTimeline::new();
        let cue = SubtitleCue {
            id: "stream_1".into(),
            start_ms: 1_000,
            end_ms: 1_500,
            speaker_name: None,
            original_text: "Hello".into(),
            translated_text: Some("你好".into()),
        };
        let live = SubtitleMetadata {
            timing: SegmentTiming::MergedWindows,
            boundary: SegmentBoundary::DurationLimit,
            revisable: true,
            finalized: false,
        };
        assert!(timeline.add_cue_with_metadata(cue.clone(), live));
        assert!(timeline.add_cue_with_metadata(
            cue,
            SubtitleMetadata {
                finalized: true,
                ..live
            },
        ));
        assert!(timeline.metadata_for("stream_1").finalized);
    }
}
