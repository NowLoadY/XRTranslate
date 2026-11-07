use crate::{
    i18n::UiLanguage,
    media::{
        backend::MediaSource,
        controller::MediaController,
        i18n::tr,
        task::{MediaTask, PlaybackTrack},
    },
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
    let tracks = controller
        .backend
        .as_ref()
        .map(|backend| backend.audio_tracks())
        .unwrap_or_default();
    let mut changed = false;
    let popup = playback_popup(&response, overlay)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_width(310.0);
            // Recompute room for the list instead of inheriting the popup's cached height.
            ui.set_max_height((ui.ctx().content_rect().height() - 24.0).clamp(0.0, 300.0));
            ui.add_enabled_ui(enabled, |ui| {
                changed = controls(task, tracks, language, ui, overlay);
            });
        });
    if changed {
        controller.apply_audio_selection(false);
    }
    if popup.is_some() {
        controller.fullscreen_last_interaction = std::time::Instant::now();
    }
}

// One editor and one saved mix for playback and video export.
pub(in crate::media::ui) fn controls(
    task: &mut MediaTask,
    tracks: &[xrtranslate_client_core::playback::AudioTrack],
    language: UiLanguage,
    ui: &mut egui::Ui,
    overlay: bool,
) -> bool {
    let mut selected = task.playback_sources();
    let original: Vec<_> = tracks
        .iter()
        .filter(|track| track.external.is_none())
        .collect();
    if let Some(first) = original.first() {
        for source in &mut selected {
            if *source == PlaybackTrack::Original(None) {
                *source = PlaybackTrack::Original(Some(first.id));
            }
        }
    }
    let before = (
        selected.clone(),
        task.play_speech_track,
        task.audio_mix.clone(),
    );
    ui.scope(|ui| {
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        egui::ScrollArea::vertical()
            .max_height(300.0)
            .show(ui, |ui| {
                let mut row = |ui: &mut egui::Ui, source, label: &str, available| {
                    let mut checked = selected.contains(&source);
                    let mut volume = task.audio_mix.volume(source);
                    ui.push_id(format!("{source:?}"), |ui| {
                        ui.add_enabled_ui(available, |ui| {
                            track_row(ui, &mut checked, &mut volume, label, language, overlay);
                        });
                    });
                    if checked && !selected.contains(&source) {
                        selected.push(source);
                        // Explicitly returning to the mixed original also restores
                        // its voices; do not leave the ducking switch misleadingly on.
                        if matches!(source, PlaybackTrack::Original(_)) {
                            task.audio_mix.duck_vocals = false;
                        }
                    }
                    if !checked {
                        selected.retain(|item| *item != source);
                    }
                    if volume != task.audio_mix.volume(source) {
                        task.audio_mix.set_volume(source, volume);
                    }
                };
                if original.is_empty() {
                    row(
                        ui,
                        PlaybackTrack::Original(task.original_audio_track),
                        tr(language, "Original"),
                        true,
                    );
                }
                for track in original {
                    let label = if !track.title.is_empty() {
                        track.title.clone()
                    } else {
                        format!("{} {}", tr(language, "Original"), track.id)
                    };
                    row(ui, PlaybackTrack::Original(Some(track.id)), &label, true);
                }
                if let Some(stems) = &task.separated_audio {
                    ui.separator();
                    row(
                        ui,
                        PlaybackTrack::Vocals,
                        tr(language, "Vocals"),
                        stems.vocals.is_file(),
                    );
                    row(
                        ui,
                        PlaybackTrack::Background,
                        tr(language, "Background"),
                        stems.background.is_file(),
                    );
                }
                if let Some(track) = &task.speech_track {
                    ui.separator();
                    let label = format!(
                        "{} · {}",
                        tr(language, "Speech track"),
                        crate::language_label(language, &track.language)
                    );
                    ui.push_id("speech", |ui| {
                        ui.add_enabled_ui(track.rendered && track.path.is_file(), |ui| {
                            track_row(
                                ui,
                                &mut task.play_speech_track,
                                &mut task.audio_mix.speech,
                                &label,
                                language,
                                overlay,
                            );
                        });
                    });
                    if task
                        .separated_audio
                        .as_ref()
                        .is_some_and(|stems| stems.vocals.is_file())
                    {
                        ui.add_enabled_ui(
                            task.play_speech_track && selected.contains(&PlaybackTrack::Vocals),
                            |ui| {
                                if components::checkbox(
                                    ui,
                                    &mut task.audio_mix.duck_vocals,
                                    tr(language, "Lower original vocals during dubbing"),
                                )
                                .changed()
                                    && task.audio_mix.duck_vocals
                                {
                                    // The unsplit original would bypass the vocal envelope.
                                    // Replace it with the same file's separated background.
                                    let had_original = selected
                                        .iter()
                                        .any(|source| matches!(source, PlaybackTrack::Original(_)));
                                    selected.retain(|source| {
                                        !matches!(source, PlaybackTrack::Original(_))
                                    });
                                    if had_original
                                        && task
                                            .separated_audio
                                            .as_ref()
                                            .is_some_and(|stems| stems.background.is_file())
                                        && !selected.contains(&PlaybackTrack::Background)
                                    {
                                        selected.push(PlaybackTrack::Background);
                                    }
                                }
                            },
                        );
                    }
                }
            });
    });
    let changed = before
        != (
            selected.clone(),
            task.play_speech_track,
            task.audio_mix.clone(),
        );
    if changed {
        task.playback_tracks = Some(selected);
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_track_rows_keep_long_names_inside_the_menu() {
        let ctx = egui::Context::default();
        for width in [310.0, 340.0] {
            for overlay in [false, true] {
                let mut bounds = egui::Rect::NOTHING;
                let mut output = ctx.run_ui(egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 180.0))),
                    ..Default::default()
                }, |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                    track_row(ui, &mut true, &mut 200, "A very long original audio track name that must not displace the volume", UiLanguage::English, overlay);
                    bounds = ui.min_rect();
                });
                output.textures_delta.clear();
                assert!(bounds.right() <= width, "track row overflows: {bounds:?}");
                assert!(
                    bounds.height() < 40.0,
                    "track row grew vertically: {bounds:?}"
                );
            }
        }
    }
}

fn track_row(
    ui: &mut egui::Ui,
    checked: &mut bool,
    volume: &mut u16,
    label: &str,
    language: UiLanguage,
    overlay: bool,
) {
    ui.horizontal(|ui| {
        let label_width = (ui.available_width() - 145.0).max(70.0);
        ui.allocate_ui_with_layout(
            egui::vec2(label_width, 28.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                playback_tooltip(components::checkbox(ui, checked, label), label, overlay);
            },
        );
        ui.add_enabled_ui(*checked, |ui| {
            let mut value = f64::from((*volume).min(200));
            let response = components::playback_slider(ui, &mut value, 200.0, 88.0, true, overlay);
            let name = format!("{label} · {}", tr(language, "Volume"));
            let automated = ui_director::egui::record_slider(
                ui,
                response.id,
                &name,
                value,
                ui.is_enabled(),
                response.rect,
            );
            if response.double_clicked() {
                value = 100.0;
            }
            if let Some(value) = automated {
                *volume = value.clamp(0.0, 200.0).round() as u16;
            } else if response.changed() || response.double_clicked() {
                *volume = value.round() as u16;
            }
            playback_tooltip(response, &format!("{name} · {}%", *volume), overlay);
            ui.label(format!("{}%", *volume));
        });
    });
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
