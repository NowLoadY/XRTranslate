use crate::ui::components;
use eframe::egui;

pub fn render(
    plugin: &mut super::super::OscPlugin,
    ui: &mut egui::Ui,
    language: crate::i18n::UiLanguage,
) -> Vec<super::OscUiAction> {
    let mut actions = Vec::new();
    components::action_card(ui, |ui| {
        components::feature_ui(
            ui,
            crate::feature_access::Feature::OscChatbox,
            language,
            |ui| {
                ui.vertical(|ui| {
                    let status = plugin.manager().listener_status();
                    let is_active = status.contains("Listening") || status.contains("Active");
                    let compact = ui.available_width() < 420.0;
                    if compact {
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new(crate::i18n::tr(
                                    language,
                                    "OSC Network Settings",
                                ))
                                .size(14.0)
                                .color(crate::ui::theme::text_strong())
                                .strong(),
                            );
                            ui.add_space(4.0);
                            components::status_badge(ui, &status, is_active, false);
                        });
                    } else {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(crate::i18n::tr(
                                    language,
                                    "OSC Network Settings",
                                ))
                                .size(14.0)
                                .color(crate::ui::theme::text_strong())
                                .strong(),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    components::status_badge(ui, &status, is_active, false);
                                },
                            );
                        });
                    }

                    ui.add_space(10.0);

                    egui::Grid::new("osc_settings_grid")
                        .num_columns(2)
                        .spacing([24.0, 10.0])
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(crate::i18n::tr(language, "Target IP:"))
                                    .color(crate::ui::theme::text_strong())
                                    .size(12.5),
                            );
                            crate::ui::components::text_edit_ui(
                                ui,
                                "osc_target_ip",
                                egui::TextEdit::singleline(&mut plugin.draft_mut().ip)
                                    .desired_width(140.0),
                            );
                            ui.end_row();

                            ui.label(
                                egui::RichText::new(crate::i18n::tr(language, "Send Port:"))
                                    .color(crate::ui::theme::text_strong())
                                    .size(12.5),
                            );
                            ui.add(
                                egui::DragValue::new(&mut plugin.draft_mut().send_port)
                                    .range(1..=u16::MAX),
                            );
                            ui.end_row();

                            ui.label(
                                egui::RichText::new(crate::i18n::tr(language, "Listen Port:"))
                                    .color(crate::ui::theme::text_strong())
                                    .size(12.5),
                            );
                            ui.add(
                                egui::DragValue::new(&mut plugin.draft_mut().listen_port)
                                    .range(1..=u16::MAX),
                            );
                            ui.end_row();

                            ui.label(
                                egui::RichText::new(crate::i18n::tr(language, "Max Text Length:"))
                                    .color(crate::ui::theme::text_strong())
                                    .size(12.5),
                            );
                            ui.add(
                                egui::DragValue::new(&mut plugin.draft_mut().max_text_length)
                                    .range(1..=10_000),
                            );
                            ui.end_row();
                        });

                    ui.add_space(10.0);

                    ui.add_space(12.0);

                    ui.horizontal(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if components::primary_button(
                                ui,
                                crate::i18n::tr(language, "Apply & Save"),
                            )
                            .clicked()
                            {
                                actions.push(super::OscUiAction::SettingsApplied(
                                    plugin.apply_draft(),
                                ));
                                actions.push(super::OscUiAction::SaveSettings);
                            }
                        });
                    });
                });
            },
        );
    });
    actions
}
