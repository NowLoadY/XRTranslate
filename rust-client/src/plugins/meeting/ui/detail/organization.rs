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
    let can_create_topic = controller.bundle.as_ref().is_some_and(|bundle| {
        bundle.meeting.status != crate::plugins::meeting::store::MeetingStatus::Processing
            && controller.active_capture.lock().ok().is_some_and(|active| {
                active.as_ref().is_some_and(|capture| {
                    capture.meeting_id == bundle.meeting.id && !capture.imported_audio
                })
            })
    });
    let speakers = controller
        .bundle
        .as_ref()
        .map(|bundle| bundle.speakers.clone())
        .unwrap_or_default();
    if !can_create_topic && speakers.is_empty() {
        return;
    }
    let title = if can_create_topic {
        "Topics and speakers"
    } else {
        "Manage speakers"
    };
    egui::CollapsingHeader::new(tr(language, title)).show(ui, |ui| {
        if can_create_topic {
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
        }

        if !speakers.is_empty() {
            for speaker in &speakers {
                ui.push_id(("speaker-editor", &speaker.id), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        let name = controller
                            .speaker_name_drafts
                            .entry(speaker.id.clone())
                            .or_insert_with(|| {
                                crate::compact_speaker_label(&speaker.name).unwrap_or_default()
                            });
                        crate::ui::components::text_edit_ui(
                            ui,
                            ("speaker_name", &speaker.id),
                            egui::TextEdit::singleline(name)
                                .desired_width(180.0_f32.min(ui.available_width())),
                        );
                        if components::edit_button(ui, tr(language, "Rename")).clicked() {
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
                            if !merge_options
                                .iter()
                                .any(|(id, _)| id.as_str() == target.as_str())
                            {
                                target.clear();
                            }

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

                            if components::danger_button_enabled(
                                ui,
                                tr(language, "Merge"),
                                !target.is_empty(),
                            )
                            .clicked()
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
            ui.add_space(8.0);
        }
    });
}
