//! Stable plugin identities and navigation data. Installed manifests own their catalogue.

/// Composition only: each plugin owns its commands and UI-shared handlers.
/// Register ToolProvider implementations here; the host and Agent need no changes.
pub(crate) fn tool_providers(
    app: &mut crate::XRTranslateApp,
) -> Vec<&mut dyn ui_director::ToolProvider> {
    let providers: Vec<&mut dyn ui_director::ToolProvider> = vec![
        &mut app.ui_tools,
        &mut app.director_audio,
        &mut app.plugin_host,
    ];

    providers
}

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::fmt;

/// Stable identifier used by settings, routes and host/plugin messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PluginId {
    bytes: [u8; 64],
    len: u8,
}

impl PluginId {
    pub const AGENT: Self = Self::named("agent");
    pub const VOCABULARY: Self = Self::named("vocabulary");
    pub const OSC: Self = Self::named("osc");
    pub const CHILLOUTVR_OSC: Self = Self::named("chilloutvr_osc");
    pub const AVATAR: Self = Self::named("avatar");
    pub const MEETING: Self = Self::named("meeting");
    pub const MEDIA: Self = Self::named("media");
    pub const VR_OVERLAY: Self = Self::named("vr_overlay");
    pub const OCR: Self = Self::named("ocr");
    pub const AUTO_INPUT: Self = Self::named("auto_input");

    const fn named(value: &str) -> Self {
        let mut bytes = [0; 64];
        let input = value.as_bytes();
        let mut i = 0;
        while i < input.len() {
            bytes[i] = input[i];
            i += 1;
        }
        Self {
            bytes,
            len: input.len() as u8,
        }
    }
    pub const fn as_str(&self) -> &str {
        match std::str::from_utf8(self.bytes.split_at(self.len as usize).0) {
            Ok(value) => value,
            Err(_) => panic!("invalid plugin identifier"),
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "agent" => Some(Self::AGENT),
            "vocabulary" => Some(Self::VOCABULARY),
            "osc" => Some(Self::OSC),
            "chilloutvr_osc" => Some(Self::CHILLOUTVR_OSC),
            "avatar" | "vrc_avatar" => Some(Self::AVATAR),
            "meeting" => Some(Self::MEETING),
            "video_player" | "media" => Some(Self::MEDIA),
            "vr_overlay" => Some(Self::VR_OVERLAY),
            "ocr" => Some(Self::OCR),
            "auto_input" => Some(Self::AUTO_INPUT),
            _ => xrtranslate_plugin::identifier(value)
                .ok()
                .map(|_| Self::named(value)),
        }
    }
}

impl fmt::Display for PluginId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for PluginId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for PluginId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).ok_or_else(|| de::Error::custom(format!("unknown plugin: {value}")))
    }
}

#[derive(Default)]
pub struct PluginPages {
    pub(crate) installed_pages: Vec<PluginPage>,
}

pub(crate) struct PluginPage {
    pub id: PluginId,
    pub title: String,
    pub order: i32,
    pub group: String,
    pub icon: Option<(String, std::sync::Arc<[u8]>)>,
}
