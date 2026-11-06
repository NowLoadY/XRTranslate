use super::AvatarPlugin;
use crate::{
    i18n::{UiLanguage, tr},
    text_output::MessageFormat,
    ui::{components, theme},
};
use eframe::egui;

impl AvatarPlugin {
    pub(crate) fn render_content(
        &mut self,
        ui: &mut egui::Ui,
        language: UiLanguage,
        enabled: bool,
    ) -> bool {
        let before = self.settings.clone();
        let selection_id = ui.make_persistent_id("avatar_region");
        let mut selected = ui
            .data(|data| data.get_temp::<usize>(selection_id))
            .unwrap_or(1);
        let width = ui.available_width();
        if width >= 720.0 {
            let left_width = (width * 0.46).clamp(320.0, 420.0);
            let right_width = width - left_width - ui.spacing().item_spacing.x;
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(left_width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        components::card(ui, |ui| {
                            ui.set_width(ui.available_width());
                            self.render_connection(ui, language, enabled);
                        });
                        ui.add_space(12.0);
                        self.render_region_settings(ui, language, selected);
                    },
                );
                ui.allocate_ui_with_layout(
                    egui::vec2(right_width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| self.render_preview(ui, language, &mut selected),
                );
            });
        } else {
            components::card(ui, |ui| {
                ui.set_width(ui.available_width());
                self.render_connection(ui, language, enabled);
            });
            ui.add_space(12.0);
            self.render_preview(ui, language, &mut selected);
            ui.add_space(12.0);
            self.render_region_settings(ui, language, selected);
        }
        ui.data_mut(|data| data.insert_temp(selection_id, selected));
        before != self.settings
    }

    fn render_preview(&mut self, ui: &mut egui::Ui, language: UiLanguage, selected: &mut usize) {
        ui.horizontal(|ui| {
            ui.label(tr(language, "Preview"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if components::delete_button(ui, tr(language, "Clear messages")).clicked() {
                    self.output.clear_messages();
                }
            });
        });
        ui.add_space(8.0);
        let snapshot = self.output.preview();
        let texts = [
            snapshot.header.unwrap_or_default(),
            snapshot
                .messages
                .iter()
                .map(|message| message.text.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            snapshot.footer.unwrap_or_default(),
        ];
        components::card(ui, |ui| {
            for (index, label) in ["Header", "Messages", "Footer"].into_iter().enumerate() {
                let text = if texts[index].is_empty() {
                    tr(language, label).to_owned()
                } else {
                    texts[index].clone()
                };
                let height = if index == 1 { 160.0 } else { 42.0 };
                let button =
                    egui::Button::new(egui::RichText::new(text).color(theme::text_strong()))
                        .selected(*selected == index)
                        .wrap();
                let response = ui
                    .add_sized([ui.available_width(), height], button)
                    .on_hover_text(tr(language, label));
                if ui_director::egui::Control::button(tr(language, label))
                    .response(ui, response)
                    .clicked()
                {
                    *selected = index;
                    ui.ctx().request_repaint();
                }
            }
        });
    }

    fn render_region_settings(&mut self, ui: &mut egui::Ui, language: UiLanguage, selected: usize) {
        components::card(ui, |ui| {
            ui.set_width(ui.available_width());
            if selected != 1 {
                let (label, banner) = if selected == 0 {
                    ("Header", &mut self.settings.header)
                } else {
                    ("Footer", &mut self.settings.footer)
                };
                ui.label(tr(language, label));
                let response = components::text_edit_ui(
                    ui,
                    ("avatar_text", selected),
                    egui::TextEdit::multiline(banner)
                        .desired_width(f32::INFINITY)
                        .desired_rows(3),
                );
                ui_director::egui::text_response(ui, tr(language, label), banner, response);
            } else {
                crate::ui::components::checkbox(
                    ui,
                    &mut self.settings.messages,
                    tr(language, "Messages"),
                );
                ui.add_enabled_ui(self.settings.messages, |ui| {
                    components::audio_source_filter(
                        ui,
                        &mut self.settings.audio_source_filter,
                        language,
                    );
                    ui.horizontal(|ui| {
                        ui.label(tr(language, "Format:"));
                        components::combobox_ui_with_width(
                            ui,
                            "avatar_message_format",
                            tr(language, self.settings.format.label_key()),
                            Some(ui.available_width()),
                            |ui| {
                                for mode in [
                                    MessageFormat::TargetOnly,
                                    MessageFormat::BilingualSourceFirst,
                                    MessageFormat::BilingualTargetFirst,
                                    MessageFormat::Inline,
                                ] {
                                    components::selectable_value(
                                        ui,
                                        &mut self.settings.format,
                                        mode,
                                        tr(language, mode.label_key()),
                                    );
                                }
                            },
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label(tr(language, "Display seconds"));
                        components::number_input(
                            ui,
                            &mut self.settings.message_seconds,
                            tr(language, "Display seconds"),
                            1..=15,
                        );
                    });
                });
            }
        });
    }
}
