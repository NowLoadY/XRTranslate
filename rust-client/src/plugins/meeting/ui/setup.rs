use super::{
    actions::{UiAction, draft_validation_error},
    presentation::{capture_label, page_header},
};
use crate::plugins::meeting::{
    MeetingAudioSource, MeetingUiSnapshot,
    controller::{MeetingController, MeetingInputMode},
    i18n::tr,
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
        match controller.draft.mode {
            MeetingInputMode::File => "Import audio",
            MeetingInputMode::Live => "New live meeting",
        },
        language,
        |ui| {
            if components::back_button(ui, tr(language, "Back")).clicked() {
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

                if controller.draft.mode == MeetingInputMode::File {
                    ui.label(
                        egui::RichText::new(tr(language, "Audio File"))
                            .strong()
                            .color(crate::ui::theme::text_strong()),
                    );
                    ui.add_space(4.0);
                    if components::source_input(
                        ui,
                        "meeting_source_file",
                        &mut controller.draft.import_path,
                        tr(language, "Choose audio file path"),
                        tr(language, "Choose file"),
                        &[("Audio", crate::plugins::media::task::AUDIO_EXTENSIONS)],
                    ) && controller.draft.name == "New meeting"
                        && std::path::Path::new(&controller.draft.import_path).is_file()
                        && let Some(stem) = std::path::Path::new(&controller.draft.import_path)
                            .file_stem()
                            .and_then(|value| value.to_str())
                    {
                        controller.draft.name = stem.to_owned();
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
                    if !system_audio
                        && matches!(
                            controller.draft.capture_source,
                            MeetingAudioSource::SystemAudio | MeetingAudioSource::Both
                        )
                    {
                        controller.draft.capture_source = MeetingAudioSource::Microphone;
                    }
                    let capture_options = [
                        MeetingAudioSource::Microphone,
                        MeetingAudioSource::SystemAudio,
                        MeetingAudioSource::Both,
                        MeetingAudioSource::OnlineStream,
                    ]
                    .into_iter()
                    .filter(|source| {
                        system_audio
                            || matches!(
                                source,
                                MeetingAudioSource::Microphone | MeetingAudioSource::OnlineStream
                            )
                    })
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
                    if controller.draft.capture_source == MeetingAudioSource::OnlineStream {
                        render_podcast(controller, snapshot, ui);
                    } else {
                        ui.checkbox(
                            &mut controller.draft.save_recording,
                            tr(language, "Keep the recording"),
                        );
                    }
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
                        if controller.draft.mode != MeetingInputMode::Live {
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

fn render_podcast(
    controller: &mut MeetingController,
    snapshot: &MeetingUiSnapshot,
    ui: &mut egui::Ui,
) {
    let language = snapshot.language;
    ui.label(tr(language, "Stream URL or RSS feed"));
    if components::input_field(ui, &mut controller.podcast.url, "https://…").changed() {
        controller.podcast.invalidate();
        controller.error = None;
    }
    match controller
        .podcast
        .render_selection(ui, language, &snapshot.download_proxy_url)
    {
        Ok(true) => {
            controller.error = None;
            if let Some(episode) = controller.podcast.selected_episode() {
                controller.draft.name = episode.title.clone();
            }
        }
        Err(error) => controller.error = Some(error),
        _ => {}
    }
    if let Some(error) = &controller.error {
        components::error_notice(ui, language, tr(language, error));
    }
}
