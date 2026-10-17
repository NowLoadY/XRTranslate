//! Android Back unwinds visible UI before returning through the main pages.
use crate::{
    XRTranslateApp,
    plugins::PluginId,
    ui::{self, Page},
};
use eframe::egui;

impl XRTranslateApp {
    pub(crate) fn handle_android_back(&mut self, ctx: &egui::Context) {
        if !super::take_back_request() {
            return;
        }
        // Let the existing topmost dialog/menu handle cancellation and its draft cleanup.
        if egui::Popup::is_any_open(ctx) || ctx.memory(|memory| memory.top_modal_layer().is_some())
        {
            ctx.input_mut(|input| {
                input.events.push(egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                })
            });
            return;
        }
        if self.first_run {
            self.onboarding_page = self.onboarding_page.saturating_sub(1);
            return;
        }
        match self.navigation.page {
            Page::Translation if self.history_archive.open => {
                self.history_archive.cancel_delete();
                self.history_archive.open = false;
                return;
            }
            Page::Translation if self.fullscreen_history => {
                self.fullscreen_history = false;
                return;
            }
            Page::Plugin(PluginId::VOCABULARY) if self.vocabulary_plugin.handle_back() => return,
            Page::Plugin(PluginId::MEETING) if self.meeting_plugin.controller.handle_back() => {
                return;
            }
            Page::Plugin(PluginId::MEDIA) => {
                if let Some(action) = self.media_plugin.controller.handle_back(ctx) {
                    self.apply_media_action(action, ctx.clone());
                    return;
                }
            }
            Page::PromptStudio => {
                if let Some(actions) = ui::pages::prompt_studio::handle_back(
                    ctx,
                    &mut self.prompt_studio,
                    &self.prompt_library,
                ) {
                    self.apply_prompt_studio_actions(actions);
                    return;
                }
            }
            Page::TtsCenter if self.tts_center.draft.is_some() => {
                self.tts_center.draft = None;
                return;
            }
            _ => {}
        }
        self.navigation.back();
        self.save_settings();
    }
}
