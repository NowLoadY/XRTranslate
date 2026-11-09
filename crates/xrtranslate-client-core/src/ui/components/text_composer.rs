//! Shared draft editing and submission intent; translation and delivery belong to the host.
use crate::i18n::{UiLanguage, tr};
use crate::ui::{animation::AnimationSystem, theme};
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
    ContextSettings,
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
        let mut edited = false;
        let editor_id = ui.make_persistent_id((id, "editor"));
        let enter = self.submit_key.pressed(ui.ctx(), editor_id);
        let focused = ui.memory(|memory| memory.has_focus(editor_id));
        let expansion = AnimationSystem::animate_bool(
            ui.ctx(),
            editor_id.with("expansion"),
            focused || collecting,
            theme::animation_timings(ui.ctx()).window_resize,
        );
        let expansion = AnimationSystem::ease_out_cubic(expansion);
        let expanded_height = (ui.ctx().content_rect().height() * 0.22).clamp(96.0, 160.0);
        let editor_height = egui::lerp(32.0..=expanded_height, expansion);
        let mut copy = false;
        let mut context_settings = false;
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
                let input_budget = context
                    .input_budget
                    .filter(|_| self.mode == TextMode::Translate);
                if input_budget.is_some() || focused {
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), 16.0),
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            if let Some(budget) = input_budget {
                                context_settings = ui
                                    .link(
                                        egui::RichText::new(tr(context.language, "Settings"))
                                            .size(11.0),
                                    )
                                    .on_hover_text(tr(context.language, "Context length"))
                                    .clicked();
                                context_indicator(ui, &self.text, budget, context.language);
                            }
                            if focused {
                                ui.with_layout(
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| {
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(tr(
                                                    context.language,
                                                    "Enter to send · Shift+Enter for a new line",
                                                ))
                                                .size(11.0)
                                                .color(theme::text_weak()),
                                            )
                                            .truncate(),
                                        );
                                    },
                                );
                            }
                        },
                    );
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
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), editor_height),
                    egui::Layout::right_to_left(egui::Align::Max),
                    |ui| {
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
                            && context.can_copy
                            && !waiting
                            && !collecting
                        {
                            copy = super::action_button::floating_copy_button(
                                ui,
                                tr(context.language, "Copy translation"),
                            )
                            .clicked();
                        }
                        if self.mode == TextMode::Translate && context.recognition.is_some() {
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
                        }
                        egui::ScrollArea::vertical()
                            .id_salt((id, "draft_scroll"))
                            .min_scrolled_height(32.0)
                            .max_height(editor_height)
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
                                            .min_size(egui::vec2(0.0, editor_height))
                                            .align(egui::Align2::LEFT_TOP)
                                            .frame(
                                                egui::Frame::NONE
                                                    .inner_margin(egui::Margin::symmetric(0, 8)),
                                            )
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
                                });
                            });
                    },
                );
                submit = (clicked || enter) && available && !self.text.trim().is_empty();
            },
        );
        if context_settings {
            Some(TextAction::ContextSettings)
        } else if toggle_collection {
            Some(TextAction::CollectRecognition {
                enabled: !collecting,
                text: self.text.clone(),
            })
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
    let response = ui.scope(|ui| {
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
