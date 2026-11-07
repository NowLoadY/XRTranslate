mod chatbox;
mod game;
pub use game::OscGame;
pub mod runtime;
pub mod ui;

use crate::text_output::TextOutputHandle;
use eframe::egui;

use runtime::{OscHandle, OscManager, OscSettings};
pub use ui::{OscPageContext, OscUiAction};

use crate::session_coordinator::{CaptionUpdate, HostOutputEvent, HostOutputSubscriber};
use crate::ui::components::text_composer::TextComposer;

impl HostOutputSubscriber for OscHandle {
    fn on_host_output(&self, event: HostOutputEvent<'_>) {
        match event {
            HostOutputEvent::Previews {
                stream_id,
                is_typing,
                entries,
            } => self.set_previews(stream_id, is_typing, &entries),
            HostOutputEvent::CommittedTranslation { .. } => {}
            HostOutputEvent::StreamCancelled(stream_id) => self.cancel_stream(stream_id),
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
                ..
            } => self.add_message_for_stream(
                stream_id,
                audio_source,
                is_typing,
                if asr_only { "" } else { &source },
                &translated,
                &additional_translations,
                &speaker,
                matches!(update, CaptionUpdate::Replace),
            ),
            HostOutputEvent::StreamEnded(stream_id) => self.end_stream_for(stream_id),
            HostOutputEvent::Clear => self.clear_chatbox(),
        }
    }
}

/// One game's Chatbox state, composition and writer. An optional shared receiver
/// supplies game controls independently of the writer's lifetime.
pub struct OscPlugin {
    manager: OscManager,
    draft: OscSettings,
    applied: OscSettings,
    composer: TextComposer,
    pub receiver: Option<xrtranslate_client_core::osc::ReceiverStatus>,
}

impl OscPlugin {
    pub fn new(
        game: OscGame,
        mut draft: OscSettings,
        text_output: TextOutputHandle,
        client: xrtranslate_plugin::Client,
    ) -> Self {
        draft.game = game;
        draft.max_text_length = draft.text_limit().max(1);
        let mut applied = draft.clone();
        applied.enabled = false;
        let manager = OscManager::new(applied.clone(), text_output, client);
        Self {
            manager,
            draft,
            applied,
            composer: TextComposer::default(),
            receiver: (game == OscGame::VRChat).then(Default::default),
        }
    }

    pub fn game(&self) -> OscGame {
        self.draft.game
    }

    pub fn applied(&self) -> &OscSettings {
        &self.applied
    }

    pub fn disable_output(&mut self) -> Result<(), String> {
        let mut settings = self.applied.clone();
        settings.enabled = false;
        self.apply_settings(settings)?;
        self.draft.enabled = false;
        Ok(())
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

    /// Review suppresses automatic microphone bodies until explicitly sent.
    /// Switching either way discards old microphone output, including previews.
    pub(crate) fn set_microphone_review(&self, enabled: bool) {
        self.manager.handle().set_microphone_review(enabled);
    }

    /// Produces the exact voice body shown before an explicit publication.
    /// Decorations and automatic captions never consume this body's budget.
    pub(crate) fn prepare_voice_text(&self, text: &str) -> Result<String, String> {
        self.applied.prepare_voice_text(text)
    }

    pub(crate) fn publish_voice(&self, operation: u64, text: &str) -> runtime::VoicePublishReply {
        self.manager.handle().publish_voice(operation, text)
    }

    pub fn apply_draft(&mut self) -> Result<(), String> {
        let settings = self.draft.clone();
        self.apply_settings(settings)
    }

    fn apply_settings(&mut self, settings: OscSettings) -> Result<(), String> {
        self.manager.update_settings(settings.clone())?;
        self.applied = settings;
        Ok(())
    }

    pub fn publisher(&self) -> OscHandle {
        self.manager.handle()
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
}
