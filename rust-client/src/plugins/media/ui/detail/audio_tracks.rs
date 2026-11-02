use crate::{
    i18n::UiLanguage,
    plugins::media::{
        backend::MediaSource, controller::MediaController, i18n::tr, task::AudioTrackSelection,
    },
    ui::components::{playback_button, playback_popup, playback_tooltip},
};
use eframe::egui;

pub(super) fn render(controller: &mut MediaController, language: UiLanguage, ui: &mut egui::Ui) {
    let Some(task) = controller
        .active_task_id
        .as_deref()
        .and_then(|id| controller.store.get_mut(id))
    else {
        return;
    };
    if !matches!(
        task.source,
        MediaSource::LocalFile(_) | MediaSource::OnlineFile(_)
    ) {
        return;
    }
    let overlay = controller.fullscreen_mode;
    let enabled = !task.is_task_running && controller.subtitle_edit.is_none();
    let response = ui
        .add_enabled_ui(enabled, |ui| {
            playback_button(
                ui,
                tr(language, "Audio track"),
                egui::include_image!("../../../../../resources/icons/media-audio-tracks.svg"),
                overlay,
                false,
            )
        })
        .inner;
    let before = (task.audio_track, task.original_audio_track);
    let popup = playback_popup(&response, overlay)
        .layout(egui::Layout::top_down_justified(egui::Align::Min))
        .show(|ui| {
            ui.set_width(200.0);
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            let original = controller
                .backend
                .as_ref()
                .map(|backend| backend.audio_tracks())
                .unwrap_or_default()
                .iter()
                .filter(|track| track.external.is_none())
                .collect::<Vec<_>>();
            ui.add_enabled_ui(enabled, |ui| {
                if original.is_empty() {
                    ui.selectable_value(
                        &mut task.audio_track,
                        AudioTrackSelection::Original,
                        tr(language, "Original"),
                    );
                }
                for track in original.iter() {
                    let label = if !track.title.is_empty() {
                        track.title.clone()
                    } else if original.len() > 1 {
                        format!("{} {}", tr(language, "Original"), track.id)
                    } else {
                        tr(language, "Original").into()
                    };
                    let response = ui.selectable_label(
                        task.audio_track == AudioTrackSelection::Original
                            && task.original_audio_track == Some(track.id),
                        &label,
                    );
                    if playback_tooltip(response, &label, overlay).clicked() {
                        task.audio_track = AudioTrackSelection::Original;
                        task.original_audio_track = Some(track.id);
                        ui.close();
                    }
                }
                if let Some(stems) = &task.separated_audio {
                    ui.separator();
                    for (value, label, path) in [
                        (AudioTrackSelection::Vocals, "Vocals", &stems.vocals),
                        (
                            AudioTrackSelection::Background,
                            "Background",
                            &stems.background,
                        ),
                    ] {
                        ui.add_enabled_ui(path.is_file(), |ui| {
                            if ui
                                .selectable_value(&mut task.audio_track, value, tr(language, label))
                                .clicked()
                            {
                                ui.close();
                            }
                        });
                    }
                }
            });
        });
    if before != (task.audio_track, task.original_audio_track) {
        controller.apply_audio_selection();
    }
    if popup.is_some() {
        controller.fullscreen_last_interaction = std::time::Instant::now();
    }
}
