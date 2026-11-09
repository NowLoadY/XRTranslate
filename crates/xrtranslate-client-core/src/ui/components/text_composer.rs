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
    submit_key: SubmitShortcut,
    recognition_revision: u64,
}

/// Enter submission shared by text entry surfaces, including IME confirmation.
#[derive(Clone, Default)]
pub struct SubmitShortcut {
    composing: bool,
}

pub struct ComposerContext<'a> {
    pub language: UiLanguage,
    pub capabilities: LanguageCapabilities,
    pub preparing: bool,
    pub input_budget: Option<usize>,
    pub can_copy: bool,
    pub recognition: Option<&'a crate::services::RecognitionInput>,
    pub translation_enabled: bool,
    pub allow_direct: bool,
    pub direct_enabled: bool,
    pub show_languages: bool,
}

pub enum TextAction {
    Copy,
    DismissCopy,
    CopyRecognition,
    CollectRecognition {
        enabled: bool,
        text: String,
    },
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
        context: ComposerContext<'_>,
    ) -> Option<TextAction> {
        if !context.allow_direct {
            self.mode = TextMode::Translate;
        }
        if self.mode == TextMode::Translate
            && let Some(input) = context.recognition
            && input.revision != self.recognition_revision
        {
            self.text.clone_from(&input.text);
            self.recognition_revision = input.revision;
        }
        let collecting = context.recognition.is_some_and(|input| input.active);
        let mut toggle_collection = false;
        let mut copy_recognition = false;
        let mut edited = false;
        let editor_id = ui.make_persistent_id((id, "editor"));
        let enter = self.submit_key.pressed(ui.ctx(), editor_id);
        let mut copy = false;
        let mut submit = false;
        let compact = ui.available_width() < 600.0;
        let radius = if compact { 22 } else { 10 };
        crate::ui::organic_border::show(
            ui,
            editor_id.with("island"),
            egui::Frame::new()
                .fill(if compact {
                    crate::ui::theme::content_backdrop(true)
                } else {
                    egui::Color32::TRANSPARENT
                })
                .corner_radius(egui::CornerRadius::same(radius))
                .inner_margin(egui::Margin::symmetric(12, 6)),
            radius as f32,
            crate::ui::theme::border(),
            |ui| {
                if self.mode == TextMode::Translate
                    && (context.input_budget.is_some() || context.can_copy)
                {
                    // Keep result actions inside the composer, clear of the editable text.
                    let diameter = 32.0_f32.max(ui.spacing().interact_size.y);
                    let height = if context.can_copy { diameter } else { 16.0 };
                    let (header, _) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), height),
                        egui::Sense::hover(),
                    );
                    if context.can_copy {
                        let mut action_ui = ui.new_child(
                            egui::UiBuilder::new()
                                .id_salt((id, "copy_translation"))
                                .max_rect(egui::Rect::from_center_size(
                                    header.center(),
                                    egui::Vec2::splat(diameter),
                                ))
                                .layout(egui::Layout::top_down(egui::Align::Center)),
                        );
                        copy = super::action_button::floating_copy_button(
                            &mut action_ui,
                            tr(context.language, "Copy translation"),
                        )
                        .clicked();
                    }
                    if let Some(budget) = context.input_budget {
                        let mut rect = header;
                        if context.can_copy {
                            rect.min.x = (header.center().x + diameter * 0.5 + 8.0).min(rect.max.x);
                        }
                        let mut indicator_ui = ui.new_child(
                            egui::UiBuilder::new()
                                .id_salt((id, "context_remaining"))
                                .max_rect(rect)
                                .layout(egui::Layout::right_to_left(egui::Align::Center)),
                        );
                        indicator_ui.set_clip_rect(rect.intersect(ui.clip_rect()));
                        context_indicator(&mut indicator_ui, &self.text, budget, context.language);
                    }
                }
                if context.allow_direct {
                    ui.horizontal_wrapped(|ui| {
                        for (mode, label) in [
                            (TextMode::Translate, "Translate"),
                            (TextMode::Direct, "Direct"),
                        ] {
                            let label = tr(context.language, label);
                            let response = ui
                                .add_enabled_ui(!collecting, |ui| {
                                    super::selectable_label(ui, self.mode == mode, label)
                                })
                                .inner;
                            if response.clicked() {
                                edited |= self.mode != mode;
                                self.mode = mode;
                            }
                        }
                    });
                }
                if context.show_languages && self.mode == TextMode::Translate {
                    edited |= super::translation_language_selector(
                        ui,
                        id,
                        languages.0,
                        languages.1,
                        context.capabilities,
                        context.language,
                    );
                }
                let waiting = self.mode == TextMode::Translate && context.preparing;
                // IME state guards the Enter shortcut, not an explicit click.
                let available = !waiting
                    && !collecting
                    && match self.mode {
                        TextMode::Translate => context.translation_enabled,
                        TextMode::Direct => context.direct_enabled,
                    };
                let mut clicked = false;
                ui.horizontal(|ui| {
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
                        if self.mode == TextMode::Translate
                            && let Some(input) = context.recognition
                        {
                            toggle_collection = ui
                                .add_enabled_ui(!waiting || collecting, |ui| {
                                    super::action_button::recognition_input_button(
                                        ui,
                                        tr(
                                            context.language,
                                            if collecting {
                                                "Finish collecting and translate"
                                            } else {
                                                "Collect recognized text"
                                            },
                                        ),
                                        collecting,
                                    )
                                })
                                .inner
                                .clicked();
                            if input.can_copy {
                                copy_recognition = super::copy_button(
                                    ui,
                                    tr(context.language, "Copy recognized text"),
                                )
                                .clicked();
                            }
                        }
                        egui::ScrollArea::vertical()
                            .id_salt((id, "draft_scroll"))
                            .max_height(84.0)
                            .stick_to_bottom(collecting)
                            .show(ui, |ui| {
                                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                                    let hint = if self.mode == TextMode::Translate {
                                        "Type text to translate…"
                                    } else {
                                        "Type a message…"
                                    };
                                    let response = ui.add_enabled(
                                        !collecting,
                                        egui::TextEdit::multiline(&mut self.text)
                                            .id(editor_id)
                                            .desired_width(f32::INFINITY)
                                            .desired_rows(1)
                                            .font(egui::FontId::proportional(13.5))
                                            .min_size(egui::vec2(0.0, 32.0))
                                            .align(egui::Align2::LEFT_CENTER)
                                            .frame(egui::Frame::NONE)
                                            .margin(egui::Margin::symmetric(0, 2))
                                            .hint_text(tr(context.language, hint)),
                                    );
                                    edited |= response.changed();
                                    if let Some(text) = ui_director::egui::record_text_input(
                                        ui,
                                        editor_id,
                                        id,
                                        &self.text,
                                        response.enabled(),
                                        response.rect,
                                    ) {
                                        edited |= self.text != text;
                                        self.text = text;
                                    }
                                    response.on_hover_text(tr(
                                        context.language,
                                        "Enter to send · Shift+Enter for a new line",
                                    ));
                                });
                            });
                    });
                });
                submit = (clicked || enter) && available && !self.text.trim().is_empty();
            },
        );
        if toggle_collection {
            Some(TextAction::CollectRecognition {
                enabled: !collecting,
                text: self.text.clone(),
            })
        } else if copy_recognition {
            Some(TextAction::CopyRecognition)
        } else if submit {
            self.submit_key = SubmitShortcut::default();
            ui.memory_mut(|memory| memory.request_focus(editor_id));
            self.submission(languages.0, languages.1)
        } else if edited
            && (context.can_copy || (context.preparing && context.recognition.is_some()))
        {
            Some(TextAction::DismissCopy)
        } else if copy {
            Some(TextAction::Copy)
        } else {
            None
        }
    }
}

fn context_indicator(ui: &mut egui::Ui, text: &str, budget: usize, language: UiLanguage) {
    let used = xrtranslate_prompt::estimated_tokens(text.trim());
    let exceeded = used > budget;
    let color = if exceeded {
        crate::ui::theme::danger()
    } else {
        crate::ui::theme::text_weak()
    };
    let remaining = budget as i64 - used as i64;
    let label = if ui.available_width() < 240.0 {
        format!("≈{remaining}")
    } else {
        format!("{} · ≈{remaining}", tr(language, "Context remaining"))
    };
    let response = ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
        let radius = 5.5;
        ui.painter().circle_stroke(
            rect.center(),
            radius,
            egui::Stroke::new(1.5, crate::ui::theme::border()),
        );
        let fraction = if exceeded {
            1.0
        } else {
            remaining as f32 / budget.max(1) as f32
        };
        if fraction > 0.0 {
            let points = (0..=32)
                .map(|step| {
                    let angle = -std::f32::consts::FRAC_PI_2
                        + std::f32::consts::TAU * fraction * step as f32 / 32.0;
                    rect.center() + egui::vec2(angle.cos(), angle.sin()) * radius
                })
                .collect();
            ui.painter()
                .add(egui::Shape::line(points, egui::Stroke::new(1.5, color)));
        }
        ui.label(egui::RichText::new(label).size(11.0).color(color));
    });
    response.response.on_hover_text(tr(
        language,
        "Estimated tokens available for one translation. Longer text is split automatically.",
    ));
}

impl SubmitShortcut {
    pub fn pressed(&mut self, ctx: &egui::Context, editor_id: egui::Id) -> bool {
        let focused = ctx.memory(|memory| memory.has_focus(editor_id));
        let was_composing = self.composing;
        let ime_event = ctx.input(|input| {
            let mut changed = false;
            if focused {
                for event in &input.events {
                    match event {
                        egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) => {
                            // Some platforms send empty preedit events while the
                            // editor is merely focused, without any composition.
                            changed |= self.composing || !text.is_empty();
                            self.composing = !text.is_empty();
                        }
                        egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
                            changed |= self.composing || !text.is_empty();
                            self.composing = false;
                        }
                        _ => {}
                    }
                }
            } else {
                self.composing = false;
            }
            changed
        });
        let ime_busy = was_composing || self.composing || ime_event;
        focused
            && ctx.input_mut(|input| {
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
                        // Confirming an IME candidate must neither submit the
                        // draft nor insert a newline into the committed text.
                        enter |= !repeat && !ime_busy;
                        false
                    } else {
                        true
                    }
                });
                enter
            })
    }
}
