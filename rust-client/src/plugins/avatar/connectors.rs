//! Persisted display preferences; wire contracts are owned by the companion SDK.
use crate::{
    plugins::text_output::{BannerConfig, BannerContentType, MessageFormat},
    session_coordinator::AudioSourceFilter,
};
use serde::{Deserialize, Serialize};
pub use xrt_connector_core::{Connector, osc::Contract as OscContract};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub connector: Connector,
    #[serde(deserialize_with = "static_text")]
    pub header: String,
    #[serde(deserialize_with = "static_text")]
    pub footer: String,
    pub messages: bool,
    pub format: MessageFormat,
    pub audio_source_filter: AudioSourceFilter,
    pub message_seconds: u64,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            connector: Connector::default(),
            header: String::new(),
            footer: String::new(),
            messages: true,
            format: MessageFormat::TargetOnly,
            audio_source_filter: AudioSourceFilter::default(),
            message_seconds: 8,
        }
    }
}
// Retain saved custom text without reviving old dynamic banner sources.
fn static_text<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum SavedText {
        Text(String),
        Legacy(BannerConfig),
    }
    Ok(match SavedText::deserialize(deserializer)? {
        SavedText::Text(text) => text,
        SavedText::Legacy(banner) if banner.content_type == BannerContentType::CustomText => {
            banner.custom_text
        }
        SavedText::Legacy(_) => String::new(),
    })
}
