mod chatbox;
pub mod runtime;
mod sys_info;
pub mod ui;

use eframe::egui;
use std::sync::{Arc, atomic::AtomicBool};

use runtime::{OscHandle, OscManager, OscSettings};
pub use ui::{OscPageContext, OscUiAction};

use crate::session_coordinator::{CaptionUpdate, HostOutputEvent, HostOutputSubscriber};

impl HostOutputSubscriber for OscHandle {
    fn on_host_output(&self, event: HostOutputEvent<'_>) {
        match event {
            HostOutputEvent::Caption {
                stream_id,
                audio_source,
                is_typing,
                source,
                translated,
                speaker,
                update: CaptionUpdate::RollOver,
            } => self.roll_stream_for(
                stream_id,
                audio_source,
                is_typing,
                source,
                translated,
                speaker,
            ),
            HostOutputEvent::Caption {
                stream_id,
                audio_source,
                is_typing,
                source,
                translated,
                speaker,
                update,
            } => self.add_message_for_stream(
                stream_id,
                audio_source,
                is_typing,
                source,
                translated,
                speaker,
                matches!(update, CaptionUpdate::Replace),
            ),
            HostOutputEvent::StreamEnded(stream_id) => self.end_stream_for(stream_id),
            HostOutputEvent::Clear => self.clear_chatbox(),
        }
    }
}

/// Owns all OSC state and runtime resources behind the desktop plugin boundary.
///
/// `host_enabled` controls whether the plugin may run at all, while
/// `draft.enabled` retains the user's OSC-output preference across sidebar
/// activation changes.
pub struct OscPlugin {
    manager: OscManager,
    draft: OscSettings,
    draft_input: String,
    translate_input: bool,
    host_enabled: bool,
}

impl OscPlugin {
    pub fn new(draft: OscSettings, host_enabled: bool) -> Self {
        let manager = OscManager::new(effective_settings(&draft, host_enabled));
        Self {
            manager,
            draft,
            draft_input: String::new(),
            translate_input: true,
            host_enabled,
        }
    }

    pub fn manager(&self) -> &OscManager {
        &self.manager
    }

    pub fn draft(&self) -> &OscSettings {
        &self.draft
    }

    pub fn draft_mut(&mut self) -> &mut OscSettings {
        &mut self.draft
    }

    pub fn draft_input(&self) -> &str {
        &self.draft_input
    }

    pub fn draft_input_mut(&mut self) -> &mut String {
        &mut self.draft_input
    }

    pub fn translate_input(&self) -> bool {
        self.translate_input
    }

    pub fn set_translate_input(&mut self, enabled: bool) {
        self.translate_input = enabled;
    }

    pub fn send_manual_message(&mut self, text: &str) {
        self.manager.send_manual_message(text);
    }

    pub fn apply_draft(&mut self) -> Result<(), String> {
        self.manager
            .update_settings(effective_settings(&self.draft, self.host_enabled))
    }

    pub fn activate(&mut self) -> Result<(), String> {
        self.host_enabled = true;
        self.apply_draft()
    }

    pub fn deactivate(&mut self) -> Result<(), String> {
        self.manager.clear_chatbox();
        self.host_enabled = false;
        self.apply_draft()
    }

    pub fn publisher(&self) -> OscHandle {
        self.manager.handle()
    }

    pub fn mute_state(&self) -> Arc<AtomicBool> {
        self.manager.muted_state()
    }

    pub fn clear_chatbox(&self) {
        self.manager.clear_chatbox();
    }

    pub fn render_page(
        &mut self,
        ui: &mut egui::Ui,
        context: OscPageContext<'_>,
    ) -> Vec<OscUiAction> {
        ui::render(self, ui, context)
    }

    pub fn render_settings(
        &mut self,
        ui: &mut egui::Ui,
        language: crate::i18n::UiLanguage,
    ) -> Vec<OscUiAction> {
        ui::render_settings(self, ui, language)
    }
}

fn effective_settings(draft: &OscSettings, host_enabled: bool) -> OscSettings {
    let mut settings = draft.clone();
    settings.enabled &= host_enabled;
    settings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_activation_does_not_overwrite_the_user_output_preference() {
        let draft = OscSettings {
            enabled: true,
            listen_port: 0,
            ..OscSettings::default()
        };
        let mut plugin = OscPlugin::new(draft, false);

        assert!(!plugin.host_enabled);
        assert!(plugin.draft().enabled);
        plugin.activate().unwrap();
        plugin.deactivate().unwrap();
        assert!(plugin.draft().enabled);
    }

    #[test]
    fn translate_input_defaults_to_true_and_can_be_toggled() {
        let mut plugin = OscPlugin::new(OscSettings::default(), true);
        assert!(plugin.translate_input());
        plugin.set_translate_input(false);
        assert!(!plugin.translate_input());
        plugin.set_translate_input(true);
        assert!(plugin.translate_input());
    }

    #[test]
    fn typing_language_direction_defaults_to_zh_to_en_and_can_be_changed() {
        let mut plugin = OscPlugin::new(OscSettings::default(), true);
        assert_eq!(plugin.draft().typing_source_lang, "zh");
        assert_eq!(plugin.draft().typing_target_lang, "en");

        plugin.draft_mut().typing_source_lang = "ja".into();
        plugin.draft_mut().typing_target_lang = "zh".into();
        assert_eq!(plugin.draft().typing_source_lang, "ja");
        assert_eq!(plugin.draft().typing_target_lang, "zh");
    }
}
