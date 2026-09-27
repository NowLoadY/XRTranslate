//! Built-in and user reference cards share storage, presentation and selection.
mod catalog;
mod controller;
mod editor;
mod transport;
pub use controller::TtsCenterController;

use crate::{
    i18n::{UiLanguage, tr, tr_dynamic},
    ui::components::{
        self, faded_scroll_text,
        selection_card::{self, SelectionCard},
    },
};
use eframe::egui;
use xrtranslate_assets::voices::builtin;

pub enum Action {
    Preview(String),
    Settings,
}

pub fn render(
    controller: &mut TtsCenterController,
    ui: &mut egui::Ui,
    language: UiLanguage,
    configured: bool,
    asr_languages: xrtranslate_engine::language::LanguageSet,
) -> Option<Action> {
    let mut action = None;
    ui.horizontal(|ui| {
        ui.heading(tr(language, "TTS Center"));
        if (controller.busy() && controller.requested_voice.is_none()) || controller.catalog.busy()
        {
            ui.spinner();
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.menu_button("⋯", |ui| {
                if ui.button(tr(language, "TTS settings")).clicked() {
                    action = Some(Action::Settings);
                    ui.close();
                }
                if ui
                    .add_enabled(
                        !controller.busy() && !controller.catalog.busy(),
                        egui::Button::new(tr(language, "Refresh")),
                    )
                    .clicked()
                {
                    controller.catalog.reload(ui.ctx());
                    if configured {
                        controller.refresh();
                    }
                    ui.close();
                }
            });
            if components::primary_button_enabled(
                ui,
                tr(language, "Add voice"),
                !controller.busy() && !controller.catalog.busy(),
            )
            .clicked()
            {
                controller.error = None;
                controller.draft = Some(Default::default());
            }
            if controller.status.personal_available
                && components::secondary_button_enabled(
                    ui,
                    tr(language, "Use my voice"),
                    configured && !controller.busy(),
                )
                .clicked()
            {
                controller.select(None);
            }
        });
    });
    ui.add_space(12.0);
    if !configured {
        ui.weak(tr(language, "Configure a TTS provider in Settings first."));
    } else if !controller.loaded {
        controller.refresh();
    }
    ui.horizontal(|ui| {
        components::input_field(
            ui,
            &mut controller.catalog.query,
            tr(language, "Search voices"),
        );
        ui.weak(controller.catalog.cards.len().to_string());
    });
    controller.catalog.filter_if_changed(language);
    if let Some(error) = &controller.error
        && controller.draft.is_none()
        && !controller.catalog.visible.iter().any(|&index| {
            controller.requested_voice.as_deref() == Some(&controller.catalog.cards[index].id)
        })
    {
        ui.colored_label(
            egui::Color32::from_rgb(165, 60, 65),
            tr_dynamic(language, error),
        );
    }
    let mut selected_id = None;
    selection_card::grid(
        ui,
        "tts_voice_cards",
        controller.catalog.visible.len(),
        300.0,
        |ui, index, width| {
            let card = &controller.catalog.cards[controller.catalog.visible[index]];
            let reference = builtin(&card.id);
            let requested = controller.requested_voice.as_deref() == Some(&card.id);
            let preparing = requested && controller.busy();
            let error = requested.then_some(controller.error.as_deref()).flatten();
            let selected = configured
                && controller.status.ready
                && controller.status.selection.voice_id.as_deref() == Some(&card.id);
            if selection_card::show(
                ui,
                SelectionCard {
                    id: egui::Id::new(("tts_voice", &card.id)),
                    title: reference.map_or(card.name.as_str(), |voice| tr(language, voice.name)),
                    avatar: &card.id,
                    width,
                    height: 300.0,
                    selected,
                    enabled: configured && !controller.busy(),
                    action: tr(
                        language,
                        if preparing {
                            "Creating voice…"
                        } else if error.is_some() {
                            "Retry"
                        } else if selected {
                            "In use"
                        } else {
                            "Use voice"
                        },
                    ),
                },
                |ui| {
                    let description = reference.map_or(card.description.as_str(), |voice| {
                        tr(language, voice.description)
                    });
                    let description = error
                        .map_or_else(|| description.into(), |error| tr_dynamic(language, error));
                    ui.allocate_ui(egui::vec2(ui.available_width(), 64.0), |ui| {
                        faded_scroll_text::show(
                            ui,
                            &description,
                            egui::FontId::proportional(13.0),
                            if error.is_some() {
                                crate::ui::theme::danger()
                            } else {
                                ui.visuals().text_color()
                            },
                        );
                    });
                    ui.horizontal(|ui| {
                        let playing = controller.previewing.as_deref() == Some(&card.id);
                        let label = tr(language, if playing { "Stop" } else { "Preview voice" });
                        let preview = ui
                            .add_enabled(
                                !controller.catalog.busy(),
                                egui::Button::new(if playing { "■" } else { "▶" })
                                    .min_size(egui::Vec2::splat(32.0))
                                    .corner_radius(16),
                            )
                            .on_hover_text(label);
                        preview.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::Button,
                                preview.enabled(),
                                label,
                            )
                        });
                        if preview.clicked() {
                            action = Some(Action::Preview(card.id.clone()));
                        }
                        ui.weak(format!("{:.1} s", card.duration_ms as f64 / 1000.0));
                        if preparing {
                            ui.spinner();
                        }
                    });
                    if let Some(reference) = reference {
                        ui.hyperlink_to(reference.credit, reference.source_url);
                    }
                },
            ) && !selected
            {
                selected_id = Some(card.id.clone());
            }
        },
    );
    if let Some(id) = selected_id {
        controller.select(Some(id));
    }
    let busy = controller.busy() || controller.catalog.busy();
    if let Some(draft) = &mut controller.draft {
        let (open, save) = editor::show(
            ui,
            draft,
            language,
            asr_languages,
            busy,
            controller.error.as_deref(),
        );
        let saved_draft = save.then(|| draft.clone());
        if !open {
            controller.draft = None;
        }
        if let Some(draft) = saved_draft {
            controller.import(draft, ui.ctx());
        }
    }
    action
}
