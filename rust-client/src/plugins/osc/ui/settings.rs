use crate::ui::components;
use eframe::egui;

pub fn render(
    plugin: &mut super::super::OscPlugin,
    ui: &mut egui::Ui,
    language: crate::i18n::UiLanguage,
) -> Vec<super::OscUiAction> {
    let mut actions = Vec::new();
    let game = plugin.game();
    let receiver_status = (game == super::super::OscGame::VRChat)
        .then(|| plugin.receiver().map(|receiver| receiver.state().listener))
        .flatten();
    components::action_card(ui, |ui| {
        components::feature_ui(
            ui,
            crate::feature_access::Feature::OscChatbox,
            language,
            |ui| {
                ui.vertical(|ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            egui::RichText::new(crate::i18n::tr(language, "OSC Network Settings"))
                                .size(14.0)
                                .color(crate::ui::theme::text_strong())
                                .strong(),
                        );
                        if let Some(status) = &receiver_status {
                            let normalized = status.to_ascii_lowercase();
                            let icon =
                                if normalized.contains("error") || normalized.contains("failed") {
                                    components::StatusIcon::Warning
                                } else if normalized.contains("listening")
                                    || normalized.starts_with("active")
                                {
                                    components::StatusIcon::Connected
                                } else {
                                    components::StatusIcon::Disconnected
                                };
                            components::status_icon(
                                ui,
                                icon,
                                &crate::i18n::tr_dynamic(language, status),
                            );
                        }
                    });

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

                            if game == super::super::OscGame::VRChat {
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
                            }

                            ui.label(
                                egui::RichText::new(crate::i18n::tr(language, "Max Text Length:"))
                                    .color(crate::ui::theme::text_strong())
                                    .size(12.5),
                            );
                            ui.add(
                                egui::DragValue::new(&mut plugin.draft_mut().max_text_length)
                                    .range(1..=game.max_text_length()),
                            );
                            ui.end_row();
                        });

                    ui.add_space(12.0);

                    ui.horizontal(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if components::primary_button(
                                ui,
                                crate::i18n::tr(language, "Apply & Save"),
                            )
                            .clicked()
                            {
                                actions.push(super::OscUiAction::ApplySettings);
                            }
                        });
                    });
                });
            },
        );
    });
    actions
}
