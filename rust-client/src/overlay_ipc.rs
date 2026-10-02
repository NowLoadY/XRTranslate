use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OverlayState {
    pub font_size: u32,
    pub max_items: usize,
    pub visible_entries: Vec<OverlayEntry>,
    pub partial_text: Option<String>,
    pub vad_active: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OverlayEntry {
    pub source: String,
    pub translated: String,
    pub live: bool,
    pub vad_active: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OcrOverlayState {
    pub source: String,
    pub translated: String,
    pub status: Option<String>,
    pub busy: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OverlayControls {
    pub translation_enabled: bool,
    pub microphone_enabled: Option<bool>,
    pub system_audio_enabled: Option<bool>,
    pub ocr_enabled: Option<bool>,
}

/// Screen coordinates use physical pixels, including negative monitor origins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverlayRegion {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OverlayCommand {
    Language(crate::i18n::UiLanguage),
    Controls(OverlayControls),
    Subtitles(OverlayState),
    Ocr(Option<OcrOverlayState>),
    Hide,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OverlayEvent {
    TranslationEnabled(bool),
    MicrophoneEnabled(bool),
    SystemAudioEnabled(bool),
    CloseRequested,
    Failed(String),
    OcrEnabled(bool),
    RegionChanging,
    RegionChanged(OverlayRegion),
    ResultChanging,
    ResultRegionChanged(Option<OverlayRegion>),
}

#[cfg(test)]
mod tests {
    use super::OverlayState;

    #[test]
    fn vad_activity_is_sent_to_the_overlay_process() {
        let state = OverlayState {
            font_size: 14,
            max_items: 5,
            visible_entries: Vec::new(),
            partial_text: None,
            vad_active: true,
        };
        let json = serde_json::to_string(&state).unwrap();

        assert!(json.contains(r#""vad_active":true"#));
    }
}
