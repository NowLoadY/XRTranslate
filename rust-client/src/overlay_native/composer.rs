use crate::{
    i18n::UiLanguage,
    overlay_ipc::{ComposerState, OverlayEvent},
};
use eframe::egui;
use xrtranslate_client_core::{
    services::TextAction as HostAction,
    ui::components::text_composer::{ComposerContext, TextAction, TextComposer},
};

#[derive(Default)]
pub(super) struct Composer {
    state: Option<ComposerState>,
    input: TextComposer,
}

impl Composer {
    pub fn update(&mut self, state: ComposerState) {
        self.state = Some(state);
    }

    pub fn show(&mut self, ui: &mut egui::Ui, language: UiLanguage) -> Option<egui::Rect> {
        let state = self.state.as_mut()?;
        let previous = (state.source_language.clone(), state.target_language.clone());
        let response = ui.scope(|ui| {
            self.input.render(
                ui,
                "floating_text_composer",
                (&mut state.source_language, &mut state.target_language),
                ComposerContext {
                    language,
                    capabilities: state.languages,
                    preparing: state.text.preparing,
                    input_budget: state.text.input_budget,
                    can_copy: state.text.can_copy,
                    recognition: state.text.recognition.as_ref(),
                    translation_enabled: true,
                    allow_direct: false,
                    direct_enabled: false,
                    show_languages: true,
                },
            )
        });
        if previous.0 != state.source_language || previous.1 != state.target_language {
            super::send_event(OverlayEvent::Text(HostAction::Language {
                source: state.source_language.clone(),
                target: state.target_language.clone(),
                asr_only: state.text.asr_only,
            }));
        }
        let action = match response.inner {
            Some(TextAction::ContextSettings) => Some(HostAction::ContextSettings),
            Some(TextAction::Translate { text, .. }) => Some(HostAction::Submit { text }),
            Some(TextAction::CollectRecognition { enabled, text }) => {
                Some(HostAction::CollectRecognition { enabled, text })
            }
            Some(TextAction::Copy) => Some(HostAction::Copy),
            Some(TextAction::CopyRecognition) => Some(HostAction::CopyRecognition),
            Some(TextAction::DismissCopy) => Some(HostAction::DismissCopy),
            _ => None,
        };
        if let Some(action) = action {
            super::send_event(OverlayEvent::Text(action));
        }
        Some(response.response.rect)
    }
}
