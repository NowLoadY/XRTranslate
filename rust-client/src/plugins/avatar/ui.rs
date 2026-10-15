use super::AvatarPlugin;
use crate::{
    i18n::{UiLanguage, tr},
    plugins::text_output::{MessageFormat, render_banner_selector},
    ui::{components, theme},
};
use eframe::egui;

impl AvatarPlugin {
    pub(crate) fn render_page(
        &mut self,
        ui: &mut egui::Ui,
        language: UiLanguage,
        lyrics: bool,
    ) -> bool {
        let before = self.settings.clone();
        let selection_id = ui.id().with("avatar_region");
        let mut selected = ui
            .data(|data| data.get_temp::<usize>(selection_id))
            .unwrap_or(1);
        egui::ScrollArea::vertical()
            .id_salt("avatar_page_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(tr(language, "Avatar"))
                        .size(22.0)
                        .color(theme::text_strong())
                        .strong(),
                );
                ui.add_space(12.0);
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
                                    self.render_settings(ui, language);
                                });
                                ui.add_space(12.0);
                                self.render_region_settings(ui, language, lyrics, selected);
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
                        self.render_settings(ui, language);
                    });
                    ui.add_space(12.0);
                    self.render_preview(ui, language, &mut selected);
                    ui.add_space(12.0);
                    self.render_region_settings(ui, language, lyrics, selected);
                }
                ui.add_space(8.0);
            });
        ui.data_mut(|data| data.insert_temp(selection_id, selected));
        before != self.settings
    }

    fn render_preview(&mut self, ui: &mut egui::Ui, language: UiLanguage, selected: &mut usize) {
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.settings.captions, tr(language, "Display content"));
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
                if ui
                    .add_sized([ui.available_width(), height], button)
                    .on_hover_text(tr(language, label))
                    .clicked()
                {
                    *selected = index;
                    ui.ctx().request_repaint();
                }
            }
        });
    }

    fn render_region_settings(
        &mut self,
        ui: &mut egui::Ui,
        language: UiLanguage,
        lyrics: bool,
        selected: usize,
    ) {
        components::card(ui, |ui| {
            ui.set_width(ui.available_width());
            if selected != 1 {
                let (label, banner) = if selected == 0 {
                    ("Header:", &mut self.settings.header)
                } else {
                    ("Footer:", &mut self.settings.footer)
                };
                render_banner_selector(
                    ui,
                    tr(language, label),
                    "avatar_banner",
                    banner,
                    language,
                    lyrics,
                );
            } else {
                ui.checkbox(&mut self.settings.messages, tr(language, "Messages"));
                components::audio_source_filter(
                    ui,
                    &mut self.settings.audio_source_filter,
                    language,
                );
                ui.label(tr(language, "Format:"));
                egui::ComboBox::from_id_salt("avatar_message_format")
                    .width(ui.available_width())
                    .truncate()
                    .selected_text(self.settings.format.label(language))
                    .show_ui(ui, |ui| {
                        for mode in [
                            MessageFormat::TargetOnly,
                            MessageFormat::BilingualSourceFirst,
                            MessageFormat::BilingualTargetFirst,
                            MessageFormat::Inline,
                        ] {
                            ui.selectable_value(
                                &mut self.settings.format,
                                mode,
                                mode.label(language),
                            );
                        }
                    });
                ui.add(
                    egui::Slider::new(&mut self.settings.message_seconds, 1..=15)
                        .text(tr(language, "Display seconds")),
                );
            }
        });
    }
}
