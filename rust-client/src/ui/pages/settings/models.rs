use crate::{
    i18n::{UiLanguage, tr},
    ui::{components, theme},
};
use eframe::egui;

pub(super) fn render(app: &mut crate::XRTranslateApp, ui: &mut egui::Ui) {
    let language = app.ui_language;
    for control in app.service_config.model_contexts() {
        let mut change = None;
        ui.push_id(
            (control.section, &control.provider, &control.window.model),
            |ui| {
                components::section(ui, tr(language, control.role), |ui| {
                    ui.set_width(ui.available_width().min(264.0));
                    ui.label(egui::RichText::new(&control.label).color(theme::text_weak()));
                    ui.horizontal_wrapped(|ui| {
                        let label = ui.label(tr(language, "Context length"));
                        let mut value = control.window.current;
                        let response = ui
                            .add(
                                egui::DragValue::new(&mut value)
                                    .range(control.window.minimum..=control.window.maximum)
                                    .speed(128.0),
                            )
                            .labelled_by(label.id);
                        let reset = ui
                            .add_enabled_ui(control.window.customized, |ui| {
                                components::reset_button(ui, "context_window")
                            })
                            .inner
                            .on_hover_text(format!(
                                "{} ({})",
                                tr(language, "Restore default"),
                                control.window.default
                            ));
                        if reset.clicked() || response.changed() {
                            let value = (!reset.clicked()).then_some(value);
                            change = Some((
                                response.id,
                                app.service_config.set_model_context(&control, value),
                            ));
                        }
                    });
                    if let Some(saved) = control.history_percent {
                        ui.add_space(4.0);
                        if let Some((id, percent)) = history_slider(ui, language, saved) {
                            change = Some((
                                id,
                                app.service_config
                                    .set_model_history_percent(&control, percent),
                            ));
                        }
                    }
                });
            },
        );
        if let Some((_id, result)) = change {
            #[cfg(not(target_os = "android"))]
            {
                app.pending_model_restart = result.is_ok().then_some(_id);
            }
            if let Err(error) = result {
                app.notifications.error("", &error);
            }
        }
        ui.add_space(12.0);
    }
    ui.label(
        egui::RichText::new(tr(language, "Restart XRTranslate to apply changes."))
            .small()
            .color(theme::text_weak()),
    );
}

fn history_slider(ui: &mut egui::Ui, language: UiLanguage, saved: u8) -> Option<(egui::Id, u8)> {
    use ui_director::egui::{Control, ControlAction, ElementKind, ElementValue};

    let id = ui.make_persistent_id("history_percent");
    let mut percent = ui
        .data_mut(|data| data.remove_temp::<u8>(id))
        .unwrap_or(saved);
    let label = tr(language, "Conversation history");
    let hint = tr(
        language,
        "Maximum share left for history after input, style and output. Left turns history off. Right restores Auto.",
    );
    let label_id = ui
        .horizontal(|ui| {
            let title = ui.label(label).on_hover_text(hint);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let value = match percent {
                    0 => tr(language, "Off").to_owned(),
                    100 => tr(language, "Auto").to_owned(),
                    n => format!("{n}%"),
                };
                ui.label(egui::RichText::new(value).color(theme::primary()));
            });
            title.id
        })
        .inner;
    let mut response = ui
        .scope(|ui| {
            let width = ui.available_width();
            let style = ui.style_mut();
            style.spacing.slider_width = width;
            style.spacing.interact_size.y = style.spacing.interact_size.y.max(24.0);
            style.spacing.slider_rail_height = 10.0;
            style.visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(5);
            style.visuals.widgets.inactive.bg_fill = theme::surface_tinted(theme::primary());
            style.visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.5, theme::primary());
            style.visuals.selection.bg_fill = theme::primary_fill();
            ui.add(
                egui::Slider::new(&mut percent, 0..=100)
                    .show_value(false)
                    .trailing_fill(true)
                    .step_by(1.0)
                    .smart_aim(false),
            )
        })
        .inner
        .labelled_by(label_id)
        .on_hover_text(hint);
    if let Some(ControlAction::Set(ElementValue::Number(value))) = Control::new(
        label,
        ElementKind::Slider,
        ElementValue::Number(f64::from(percent)),
    )
    .range(0.0, 100.0)
    .record(ui, id, response.enabled(), response.rect)
    {
        percent = value.round().clamp(0.0, 100.0) as u8;
        response.mark_changed();
    }
    if response.changed() {
        ui.ctx().request_repaint();
    }
    if response.dragged() {
        ui.data_mut(|data| data.insert_temp(id, percent));
        return None;
    }
    (percent != saved).then_some((response.id, percent))
}
