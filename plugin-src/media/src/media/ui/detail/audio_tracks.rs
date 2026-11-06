use crate::{
    i18n::UiLanguage,
    media::{backend::MediaSource, controller::MediaController, i18n::tr, task::PlaybackTrack},
    ui::components::{self, playback_button, playback_popup, playback_tooltip},
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
    let enabled = controller.subtitle_edit.is_none() && controller.speech_edit.is_none();
    let response = ui
        .add_enabled_ui(enabled, |ui| {
            playback_button(
                ui,
                tr(language, "Audio track"),
                xrtranslate_client_core::ui::icon!("media-audio-tracks.svg"),
                overlay,
                false,
            )
        })
        .inner;
    let mut selected = task.playback_sources();
    let original = controller
        .backend
        .as_ref()
        .map(|backend| backend.audio_tracks())
        .unwrap_or_default()
        .iter()
        .filter(|track| track.external.is_none())
        .collect::<Vec<_>>();
    if let Some(first) = original.first() {
        for source in &mut selected {
            if *source == PlaybackTrack::Original(None) {
                *source = PlaybackTrack::Original(Some(first.id));
            }
        }
    }
    let before = (selected.clone(), task.play_speech_track);
    let popup = playback_popup(&response, overlay)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .layout(egui::Layout::top_down_justified(egui::Align::Min))
        .show(|ui| {
            ui.set_width(200.0);
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            ui.add_enabled_ui(enabled, |ui| {
                if original.is_empty() {
                    track_option(
                        ui,
                        &mut selected,
                        PlaybackTrack::Original(None),
                        tr(language, "Original"),
                        overlay,
                    );
                }
                for track in &original {
                    let label = if !track.title.is_empty() {
                        track.title.clone()
                    } else if original.len() > 1 {
                        format!("{} {}", tr(language, "Original"), track.id)
                    } else {
                        tr(language, "Original").into()
                    };
                    track_option(
                        ui,
                        &mut selected,
                        PlaybackTrack::Original(Some(track.id)),
                        &label,
                        overlay,
                    );
                }
                if let Some(stems) = &task.separated_audio {
                    ui.separator();
                    for (value, label, path) in [
                        (PlaybackTrack::Vocals, "Vocals", &stems.vocals),
                        (PlaybackTrack::Background, "Background", &stems.background),
                    ] {
                        ui.add_enabled_ui(path.is_file(), |ui| {
                            track_option(ui, &mut selected, value, tr(language, label), overlay);
                        });
                    }
                }
                if let Some(track) = &task.speech_track {
                    ui.separator();
                    ui.add_enabled_ui(track.rendered && track.path.is_file(), |ui| {
                        let label = format!(
                            "{} · {}",
                            tr(language, "Speech track"),
                            crate::language_label(language, &track.language)
                        );
                        components::checkbox(ui, &mut task.play_speech_track, &label);
                    });
                }
            });
        });
    if before != (selected.clone(), task.play_speech_track) {
        task.playback_tracks = Some(selected);
        controller.apply_audio_selection(false);
    }
    if popup.is_some() {
        controller.fullscreen_last_interaction = std::time::Instant::now();
    }
}

fn track_option(
    ui: &mut egui::Ui,
    selected: &mut Vec<PlaybackTrack>,
    source: PlaybackTrack,
    label: &str,
    overlay: bool,
) {
    let mut checked = selected.contains(&source);
    if playback_tooltip(
        components::checkbox(ui, &mut checked, label),
        label,
        overlay,
    )
    .changed()
    {
        if checked {
            selected.push(source);
        } else {
            selected.retain(|value| *value != source);
        }
    }
}

// Recognition keeps one source; playback checkboxes never feed their mix into ASR.
pub(super) fn recognition_source(
    task: &mut crate::media::task::MediaTask,
    tracks: &[xrtranslate_client_core::playback::AudioTrack],
    language: UiLanguage,
    ui: &mut egui::Ui,
) -> bool {
    use crate::media::task::AudioTrackSelection;
    let originals = tracks
        .iter()
        .filter(|track| track.external.is_none())
        .collect::<Vec<_>>();
    let mut options = originals
        .iter()
        .map(|track| {
            let label = if !track.title.is_empty() {
                track.title.clone()
            } else if originals.len() > 1 {
                format!("{} {}", tr(language, "Original"), track.id)
            } else {
                tr(language, "Original").into()
            };
            ((AudioTrackSelection::Original, Some(track.id)), label)
        })
        .collect::<Vec<_>>();
    if options.is_empty() {
        options.push((
            (AudioTrackSelection::Original, None),
            tr(language, "Original").into(),
        ));
    }
    if let Some(stems) = &task.separated_audio {
        for (kind, label, path) in [
            (AudioTrackSelection::Vocals, "Vocals", &stems.vocals),
            (
                AudioTrackSelection::Background,
                "Background",
                &stems.background,
            ),
        ] {
            if path.is_file() {
                options.push((
                    (kind, task.original_audio_track),
                    tr(language, label).into(),
                ));
            }
        }
    }
    if options.len() < 2 {
        return false;
    }
    let before = (task.audio_track, task.original_audio_track);
    let mut selected = before;
    let label = options
        .iter()
        .find(|(value, _)| *value == selected)
        .map_or(tr(language, "Original"), |(_, label)| label.as_str());
    crate::ui::layout::flow_row(ui, |ui| {
        ui.label(tr(language, "Audio track"));
        components::combobox_ui(ui, "recognition_audio_track", label, |ui| {
            for (value, label) in &options {
                components::selectable_value(ui, &mut selected, *value, label);
            }
        });
    });
    (task.audio_track, task.original_audio_track) = selected;
    selected != before
}
