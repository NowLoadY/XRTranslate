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
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading(tr(language, "Avatar"));
            ui.add_space(12.0);
            components::card(ui, |ui| {
                self.render_settings(ui, language);
            });
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.settings.captions, tr(language, "Display content"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if components::delete_button(ui, tr(language, "Clear messages")).clicked() {
                        self.output.clear_messages();
                    }
                });
            });
            ui.add_space(8.0);
            let selection_id = ui.id().with("avatar_region");
            let mut selected = ui
                .data(|data| data.get_temp::<usize>(selection_id))
                .unwrap_or(1);
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
                    let height = if index == 1 { 112.0 } else { 42.0 };
                    let button =
                        egui::Button::new(egui::RichText::new(text).color(theme::text_strong()))
                            .selected(selected == index)
                            .wrap();
                    if ui
                        .add_sized([ui.available_width(), height], button)
                        .on_hover_text(tr(language, label))
                        .clicked()
                    {
                        selected = index;
                    }
                }
            });
            ui.data_mut(|data| data.insert_temp(selection_id, selected));
            ui.add_space(12.0);
            components::card(ui, |ui| {
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
                    egui::ComboBox::from_label(tr(language, "Format:"))
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
        });
        before != self.settings
    }
}
