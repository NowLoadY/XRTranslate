mod chatbox;
pub mod runtime;
mod sys_info;
pub mod ui;

use eframe::egui;
use std::sync::{Arc, atomic::AtomicBool};

use runtime::{OscHandle, OscManager, OscSettings};
pub use ui::{OscPageContext, OscUiAction};

use crate::session_coordinator::{CaptionUpdate, HostOutputEvent, HostOutputSubscriber};
use crate::ui::components::text_composer::TextComposer;

impl HostOutputSubscriber for OscHandle {
    fn on_host_output(&self, event: HostOutputEvent<'_>) {
        match event {
            HostOutputEvent::CommittedTranslation { .. } | HostOutputEvent::StreamCancelled(_) => {}
            HostOutputEvent::Caption {
                stream_id,
                audio_source,
                is_typing,
                source,
                translated,
                additional_translations,
                asr_only,
                speaker,
                update,
            } => self.add_message_for_stream(
                stream_id,
                audio_source,
                is_typing,
                if asr_only { "" } else { source },
                translated,
                additional_translations,
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
    composer: TextComposer,
    host_enabled: bool,
}

impl OscPlugin {
    pub fn new(draft: OscSettings, host_enabled: bool) -> Self {
        let manager = OscManager::new(effective_settings(&draft, host_enabled));
        Self {
            manager,
            draft,
            composer: TextComposer::default(),
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

    pub fn draft_input_mut(&mut self) -> &mut String {
        &mut self.composer.text
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
