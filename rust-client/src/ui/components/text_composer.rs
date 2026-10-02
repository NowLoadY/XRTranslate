//! Shared draft editing and submission intent; translation and delivery belong to the host.
use crate::i18n::{UiLanguage, tr};
use eframe::egui;
use xrtranslate_engine::language::LanguageCapabilities;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum TextMode {
    #[default]
    Translate,
    Direct,
}

#[derive(Default)]
pub struct TextComposer {
    pub text: String,
    pub mode: TextMode,
    composing: bool,
}

pub struct ComposerContext {
    pub language: UiLanguage,
    pub capabilities: LanguageCapabilities,
    pub preparing: bool,
    pub allow_direct: bool,
    pub direct_enabled: bool,
    pub show_languages: bool,
}

pub enum TextAction {
    Translate {
        text: String,
        source_lang: String,
        target_lang: String,
    },
    Direct(String),
}

impl TextComposer {
    fn submission(&self, source: &str, target: &str) -> Option<TextAction> {
        let text = self.text.trim();
        if text.is_empty() {
            return None;
        }
        if self.mode == TextMode::Direct {
            return Some(TextAction::Direct(text.to_owned()));
        }
        Some(TextAction::Translate {
            text: text.to_owned(),
            source_lang: source.to_owned(),
            target_lang: target.to_owned(),
        })
    }

    pub fn render(
        &mut self,
        ui: &mut egui::Ui,
        id: &str,
        languages: (&mut String, &mut String),
        context: ComposerContext,
    ) -> Option<TextAction> {
        if !context.allow_direct {
            self.mode = TextMode::Translate;
        }
        let editor_id = ui.make_persistent_id((id, "editor"));
        let focused = ui.memory(|memory| memory.has_focus(editor_id));
        let was_composing = self.composing;
        let ime_event = ui.input(|input| {
            let mut changed = false;
            if focused {
                for event in &input.events {
                    match event {
                        egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) => {
                            self.composing = !text.is_empty();
                            changed = true;
                        }
                        egui::Event::Ime(egui::ImeEvent::Commit(_)) => {
                            self.composing = false;
                            changed = true;
                        }
                        _ => {}
                    }
                }
            } else {
                self.composing = false;
            }
            changed
        });
        let enter = focused
            && !was_composing
            && !self.composing
            && !ime_event
            && ui.input_mut(|input| {
                let mut enter = false;
                input.events.retain(|event| {
                    if let egui::Event::Key {
                        key: egui::Key::Enter,
                        pressed: true,
                        repeat,
                        modifiers,
                        ..
                    } = event
                        && *modifiers == egui::Modifiers::NONE
                    {
                        enter |= !repeat;
                        false
                    } else {
                        true
                    }
                });
                enter
            });
        let mut submit = false;
        let compact = ui.available_width() < 600.0;
        let radius = if compact { 22 } else { 10 };
        crate::ui::organic_border::show(
            ui,
            editor_id.with("island"),
            egui::Frame::new()
                .fill(if compact {
                    crate::ui::theme::surface_control()
                } else {
                    egui::Color32::TRANSPARENT
                })
                .corner_radius(egui::CornerRadius::same(radius))
                .inner_margin(egui::Margin::symmetric(12, 6)),
            radius as f32,
            crate::ui::theme::border(),
            |ui| {
                if context.allow_direct {
                    ui.horizontal_wrapped(|ui| {
                        for (mode, label) in [
                            (TextMode::Translate, "Translate"),
                            (TextMode::Direct, "Direct"),
                        ] {
                            let label = tr(context.language, label);
                            let response = ui.selectable_label(self.mode == mode, label);
                            let response = crate::ui::automation::button_response(
                                ui,
                                response.id,
                                label,
                                response.enabled(),
                                response,
                            );
                            if response.clicked() {
                                self.mode = mode;
                            }
                        }
                    });
                }
                if context.show_languages && self.mode == TextMode::Translate {
                    super::translation_language_selector(
                        ui,
                        id,
                        languages.0,
                        languages.1,
                        context.capabilities,
                        context.language,
                    );
                }
                let waiting = self.mode == TextMode::Translate && context.preparing;
                let available = !waiting
                    && !self.composing
                    && !ime_event
                    && (self.mode == TextMode::Translate || context.direct_enabled);
                let mut clicked = false;
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if context.allow_direct {
                        "Send"
                    } else {
                        "Translate"
                    };
                    clicked = super::primary_button_enabled(
                        ui,
                        tr(context.language, label),
                        available && !self.text.trim().is_empty(),
                    )
                    .clicked();
                    if waiting {
                        ui.spinner();
                    }
                    egui::ScrollArea::vertical()
                        .id_salt((id, "draft_scroll"))
                        .max_height(84.0)
                        .show(ui, |ui| {
                            let hint = if self.mode == TextMode::Translate {
                                "Type text to translate…"
                            } else {
                                "Temporary Message"
                            };
                            let response = ui.add(
                                egui::TextEdit::multiline(&mut self.text)
                                    .id(editor_id)
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(1)
                                    .font(egui::FontId::proportional(13.5))
                                    .frame(egui::Frame::NONE)
                                    .margin(egui::Margin::symmetric(0, 2))
                                    .hint_text(tr(context.language, hint)),
                            );
                            if let Some(text) = crate::ui::automation::record_text_input(
                                ui,
                                editor_id,
                                id,
                                &self.text,
                                response.enabled(),
                                response.rect,
                            ) {
                                self.text = text;
                            }
                            response.on_hover_text(tr(
                                context.language,
                                "Enter to send · Shift+Enter for a new line",
                            ));
                        });
                });
                submit = (clicked || enter) && available && !self.text.trim().is_empty();
            },
        );
        if submit {
            ui.memory_mut(|memory| memory.request_focus(editor_id));
            self.submission(languages.0, languages.1)
        } else {
            None
        }
    }
}
