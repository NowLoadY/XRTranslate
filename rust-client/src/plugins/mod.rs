//! Built-in plugin identities, page catalogue and enablement preferences.
//!
//! Plugins are linked into the client and identified through stable IDs.
//! Plugins with standalone pages contribute navigation and settings here;
//! embedded inputs follow their shared capability's configuration. The catalogue removes
//! repeated navigation/settings metadata; statically typed runtime composition
//! remains explicit in the host adapter.

pub(crate) mod agent;
pub(crate) mod avatar;
pub(crate) mod game_plugins;
pub mod media;
pub mod meeting;
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub mod ocr;
pub mod osc;
pub(crate) mod text_output;
pub(crate) mod vocabulary;
pub mod vr_overlay;

/// Composition only: each plugin owns its commands and UI-shared handlers.
/// Register ToolProvider implementations here; the host and Agent need no changes.
pub(crate) fn tool_providers(
    app: &mut crate::XRTranslateApp,
) -> Vec<&mut dyn ui_director::ToolProvider> {
    let media = app.plugin_enabled(PluginId::MEDIA);
    let agent = app.plugin_enabled(PluginId::AGENT);
    let mut providers: Vec<&mut dyn ui_director::ToolProvider> = vec![
        &mut app.ui_tools,
        &mut app.director_audio,
        &mut app.plugin_host,
    ];
    if media {
        providers.push(&mut app.media_plugin);
    }
    if agent {
        providers.push(&mut app.agent);
    }
    providers
}

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::{collections::BTreeMap, fmt};

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
    pub const MEDIA: Self = Self::named("video_player");
    pub const VR_OVERLAY: Self = Self::named("vr_overlay");
    pub const OCR: Self = Self::named("ocr");
    pub const AUTO_INPUT: Self = Self::named("auto_input");

    pub fn is_supported(self) -> bool {
        match self {
            Self::AUTO_INPUT => cfg!(windows),
            Self::OCR => cfg!(any(target_os = "windows", target_os = "linux")),
            Self::AVATAR => cfg!(any(target_os = "windows", target_os = "linux")),
            Self::VR_OVERLAY => !cfg!(target_os = "android"),
            _ => true,
        }
    }

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
    /// None for embedded plugins without a standalone page.
    pub navigation_order: Option<u16>,
    pub icon: PluginIcon,
    pub scroll_policy: PluginScrollPolicy,
    pub settings_contribution: PluginSettingsContribution,
    pub default_enabled: bool,
}

const PLUGIN_DESCRIPTORS: &[PluginDescriptor] = &[
    PluginDescriptor {
        id: PluginId::AGENT,
        title_key: "Agent",
        description_key: "Optimize tasks using the configured model.",
        navigation_order: None,
        icon: PluginIcon {
            uri: "bytes://plugins/agent/icon.svg",
            bytes: include_bytes!("../../resources/icons/star.svg"),
        },
        scroll_policy: PluginScrollPolicy::Plugin,
        settings_contribution: PluginSettingsContribution::EnablementOnly,
        default_enabled: true,
    },
    PluginDescriptor {
        id: PluginId::VOCABULARY,
        title_key: "Vocabulary",
        description_key: "Manage wordbooks and saved words.",
        navigation_order: Some(80),
        icon: PluginIcon {
            uri: "bytes://plugins/vocabulary/icon.svg",
            bytes: include_bytes!("../../resources/icons/star.svg"),
        },
        scroll_policy: PluginScrollPolicy::Plugin,
        settings_contribution: PluginSettingsContribution::EnablementOnly,
        default_enabled: true,
    },
    PluginDescriptor {
        id: PluginId::MEETING,
        title_key: "Meeting notes",
        description_key: "Capture, import, organize, and export local meeting records.",
        navigation_order: Some(100),
        icon: PluginIcon {
            uri: "bytes://plugins/meeting/icon.svg",
            bytes: include_bytes!("../../resources/plugins/meeting/icon.svg"),
        },
        scroll_policy: PluginScrollPolicy::Plugin,
        settings_contribution: PluginSettingsContribution::EnablementOnly,
        default_enabled: true,
    },
    PluginDescriptor {
        id: PluginId::MEDIA,
        title_key: "Media",
        description_key: "Translate audio, video and subtitle files.",
        navigation_order: Some(150),
        icon: PluginIcon {
            uri: "bytes://plugins/media/icon.svg",
            bytes: include_bytes!("../../resources/plugins/media/icon.svg"),
        },
        scroll_policy: PluginScrollPolicy::Plugin,
        settings_contribution: PluginSettingsContribution::EnablementOnly,
        default_enabled: true,
    },
    PluginDescriptor {
        id: PluginId::VR_OVERLAY,
        title_key: "SteamVR Overlay",
        description_key: "Display private real-time bilingual subtitles in SteamVR (HMD-locked HUD).",
        navigation_order: Some(180),
        icon: PluginIcon {
            uri: "bytes://plugins/vr_overlay/icon.svg",
            bytes: include_bytes!("../../resources/plugins/vr_overlay/icon.svg"),
        },
        scroll_policy: PluginScrollPolicy::Plugin,
        settings_contribution: PluginSettingsContribution::Plugin,
        default_enabled: !cfg!(target_os = "android"),
    },
    PluginDescriptor {
        id: PluginId::OSC,
        title_key: "VRChat OSC",
        description_key: "Send translation captions to VRChat and follow avatar mute state.",
        navigation_order: Some(200),
        icon: PluginIcon {
            uri: "bytes://plugins/osc/icon.svg",
            bytes: include_bytes!("../../resources/plugins/osc/icon.svg"),
        },
        scroll_policy: PluginScrollPolicy::Plugin,
        settings_contribution: PluginSettingsContribution::Plugin,
        default_enabled: !cfg!(target_os = "android"),
    },
    PluginDescriptor {
        id: PluginId::CHILLOUTVR_OSC,
        title_key: "ChilloutVR OSC",
        description_key: "Send translation captions to ChilloutVR chat bubbles.",
        navigation_order: Some(205),
        icon: PluginIcon {
            uri: "bytes://plugins/osc/icon.svg",
            bytes: include_bytes!("../../resources/plugins/osc/icon.svg"),
        },
        scroll_policy: PluginScrollPolicy::Plugin,
        settings_contribution: PluginSettingsContribution::Plugin,
        default_enabled: false,
    },
];

/// Catalogue for built-in plugins compiled into this build.
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
            && !preferences
                .installed_pages
                .iter()
                .any(|(installed, _)| *installed == id)
            && (self
                .descriptor(id)
                .is_none_or(|descriptor| descriptor.navigation_order.is_none())
                || !preferences.is_enabled(id))
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
    #[serde(skip)]
    pub(crate) installed_pages: Vec<(PluginId, String)>,
}

impl Default for PluginPreferences {
    fn default() -> Self {
        let mut preferences = Self {
            enabled: BTreeMap::new(),
            installed_pages: Vec::new(),
        };
        PluginRegistry::builtin().initialize_preferences(&mut preferences);
        preferences
    }
}

impl PluginPreferences {
    pub fn is_enabled(&self, id: PluginId) -> bool {
        if !id.is_supported() {
            return false;
        }
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
