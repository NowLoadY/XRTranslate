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
    pub header: BannerConfig,
    pub footer: BannerConfig,
    pub messages: bool,
    pub format: MessageFormat,
    pub audio_source_filter: AudioSourceFilter,
    pub message_seconds: u64,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            connector: Connector::default(),
            header: BannerConfig::default(),
            footer: BannerConfig::default(),
            messages: true,
            format: MessageFormat::TargetOnly,
            audio_source_filter: AudioSourceFilter::default(),
            message_seconds: 8,
        }
    }
}
impl Settings {
    pub fn lyric_budget(&self) -> Option<usize> {
        [&self.header, &self.footer]
            .iter()
            .any(|banner| banner.content_type == BannerContentType::Lyrics)
        .then_some(24)
    }
}
