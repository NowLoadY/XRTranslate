use crate::{
    i18n::tr,
    ui::{components, theme},
};
use eframe::egui;

pub(super) fn render(app: &mut crate::XRTranslateApp, ui: &mut egui::Ui) {
    let language = app.ui_language;
    for control in app.service_config.model_contexts() {
        ui.push_id(
            (control.section, &control.provider, &control.window.model),
            |ui| {
                components::section(ui, tr(language, control.role), |ui| {
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
                        let changed = if reset.clicked() {
                            Some(None)
                        } else if response.changed() {
                            Some(Some(value))
                        } else {
                            None
                        };
                        if let Some(value) = changed {
                            match app.service_config.set_model_context(&control, value) {
                                Ok(()) => {
                                    #[cfg(not(target_os = "android"))]
                                    {
                                        app.pending_model_restart = Some(response.id);
                                    }
                                }
                                Err(error) => {
                                    #[cfg(not(target_os = "android"))]
                                    {
                                        app.pending_model_restart = None;
                                    }
                                    app.notifications.error("", &error);
                                }
                            }
                        }
                    });
                });
            },
        );
        ui.add_space(12.0);
    }
    ui.label(
        egui::RichText::new(tr(language, "Restart XRTranslate to apply changes."))
            .small()
            .color(theme::text_weak()),
    );
}
