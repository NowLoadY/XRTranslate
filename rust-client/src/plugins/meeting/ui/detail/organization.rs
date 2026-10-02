use super::super::actions::UiAction;
use crate::plugins::meeting::{controller::MeetingController, i18n::tr};
use crate::ui::components;
use eframe::egui;

pub(super) fn render(
    controller: &mut MeetingController,
    language: crate::i18n::UiLanguage,
    action: &mut UiAction,
    ui: &mut egui::Ui,
) {
    egui::CollapsingHeader::new(tr(language, "Topics and speakers")).show(ui, |ui| {
        components::text_edit_ui(
            ui,
            "meeting_new_topic",
            egui::TextEdit::singleline(&mut controller.new_topic_title)
                .hint_text(tr(language, "New topic title"))
                .desired_width(f32::INFINITY),
        );
        if components::animated_button_enabled(
            ui,
            tr(language, "New topic"),
            !controller.new_topic_title.trim().is_empty(),
        )
        .clicked()
        {
            *action = UiAction::NewTopic;
        }
        ui.add_space(8.0);
        let speakers = controller
            .bundle
            .as_ref()
            .map(|bundle| bundle.speakers.clone())
            .unwrap_or_default();

        if !speakers.is_empty() {
            egui::CollapsingHeader::new(tr(language, "Manage speakers")).show(ui, |ui| {
                for speaker in &speakers {
                    ui.push_id(("speaker-editor", &speaker.id), |ui| {
                        ui.horizontal_wrapped(|ui| {
                            let name = controller
                                .speaker_name_drafts
                                .entry(speaker.id.clone())
                                .or_insert_with(|| speaker.name.clone());
                            crate::ui::components::text_edit_ui(
                                ui,
                                ("speaker_name", &speaker.id),
                                egui::TextEdit::singleline(name)
                                    .desired_width(180.0_f32.min(ui.available_width())),
                            );
                            if components::animated_button(ui, tr(language, "Rename")).clicked() {
                                *action = UiAction::RenameSpeaker(speaker.id.clone(), name.clone());
                            }
                            if speakers.len() > 1 {
                                let target = controller
                                    .speaker_merge_targets
                                    .entry(speaker.id.clone())
                                    .or_insert_with(|| {
                                        speakers
                                            .iter()
                                            .find(|other| other.id != speaker.id)
                                            .map(|other| other.id.clone())
                                            .unwrap_or_default()
                                    });

                                let merge_options: Vec<_> = speakers
                                    .iter()
                                    .filter(|other| other.id != speaker.id)
                                    .map(|other| (other.id.clone(), other.name.clone()))
                                    .collect();

                                let current_target_name = speakers
                                    .iter()
                                    .find(|other| other.id == *target)
                                    .map(|other| other.name.as_str())
                                    .unwrap_or_else(|| tr(language, "Merge into…"));

                                components::searchable_combobox(
                                    ui,
                                    ("merge-target", &speaker.id),
                                    current_target_name,
                                    target,
                                    &merge_options,
                                );

                                if components::danger_button(ui, tr(language, "Merge")).clicked()
                                    && !target.is_empty()
                                {
                                    *action =
                                        UiAction::MergeSpeaker(speaker.id.clone(), target.clone());
                                }
                            }
                        });
                    });
                }
                ui.label(
                    egui::RichText::new(tr(
                        language,
                        "Give speakers a name or combine labels for the same person.",
                    ))
                    .size(11.0)
                    .color(crate::ui::theme::text_weak()),
                );
            });
            ui.add_space(8.0);
        }
    });
}
