use super::{
    actions::{UiAction, draft_validation_error},
    presentation::{capture_label, page_header},
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
    let import_notice = tr(
        language,
        "This meeting references the source file. Moving or deleting it prevents reprocessing.",
    );
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

    egui::ScrollArea::vertical()
        .id_salt("meeting_setup_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            components::card(ui, |ui| {
                ui.label(
                    egui::RichText::new(tr(language, "Name"))
                        .strong()
                        .color(crate::ui::theme::text_strong()),
                );
                ui.add_space(4.0);
                let name_height = if ui.available_width() < 600.0 {
                    44.0
                } else {
                    0.0
                };
                ui.add_sized([ui.available_width(), name_height], |ui: &mut egui::Ui| {
                    components::input_field(
                        ui,
                        &mut controller.draft.name,
                        tr(language, "Meeting name (e.g. Weekly Sync)"),
                    )
                });

                ui.add_space(14.0);

                if controller.draft.import_audio {
                    ui.label(
                        egui::RichText::new(tr(language, "Audio File"))
                            .strong()
                            .color(crate::ui::theme::text_strong()),
                    );
                    ui.add_space(4.0);
                    components::input_field(
                        ui,
                        &mut controller.draft.import_path,
                        tr(language, "Choose audio file path"),
                    );
                    let choose_file =
                        components::animated_button(ui, tr(language, "Choose file")).clicked();
                    if let Some(path) = crate::file_dialog::FileDialog::new()
                        .add_filter("Audio", &["wav", "mp3", "flac", "m4a", "aac", "ogg"])
                        .pick_file(ui.ctx(), "meeting_source_file", choose_file)
                    {
                        controller.draft.import_path = path.display().to_string();
                        if controller.draft.name == "New meeting"
                            && let Some(stem) = path.file_stem().and_then(|value| value.to_str())
                        {
                            controller.draft.name = stem.to_owned();
                        }
                    }
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(import_notice)
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

                    let system_audio = crate::audio::AudioSystem::supports_system_audio();
                    if !system_audio {
                        controller.draft.capture_source = MeetingAudioSource::Microphone;
                    }
                    let capture_options = [
                        MeetingAudioSource::Microphone,
                        MeetingAudioSource::SystemAudio,
                        MeetingAudioSource::Both,
                    ]
                    .into_iter()
                    .filter(|source| system_audio || *source == MeetingAudioSource::Microphone)
                    .map(|source| (source, capture_label(source, language).to_owned()))
                    .collect::<Vec<_>>();
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
                        tr(language, "Keep the recording"),
                    );
                }

                ui.add_space(14.0);

                components::translation_language_selector(
                    ui,
                    "meeting_setup",
                    &mut controller.draft.source_language,
                    &mut controller.draft.target_language,
                    snapshot.languages,
                    language,
                );

                ui.add_space(18.0);
                ui.separator();
                ui.add_space(14.0);

                let validation_error = draft_validation_error(controller, snapshot);
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
