//! Optional lyric display data exchanged between plugins via `lyrics.text`.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LyricsText {
    pub title: String,
    pub lyric: String,
    /// Requested surrounding cues; absent for existing single-line consumers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<LyricWindow>,
}

impl LyricsText {
    pub fn preferred(&self) -> &str {
        if self.lyric.is_empty() {
            &self.title
        } else {
            &self.lyric
        }
    }

    pub fn is_empty(&self) -> bool {
        self.title.is_empty() && self.lyric.is_empty() && self.window.is_none()
    }
}

/// Ordered cue text around the active line. The consumer owns layout/budgeting.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LyricWindow {
    pub lines: Vec<String>,
    pub current: usize,
    /// Estimated progress through the current cue, from 0 to 1000.
    pub progress: u16,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_payloads_still_round_trip_without_new_fields() {
        let old = serde_json::json!({"title":"Song","lyric":"Line"});
        let text: LyricsText = serde_json::from_value(old.clone()).unwrap();
        assert!(text.window.is_none());
        assert_eq!(text.preferred(), "Line");
        assert_eq!(serde_json::to_value(text).unwrap(), old);
    }
}
