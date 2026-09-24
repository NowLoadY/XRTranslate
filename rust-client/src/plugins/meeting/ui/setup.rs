use super::{
    actions::{UiAction, draft_validation_error},
    presentation::{capture_label, meeting_language_label, page_header},
};
use crate::plugins::meeting::{
    MeetingAudioSource, MeetingUiSnapshot, controller::MeetingController, i18n::tr,
};
use crate::ui::components;
use eframe::egui;

pub(super) fn render_setup(
    controller: &mut MeetingController,
    snapshot: &MeetingUiSnapshot,
    ui: &mut egui::Ui,
) -> UiAction {
    let language = snapshot.language;
    let mut action = UiAction::None;
    page_header(
        ui,
        if controller.draft.import_audio {
            "Import audio"
        } else {
            "New live meeting"
        },
        language,
        |ui| {
            if components::animated_button(ui, tr(language, "Back")).clicked() {
                action = UiAction::Back;
            }
        },
    );

    let validation_error = draft_validation_error(controller, snapshot);

    components::card(ui, |ui| {
        ui.vertical(|ui| {
            components::section_heading(ui, tr(language, "Meeting Details"));

            ui.label(
                egui::RichText::new(tr(language, "Name"))
                    .strong()
                    .color(crate::ui::theme::text_strong()),
            );
            ui.add_space(4.0);
            components::input_field(
                ui,
                &mut controller.draft.name,
                tr(language, "Meeting name (e.g. Weekly Sync)"),
            );

            ui.add_space(14.0);

            if controller.draft.import_audio {
                ui.label(
                    egui::RichText::new(tr(language, "Audio File"))
                        .strong()
                        .color(crate::ui::theme::text_strong()),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.allocate_ui(
                        egui::vec2((ui.available_width() - 110.0).max(200.0), 0.0),
                        |ui| {
                            components::input_field(
                                ui,
                                &mut controller.draft.import_path,
                                tr(language, "Choose audio file path"),
                            );
                        },
                    );
                    ui.add_space(6.0);
                    if components::animated_button(ui, tr(language, "Choose file")).clicked()
                        && let Some(path) = rfd::FileDialog::new()
                            .add_filter("Audio", &["wav", "mp3", "flac", "m4a", "aac", "ogg"])
                            .pick_file()
                    {
                        controller.draft.import_path = path.display().to_string();
                        if controller.draft.name == "New meeting"
                            && let Some(stem) = path.file_stem().and_then(|value| value.to_str())
                        {
                            controller.draft.name = stem.to_owned();
                        }
                    }
                });
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(tr(
                        language,
                        "This meeting references the source file. Moving or deleting it prevents reprocessing.",
                    ))
                    .color(crate::ui::theme::text_weak())
                    .size(11.5),
                );
            } else {
                ui.label(
                    egui::RichText::new(tr(language, "Audio source"))
                        .strong()
                        .color(crate::ui::theme::text_strong()),
                );
                ui.add_space(4.0);

                let capture_options = [
                    (
                        MeetingAudioSource::Microphone,
                        capture_label(MeetingAudioSource::Microphone, language).to_string(),
                    ),
                    (
                        MeetingAudioSource::SystemAudio,
                        capture_label(MeetingAudioSource::SystemAudio, language).to_string(),
                    ),
                    (
                        MeetingAudioSource::Both,
                        capture_label(MeetingAudioSource::Both, language).to_string(),
                    ),
                ];
                components::searchable_combobox(
                    ui,
                    "meeting_capture_source",
                    capture_label(controller.draft.capture_source, language),
                    &mut controller.draft.capture_source,
                    &capture_options,
                );

                ui.add_space(8.0);
                ui.checkbox(
                    &mut controller.draft.save_recording,
                    tr(language, "Save audio for reprocessing"),
                );
            }

            ui.add_space(14.0);

            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(tr(language, "Spoken language"))
                            .strong()
                            .color(crate::ui::theme::text_strong()),
                    );
                    ui.add_space(4.0);

                    let mut source_options = vec![(
                        "auto".to_string(),
                        tr(language, "Auto (bidirectional)").to_string(),
                    )];
                    for (code, label) in crate::LANGUAGE_OPTIONS {
                        source_options.push((
                            (*code).to_string(),
                            tr(language, label).to_string(),
                        ));
                    }

                    if components::searchable_combobox(
                        ui,
                        "meeting_source_language",
                        meeting_language_label(&controller.draft.source_language, language),
                        &mut controller.draft.source_language,
                        &source_options,
                    ) && controller.draft.source_language != "auto"
                        && controller.draft.target_language == controller.draft.source_language
                    {
                        controller.draft.target_language =
                            if controller.draft.source_language == "zh" {
                                "en".to_string()
                            } else {
                                "zh".to_string()
                            };
                    }
                });
                ui.add_space(24.0);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(tr(language, "Translation language"))
                            .strong()
                            .color(crate::ui::theme::text_strong()),
                    );
                    ui.add_space(4.0);

                    components::target_language_pair_selector(
                        ui,
                        "meeting_setup",
                        &controller.draft.source_language,
                        &mut controller.draft.target_language,
                        language,
                        |code, lang| meeting_language_label(code, lang),
                    );
                });
            });

            ui.add_space(18.0);
            ui.separator();
            ui.add_space(14.0);

            if let Some(error) = validation_error.as_deref() {
                components::validation_notice(ui, language, tr(language, error));
                ui.add_space(12.0);
            }

            if components::primary_button_enabled(
                ui,
                tr(
                    language,
                    if controller.draft.import_audio {
                        "Create and process"
                    } else {
                        "Start meeting"
                    },
                ),
                validation_error.is_none(),
            )
            .clicked()
            {
                action = UiAction::CreateAndStart;
            }
        });
    });

    action
}
