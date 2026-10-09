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
    pub status: Option<String>,
    pub busy: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OverlayControls {
    pub translation_enabled: bool,
    pub microphone_enabled: Option<bool>,
    pub system_audio_enabled: Option<bool>,
    pub ocr_enabled: Option<bool>,
    #[serde(default)]
    pub auto_input_available: bool,
    #[serde(default)]
    pub auto_input_enabled: bool,
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
    Companion(crate::ui::components::avatar::Presentation),
    Hide,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OverlayEvent {
    TranslationEnabled(bool),
    MicrophoneEnabled(bool),
    SystemAudioEnabled(bool),
    AutoInputEnabled(bool),
    CloseRequested,
    Failed(String),
    OcrEnabled(bool),
    RegionChanging,
    RegionChanged(OverlayRegion),
    ResultChanging,
    ResultRegionChanged(Option<OverlayRegion>),
    CompanionDetached(bool),
    CompanionRegionChanged(Option<OverlayRegion>),
}
