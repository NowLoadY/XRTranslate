//! Built-in plugin catalogue and persisted enablement preferences.
//!
//! Plugins are linked into the client, but their navigation, settings and
//! runtime lifecycles are selected through stable IDs. This catalogue removes
//! repeated navigation/settings metadata; statically typed runtime composition
//! remains explicit in the host adapter.

pub mod meeting;
pub mod osc;
pub mod player;
pub mod vr_overlay;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::{collections::BTreeMap, fmt};

/// Stable identifier used by settings, routes and host/plugin messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PluginId(&'static str);

impl PluginId {
    pub const OSC: Self = Self("osc");
    pub const MEETING: Self = Self("meeting");
    pub const VIDEO_PLAYER: Self = Self("video_player");
    pub const VR_OVERLAY: Self = Self("vr_overlay");

    pub const fn as_str(self) -> &'static str {
        self.0
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "osc" => Some(Self::OSC),
            "meeting" => Some(Self::MEETING),
            "video_player" => Some(Self::VIDEO_PLAYER),
            "vr_overlay" => Some(Self::VR_OVERLAY),
            _ => None,
        }
    }
}

impl fmt::Display for PluginId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl Serialize for PluginId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.0)
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginScrollPolicy {
    /// The application shell provides the page's vertical scroll area.
    #[allow(dead_code)]
    Host,
    /// The plugin manages its own nested or virtualized scroll regions.
    Plugin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginSettingsContribution {
    /// Only the common enable/disable control is shown.
    EnablementOnly,
    /// The plugin contributes additional controls below its enablement row.
    Plugin,
}

#[derive(Clone, Copy)]
pub struct PluginIcon {
    pub uri: &'static str,
    pub bytes: &'static [u8],
}

impl PluginIcon {
    pub fn image_source(self) -> eframe::egui::ImageSource<'static> {
        eframe::egui::ImageSource::Bytes {
            uri: self.uri.into(),
            bytes: self.bytes.into(),
        }
    }
}

#[derive(Clone, Copy)]
pub struct PluginDescriptor {
    pub id: PluginId,
    /// i18n dictionary key, not persisted identity.
    pub title_key: &'static str,
    pub description_key: &'static str,
    pub navigation_order: u16,
    pub icon: PluginIcon,
    pub scroll_policy: PluginScrollPolicy,
    pub settings_contribution: PluginSettingsContribution,
    pub default_enabled: bool,
}

const PLUGIN_DESCRIPTORS: [PluginDescriptor; 4] = [
    PluginDescriptor {
        id: PluginId::MEETING,
        title_key: "Meeting notes",
        description_key: "Capture, import, organize, and export local meeting records.",
        navigation_order: 100,
        icon: PluginIcon {
            uri: "bytes://plugins/meeting/icon.svg",
            bytes: include_bytes!("../../resources/plugins/meeting/icon.svg"),
        },
        scroll_policy: PluginScrollPolicy::Plugin,
        settings_contribution: PluginSettingsContribution::EnablementOnly,
        default_enabled: false,
    },
    PluginDescriptor {
        id: PluginId::VIDEO_PLAYER,
        title_key: "Media Player",
        description_key: "Play video files and streams with real-time synchronized subtitles.",
        navigation_order: 150,
        icon: PluginIcon {
            uri: "bytes://plugins/player/icon.svg",
            bytes: include_bytes!("../../resources/plugins/player/icon.svg"),
        },
        scroll_policy: PluginScrollPolicy::Plugin,
        settings_contribution: PluginSettingsContribution::EnablementOnly,
        default_enabled: false,
    },
    PluginDescriptor {
        id: PluginId::VR_OVERLAY,
        title_key: "SteamVR Overlay",
        description_key: "Display private real-time bilingual subtitles in SteamVR (HMD-locked HUD).",
        navigation_order: 180,
        icon: PluginIcon {
            uri: "bytes://plugins/vr_overlay/icon.svg",
            bytes: include_bytes!("../../resources/plugins/vr_overlay/icon.svg"),
        },
        scroll_policy: PluginScrollPolicy::Plugin,
        settings_contribution: PluginSettingsContribution::Plugin,
        default_enabled: true,
    },
    PluginDescriptor {
        id: PluginId::OSC,
        title_key: "VRChat OSC",
        description_key: "Send translation captions to VRChat and follow avatar mute state.",
        navigation_order: 200,
        icon: PluginIcon {
            uri: "bytes://plugins/osc/icon.svg",
            bytes: include_bytes!("../../resources/plugins/osc/icon.svg"),
        },
        scroll_policy: PluginScrollPolicy::Plugin,
        settings_contribution: PluginSettingsContribution::Plugin,
        default_enabled: true,
    },
];

/// Catalogue for plugins compiled into this build.
#[derive(Clone, Copy, Debug, Default)]
pub struct PluginRegistry;

impl PluginRegistry {
    pub const fn builtin() -> Self {
        Self
    }

    pub const fn descriptors(self) -> &'static [PluginDescriptor] {
        &PLUGIN_DESCRIPTORS
    }

    pub fn descriptor(self, id: PluginId) -> Option<&'static PluginDescriptor> {
        self.descriptors()
            .iter()
            .find(|descriptor| descriptor.id == id)
    }

    pub fn is_enabled(self, preferences: &PluginPreferences, id: PluginId) -> bool {
        preferences.is_enabled(id)
    }

    pub fn set_enabled(self, preferences: &mut PluginPreferences, id: PluginId, enabled: bool) {
        preferences.set_enabled(id, enabled);
    }

    pub fn initialize_preferences(self, preferences: &mut PluginPreferences) {
        for descriptor in self.descriptors() {
            preferences
                .enabled
                .entry(descriptor.id.as_str().to_owned())
                .or_insert(descriptor.default_enabled);
        }
    }

    /// Routes for disabled or unavailable plugins always return to the core
    /// translation page. This is called after settings migration and toggles.
    pub fn normalize_active_page(
        self,
        preferences: &PluginPreferences,
        page: &mut crate::ui::Page,
    ) {
        if let crate::ui::Page::Plugin(id) = *page
            && (self.descriptor(id).is_none() || !preferences.is_enabled(id))
        {
            *page = crate::ui::Page::Translation;
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginPreferences {
    /// String keys deliberately preserve settings for plugins unavailable in a
    /// particular build instead of discarding their preference.
    #[serde(default)]
    enabled: BTreeMap<String, bool>,
}

impl Default for PluginPreferences {
    fn default() -> Self {
        let mut preferences = Self {
            enabled: BTreeMap::new(),
        };
        PluginRegistry::builtin().initialize_preferences(&mut preferences);
        preferences
    }
}

impl PluginPreferences {
    pub fn is_enabled(&self, id: PluginId) -> bool {
        self.enabled.get(id.as_str()).copied().unwrap_or_else(|| {
            PluginRegistry::builtin()
                .descriptor(id)
                .is_some_and(|descriptor| descriptor.default_enabled)
        })
    }

    pub fn set_enabled(&mut self, id: PluginId, enabled: bool) {
        self.enabled.insert(id.as_str().to_owned(), enabled);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_preferences_enable_default_plugins() {
        let preferences = PluginPreferences::default();
        assert!(preferences.is_enabled(PluginId::OSC));
        assert!(preferences.is_enabled(PluginId::VR_OVERLAY));
        assert!(!preferences.is_enabled(PluginId::MEETING));
        assert!(!preferences.is_enabled(PluginId::VIDEO_PLAYER));
    }

    #[test]
    fn plugin_ids_have_stable_string_serialization() {
        assert_eq!(serde_json::to_string(&PluginId::OSC).unwrap(), r#""osc""#);
        assert_eq!(
            serde_json::to_string(&PluginId::VR_OVERLAY).unwrap(),
            r#""vr_overlay""#
        );
        assert_eq!(
            serde_json::from_str::<PluginId>(r#""meeting""#).unwrap(),
            PluginId::MEETING
        );
        assert_eq!(
            serde_json::from_str::<PluginId>(r#""vr_overlay""#).unwrap(),
            PluginId::VR_OVERLAY
        );
    }

    #[test]
    fn descriptors_are_in_navigation_order() {
        assert!(
            PluginRegistry::builtin()
                .descriptors()
                .windows(2)
                .all(|pair| pair[0].navigation_order < pair[1].navigation_order)
        );
    }

    #[test]
    fn core_studios_are_not_plugins() {
        assert!(PluginId::parse("audio_studio").is_none());
        assert!(
            PluginRegistry::builtin()
                .descriptors()
                .iter()
                .all(|descriptor| descriptor.title_key != "Audio Studio")
        );
    }
}
