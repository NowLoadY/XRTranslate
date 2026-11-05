use crate::{
    plugins::media::{controller::MediaController, i18n::tr},
    ui::{components, theme},
};
use eframe::egui;

pub(super) fn render(
    controller: &mut MediaController,
    language: crate::i18n::UiLanguage,
    ui: &mut egui::Ui,
) {
    let overlay = controller.fullscreen_mode;
    let label = tr(language, "Subtitle style");
    let tint = if overlay {
        egui::Color32::WHITE
    } else {
        theme::text_strong()
    };
    let response = ui.add_sized([18.0, 36.0], egui::Button::new("").frame(false));
    let response =
        ui_director::egui::button_response(ui, response.id, label, ui.is_enabled(), response);
    let response = components::playback_tooltip(response, label, overlay);
    let center = response.rect.center();
    let arrow_y = if overlay { -1.5 } else { 1.5 };
    ui.painter().add(egui::Shape::line(
        vec![
            center + egui::vec2(-3.0, arrow_y),
            center + egui::vec2(0.0, -arrow_y),
            center + egui::vec2(3.0, arrow_y),
        ],
        egui::Stroke::new(1.2, tint),
    ));
    let mut changed = false;
    components::playback_popup(&response, overlay)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_width(180.0);
            let Some(task) = controller
                .active_task_id
                .as_deref()
                .and_then(|id| controller.store.get_mut(id))
            else {
                return;
            };
            let style = &mut task.subtitle_style;
            ui.horizontal(|ui| {
                for (text, delta, hint) in [
                    ("a", -2.0, "Smaller subtitles"),
                    ("A", 2.0, "Larger subtitles"),
                ] {
                    let enabled = if delta < 0.0 {
                        style.size > 12.0
                    } else {
                        style.size < 72.0
                    };
                    let button = ui.add_enabled(
                        enabled,
                        egui::Button::new(egui::RichText::new(text).size(if delta < 0.0 {
                            14.0
                        } else {
                            20.0
                        }))
                        .min_size(egui::vec2(86.0, 36.0)),
                    );
                    let button = ui_director::egui::button_response(
                        ui,
                        button.id,
                        tr(language, hint),
                        enabled,
                        button,
                    );
                    if components::playback_tooltip(button, tr(language, hint), overlay).clicked() {
                        style.size = (style.size + delta).clamp(12.0, 72.0);
                        changed = true;
                    }
                }
            });
            let [r, g, b] = style.color;
            let mut color = egui::Color32::from_rgb(r, g, b);
            let color_row = ui
                .horizontal(|ui| {
                    ui.label(tr(language, "Text color"));
                    ui.add(
                        egui::Button::new("")
                            .fill(color)
                            .sense(egui::Sense::hover())
                            .min_size(egui::vec2(40.0, 18.0)),
                    );
                })
                .response
                .interact(egui::Sense::click());
            let color_row = ui_director::egui::button_response(
                ui,
                color_row.id,
                tr(language, "Text color"),
                ui.is_enabled(),
                color_row,
            );
            egui::containers::menu::SubMenu::new().show(ui, &color_row, |ui| {
                ui.spacing_mut().slider_width = 220.0;
                if egui::color_picker::color_picker_color32(
                    ui,
                    &mut color,
                    egui::color_picker::Alpha::Opaque,
                ) {
                    style.color = [color.r(), color.g(), color.b()];
                    changed = true;
                }
            });
            let button =
                components::button(ui, format!("{}: {}", tr(language, "Font"), style.font));
            egui::Popup::menu(&button).show(|ui| {
                ui.set_width(230.0);
                egui::ScrollArea::vertical()
                    .max_height(260.0)
                    .show(ui, |ui| {
                        changed |= components::searchable_options(
                            ui,
                            ui.make_persistent_id("subtitle_fonts"),
                            &mut style.font,
                            crate::ui::fonts::family_options(),
                        );
                    });
            });
        });
    if egui::Popup::is_any_open(ui.ctx()) {
        controller.fullscreen_last_interaction = std::time::Instant::now();
    }
    if changed {
        controller.save_active();
    }
}
