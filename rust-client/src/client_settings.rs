use crate::audio::InputDevice;
use crate::i18n::UiLanguage;
#[cfg(test)]
use crate::plugins::PluginId;
use crate::plugins::osc::runtime::OscSettings;
use crate::plugins::{PluginPreferences, PluginRegistry};
use crate::ui::{Page, theme::UiTheme};
use serde::{Deserialize, Serialize};
use std::path::Path;
use xrtranslate_prompt::PromptTemplateLibrary;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateChannel {
    #[default]
    Stable,
    Beta,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum CaptureSource {
    #[default]
    Microphone,
    SystemAudio,
    Both,
}

impl CaptureSource {
    pub const fn routes(self) -> &'static [Self] {
        match self {
            Self::Microphone => &[Self::Microphone],
            Self::SystemAudio => &[Self::SystemAudio],
            Self::Both => &[Self::Microphone, Self::SystemAudio],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RecognitionSettings {
    #[serde(default = "default_background_noise")]
    pub background_noise: f32,
    #[serde(default = "default_pause_tolerance")]
    pub pause_tolerance: f32,
    #[serde(default)]
    pub continuous_recognition: bool,
}

impl Default for RecognitionSettings {
    fn default() -> Self {
        Self {
            background_noise: default_background_noise(),
            pause_tolerance: default_pause_tolerance(),
            continuous_recognition: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ClientSettings {
    #[serde(default)]
    pub capture_source: CaptureSource,
    #[serde(default)]
    pub selected_device_id: String,
    #[serde(default)]
    pub selected_loopback_device_id: String,
    #[serde(default)]
    pub selected_tts_output_device_id: String,
    #[serde(default = "default_background_noise")]
    pub background_noise: f32,
    #[serde(default = "default_pause_tolerance")]
    pub pause_tolerance: f32,
    #[serde(default)]
    pub continuous_recognition: bool,
    #[serde(default)]
    pub microphone_recognition: RecognitionSettings,
    #[serde(default)]
    pub loopback_recognition: RecognitionSettings,
    #[serde(default = "default_source_lang")]
    pub source_lang: String,
    #[serde(default = "default_target_lang")]
    pub target_lang: String,
    #[serde(default = "default_true")]
    pub denoise_enabled: bool,
    #[serde(default)]
    pub tts_enabled: bool,
    #[serde(default)]
    pub mute_self_pauses_translation: bool,
    #[serde(default)]
    pub ui_language: UiLanguage,
    #[serde(default)]
    pub ui_theme: UiTheme,
    #[serde(default = "default_true")]
    pub first_run: bool,
    #[serde(default = "default_server_url")]
    pub server_url: String,
    #[serde(default)]
    pub download_proxy_url: String,
    #[serde(default)]
    pub update_channel: UpdateChannel,
    #[serde(default = "OscSettings::from_project_config")]
    pub osc_settings: OscSettings,
    #[serde(default, rename = "plugins", alias = "plugin_preferences")]
    pub plugin_preferences: PluginPreferences,
    #[serde(default)]
    pub active_page: Page,
    #[serde(default)]
    pub sidebar_collapsed: bool,
    #[serde(default)]
    pub floating_subtitles_enabled: bool,
    #[serde(default = "default_floating_max_count")]
    pub floating_subtitles_max_count: usize,
    #[serde(default = "default_floating_font_size")]
    pub floating_subtitles_font_size: f64,
    #[serde(skip)]
    pub prompt_library: PromptTemplateLibrary,
}

fn default_source_lang() -> String {
    "auto".into()
}

fn default_target_lang() -> String {
    "zh,en".into()
}

const fn default_background_noise() -> f32 {
    0.30
}
const fn default_pause_tolerance() -> f32 {
    0.40
}

const fn default_true() -> bool {
    true
}

fn default_server_url() -> String {
    "ws://127.0.0.1:7654/ws".into()
}

fn default_floating_max_count() -> usize {
    5
}

fn default_floating_font_size() -> f64 {
    14.0
}

impl Default for ClientSettings {
    fn default() -> Self {
        Self {
            capture_source: CaptureSource::Microphone,
            selected_device_id: String::new(),
            selected_loopback_device_id: String::new(),
            selected_tts_output_device_id: String::new(),
            background_noise: default_background_noise(),
            pause_tolerance: default_pause_tolerance(),
            continuous_recognition: false,
            microphone_recognition: RecognitionSettings::default(),
            loopback_recognition: RecognitionSettings::default(),
            source_lang: default_source_lang(),
            target_lang: default_target_lang(),
            denoise_enabled: true,
            tts_enabled: false,
            mute_self_pauses_translation: false,
            ui_language: UiLanguage::default(),
            ui_theme: UiTheme::default(),
            first_run: true,
            server_url: default_server_url(),
            download_proxy_url: String::new(),
            update_channel: UpdateChannel::Stable,
            osc_settings: OscSettings::from_project_config(),
            plugin_preferences: PluginPreferences::default(),
            active_page: Page::default(),
            sidebar_collapsed: false,
            floating_subtitles_enabled: false,
            floating_subtitles_max_count: default_floating_max_count(),
            floating_subtitles_font_size: default_floating_font_size(),
            prompt_library: PromptTemplateLibrary::default(),
        }
    }
}

impl ClientSettings {
    pub fn load(project_root: &Path) -> Self {
        let runtime_dir = project_root.join("runtime");
        let settings_path = runtime_dir.join("rust-client-settings.json");
        let mut settings = std::fs::read_to_string(&settings_path)
            .ok()
            .and_then(|contents| serde_json::from_str::<ClientSettings>(&contents).ok())
            .unwrap_or_default();
        settings.prompt_library = PromptTemplateLibrary::load_from_dir(&runtime_dir);
        settings.migrate_recognition_settings();
        // Keep lifecycle state authoritative across development and packaged launches.
        settings.apply_app_state(project_root);
        settings.normalize_feature_dependencies();
        let registry = PluginRegistry::builtin();
        registry.initialize_preferences(&mut settings.plugin_preferences);
        registry.normalize_active_page(&settings.plugin_preferences, &mut settings.active_page);
        settings
    }

    fn apply_app_state(&mut self, project_root: &Path) {
        #[derive(Deserialize)]
        struct AppState {
            first_run: Option<bool>,
            ui_language: Option<UiLanguage>,
        }
        let path = project_root.join("runtime").join("app_state.json");
        let Ok(contents) = std::fs::read_to_string(path) else {
            return;
        };
        let Ok(state) = serde_json::from_str::<AppState>(&contents) else {
            return;
        };
        if let Some(first_run) = state.first_run {
            self.first_run = first_run;
        }
        if let Some(ui_language) = state.ui_language {
            self.ui_language = ui_language;
        }
    }

    fn migrate_recognition_settings(&mut self) {
        let defaults = RecognitionSettings::default();
        if self.microphone_recognition == defaults && self.loopback_recognition == defaults {
            let legacy = RecognitionSettings {
                background_noise: self.background_noise,
                pause_tolerance: self.pause_tolerance,
                continuous_recognition: self.continuous_recognition,
            };
            self.microphone_recognition = legacy.clone();
            self.loopback_recognition = legacy;
        }
    }

    /// Normalizes settings that share one user-facing feature.
    pub fn normalize_feature_dependencies(&mut self) {
        self.background_noise = self.background_noise.clamp(0.2, 0.8);
        self.pause_tolerance = self.pause_tolerance.clamp(0.0, 1.0);
        for settings in [
            &mut self.microphone_recognition,
            &mut self.loopback_recognition,
        ] {
            settings.background_noise = settings.background_noise.clamp(0.2, 0.8);
            settings.pause_tolerance = settings.pause_tolerance.clamp(0.0, 1.0);
        }
        // Persisted preferences remain subject to feature availability.
        self.tts_enabled &=
            crate::feature_access::is_available(crate::feature_access::Feature::TtsPlayback);
        self.floating_subtitles_enabled &=
            crate::feature_access::is_available(crate::feature_access::Feature::FloatingSubtitles);
        self.osc_settings.enabled &=
            crate::feature_access::is_available(crate::feature_access::Feature::OscChatbox);
        self.osc_settings.show_speaker_number &=
            crate::feature_access::is_available(crate::feature_access::Feature::SpeakerNumbers);
        // Replace the old emoji defaults that render as tofu on systems without
        // an emoji-capable egui font, while preserving user-customized prefixes.
        for (prefix, legacy, replacement) in [
            (&mut self.osc_settings.microphone_prefix, "🎙️ ", "MIC "),
            (&mut self.osc_settings.system_audio_prefix, "🔊 ", "SYS "),
            (&mut self.osc_settings.typing_prefix, "⌨️ ", "TXT "),
        ] {
            if prefix == legacy {
                *prefix = replacement.into();
            }
        }
        self.mute_self_pauses_translation &=
            crate::feature_access::is_available(crate::feature_access::Feature::MuteSync);
    }

    pub fn sanitize_devices(
        &mut self,
        available_mics: &[InputDevice],
        available_loopbacks: &[InputDevice],
    ) {
        self.osc_settings.history_ttl_seconds =
            self.osc_settings.history_ttl_seconds.clamp(10.0, 20.0);
        self.floating_subtitles_max_count = self.floating_subtitles_max_count.clamp(1, 10);
        self.floating_subtitles_font_size = self.floating_subtitles_font_size.clamp(10.0, 24.0);

        if !self.selected_device_id.is_empty()
            && !available_mics
                .iter()
                .any(|d| d.id == self.selected_device_id)
        {
            log::warn!(
                "Saved microphone ID '{}' is no longer available. Falling back to default.",
                self.selected_device_id
            );
            self.selected_device_id.clear();
        }

        if !self.selected_loopback_device_id.is_empty()
            && !available_loopbacks
                .iter()
                .any(|d| d.id == self.selected_loopback_device_id)
        {
            log::warn!(
                "Saved loopback device ID '{}' is no longer available. Falling back to default.",
                self.selected_loopback_device_id
            );
            self.selected_loopback_device_id.clear();
        }

        // Loopback is a host capability. Do not leave a persisted session in a
        // source mode that this host cannot provide (Linux currently exposes
        // microphone capture but no system-audio loopback).
        if available_loopbacks.is_empty()
            && matches!(
                self.capture_source,
                CaptureSource::SystemAudio | CaptureSource::Both
            )
        {
            if !available_mics.is_empty() || self.selected_device_id.is_empty() {
                log::warn!(
                    "System-audio loopback is unavailable; falling back to microphone capture."
                );
                self.capture_source = CaptureSource::Microphone;
                self.selected_loopback_device_id.clear();
            }
        }
    }

    pub fn save(&self, project_root: &Path) -> Result<(), String> {
        let directory = project_root.join("runtime");
        let _ = std::fs::create_dir_all(&directory);
        let path = directory.join("rust-client-settings.json");
        let mut normalized = self.clone();
        let registry = PluginRegistry::builtin();
        registry.initialize_preferences(&mut normalized.plugin_preferences);
        registry.normalize_active_page(&normalized.plugin_preferences, &mut normalized.active_page);
        let contents = serde_json::to_string_pretty(&normalized).map_err(|e| e.to_string())?;
        std::fs::write(&path, format!("{contents}\n")).map_err(|e| e.to_string())?;

        self.prompt_library.save_to_dir(&directory)?;

        let app_state_path = directory.join("app_state.json");
        let mut app_state = std::fs::read_to_string(&app_state_path)
            .ok()
            .and_then(|contents| serde_json::from_str::<serde_json::Value>(&contents).ok())
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();
        app_state.insert("first_run".into(), serde_json::Value::Bool(self.first_run));
        app_state.insert(
            "ui_language".into(),
            serde_json::to_value(self.ui_language).map_err(|e| e.to_string())?,
        );
        let contents = serde_json::to_string_pretty(&app_state).map_err(|e| e.to_string())?;
        std::fs::write(app_state_path, format!("{contents}\n")).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognition_defaults_prioritize_speech() {
        let recognition = RecognitionSettings::default();
        assert_eq!(recognition.background_noise, 0.30);
        assert_eq!(recognition.pause_tolerance, 0.40);
        assert!(!recognition.continuous_recognition);
    }

    #[test]
    fn test_client_settings_load_save_and_sanitize() {
        let root = std::env::temp_dir().join("xrtranslate_test_settings");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::create_dir_all(&root);

        let settings = ClientSettings {
            capture_source: CaptureSource::SystemAudio,
            selected_device_id: "mic-1".into(),
            selected_loopback_device_id: "loopback-1".into(),
            tts_enabled: true,
            source_lang: "en".into(),
            download_proxy_url: "socks5://127.0.0.1:1080".into(),
            update_channel: UpdateChannel::Beta,
            sidebar_collapsed: true,
            active_page: Page::Plugin(PluginId::OSC),
            osc_settings: OscSettings {
                show_speaker_number: true,
                message_separator: crate::plugins::osc::runtime::OscMessageSeparator::NewLine,
                ..OscSettings::default()
            },
            ..ClientSettings::default()
        };

        settings.save(&root).unwrap();

        let mut loaded = ClientSettings::load(&root);
        assert_eq!(loaded.capture_source, CaptureSource::SystemAudio);
        assert_eq!(loaded.selected_device_id, "mic-1");
        assert_eq!(loaded.selected_loopback_device_id, "loopback-1");
        assert!(loaded.tts_enabled);
        assert_eq!(loaded.source_lang, "en");
        assert_eq!(loaded.download_proxy_url, "socks5://127.0.0.1:1080");
        assert_eq!(loaded.update_channel, UpdateChannel::Beta);
        assert!(loaded.sidebar_collapsed);
        assert_eq!(loaded.active_page, Page::Plugin(PluginId::OSC));
        assert!(loaded.osc_settings.show_speaker_number);
        assert_eq!(
            loaded.osc_settings.message_separator,
            crate::plugins::osc::runtime::OscMessageSeparator::NewLine
        );

        // Test sanitization with missing device
        let available_mics = vec![InputDevice {
            id: "mic-2".into(),
            name: "Other Mic".into(),
        }];
        let available_loopbacks = vec![InputDevice {
            id: "loopback-1".into(),
            name: "Loopback 1".into(),
        }];

        loaded.sanitize_devices(&available_mics, &available_loopbacks);
        assert_eq!(loaded.selected_device_id, ""); // Reset due to mic-1 missing
        assert_eq!(loaded.selected_loopback_device_id, "loopback-1"); // Kept

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn prompt_studio_user_profile_survives_save_and_reload() {
        let root = std::env::temp_dir().join("xrtranslate_test_prompt_profile_persistence");
        let _ = std::fs::remove_dir_all(&root);

        let mut settings = ClientSettings::default();
        let mut profile = PromptTemplateLibrary::editable_copy_of(
            &settings.prompt_library.profiles[0],
            "user-profile-1",
        );
        profile.name = "My saved project".into();
        profile.description = "User-authored prompt graph".into();
        settings.prompt_library.profiles.push(profile);
        settings.prompt_library.active_id = "user-profile-1".into();

        settings.save(&root).unwrap();

        assert!(
            root.join("runtime")
                .join(PromptTemplateLibrary::FILE_NAME)
                .exists()
        );
        let settings_json =
            std::fs::read_to_string(root.join("runtime/rust-client-settings.json")).unwrap();
        assert!(!settings_json.contains("prompt_library"));
        assert!(!settings_json.contains("user-profile-1"));

        let prompt_studio_json =
            std::fs::read_to_string(root.join("runtime").join(PromptTemplateLibrary::FILE_NAME))
                .unwrap();
        assert!(prompt_studio_json.contains("user-profile-1"));
        assert!(prompt_studio_json.contains("My saved project"));

        let loaded = ClientSettings::load(&root);
        let loaded_profile = loaded
            .prompt_library
            .profiles
            .iter()
            .find(|profile| profile.id == "user-profile-1")
            .expect("user profile must be persisted");

        assert_eq!(loaded.prompt_library.active_id, "user-profile-1");
        assert_eq!(loaded_profile.name, "My saved project");
        assert_eq!(loaded_profile.description, "User-authored prompt graph");
        assert!(!loaded_profile.read_only);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn osc_speaker_number_visibility_is_independent() {
        let root = std::env::temp_dir().join("xrtranslate_test_speaker_number_visibility");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("runtime")).unwrap();
        let settings = ClientSettings {
            osc_settings: OscSettings {
                show_speaker_number: true,
                ..OscSettings::default()
            },
            ..ClientSettings::default()
        };
        settings.save(&root).unwrap();

        let loaded = ClientSettings::load(&root);
        assert!(loaded.osc_settings.show_speaker_number);
        let serialized = serde_json::to_value(&loaded).unwrap();
        assert!(serialized.get("speaker_recognition_enabled").is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_emoji_prefix_defaults_migrate_without_overwriting_custom_text() {
        let mut settings = ClientSettings::default();
        settings.osc_settings.microphone_prefix = "🎙️ ".into();
        settings.osc_settings.system_audio_prefix = "game: ".into();
        settings.osc_settings.typing_prefix = "⌨️ ".into();

        settings.normalize_feature_dependencies();

        assert_eq!(settings.osc_settings.microphone_prefix, "MIC ");
        assert_eq!(settings.osc_settings.system_audio_prefix, "game: ");
        assert_eq!(settings.osc_settings.typing_prefix, "TXT ");
    }

    #[test]
    fn available_tts_preference_is_preserved_on_load() {
        let root = std::env::temp_dir().join("xrtranslate_test_feature_access");
        let _ = std::fs::remove_dir_all(&root);
        let settings = ClientSettings {
            tts_enabled: true,
            ..ClientSettings::default()
        };
        settings.save(&root).unwrap();

        assert!(ClientSettings::load(&root).tts_enabled);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn disabled_plugin_page_falls_back_to_translation() {
        let root = std::env::temp_dir().join("xrtranslate_test_disabled_plugin_route");
        let _ = std::fs::remove_dir_all(&root);
        let mut preferences = PluginPreferences::default();
        preferences.set_enabled(PluginId::MEETING, false);
        let settings = ClientSettings {
            active_page: Page::Plugin(PluginId::MEETING),
            plugin_preferences: preferences,
            ..ClientSettings::default()
        };
        settings.save(&root).unwrap();

        assert_eq!(ClientSettings::load(&root).active_page, Page::Translation);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn app_state_can_reset_onboarding_after_client_settings_exist() {
        let root = std::env::temp_dir().join("xrtranslate_test_first_run_reset");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("runtime")).unwrap();

        let mut settings = ClientSettings {
            first_run: false,
            ..ClientSettings::default()
        };
        settings.save(&root).unwrap();
        std::fs::write(
            root.join("runtime/app_state.json"),
            r#"{"first_run":true,"ui_language":"english"}"#,
        )
        .unwrap();

        assert!(ClientSettings::load(&root).first_run);
        settings = ClientSettings::load(&root);
        settings.first_run = false;
        settings.save(&root).unwrap();
        let state: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("runtime/app_state.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(state["first_run"], false);

        let _ = std::fs::remove_dir_all(root);
    }
}
