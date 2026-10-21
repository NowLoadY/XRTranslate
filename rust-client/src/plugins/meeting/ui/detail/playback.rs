use super::super::presentation::format_duration;
use crate::{
    plugins::meeting::{
        controller::MeetingController, i18n::tr, playback::MeetingPlayback, store::SegmentSource,
    },
    ui::{components, layout::flow_row},
};
use eframe::egui;

pub(super) fn render(
    controller: &mut MeetingController,
    language: crate::i18n::UiLanguage,
    ui: &mut egui::Ui,
) {
    if controller.playback.clips.is_empty() {
        return;
    }
    let capturing = controller.active_meeting_id().is_some();
    let playback = &mut controller.playback;
    let mut error = playback.take_error();
    components::card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.add_enabled_ui(!capturing, |ui| {
            let duration = playback.duration().max(0) as f64 / 1000.0;
            let mut position = (playback.position().max(0) as f64 / 1000.0).min(duration);
            let response = components::playback_slider(
                ui,
                &mut position,
                duration.max(0.001),
                ui.available_width(),
                duration > 0.0,
                false,
            );
            let automated = crate::ui::automation::record_slider(
                ui,
                response.id,
                tr(language, "Recording position"),
                position,
                ui.is_enabled() && duration > 0.0,
                response.rect,
            );
            if let Some(value) = automated {
                position = value.clamp(0.0, duration);
            }
            if response.changed() || automated.is_some() {
                playback.seek((position * 1000.0).round() as i64);
            }
            let width = ui.available_width();
            flow_row(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let playing = playback.playing();
                if components::playback_button(
                    ui,
                    tr(
                        language,
                        if playing {
                            "Pause playback"
                        } else {
                            "Play recording"
                        },
                    ),
                    if playing {
                        egui::include_image!("../../../../../resources/icons/media-pause.svg")
                    } else {
                        egui::include_image!("../../../../../resources/icons/media-play.svg")
                    },
                    false,
                    false,
                )
                .clicked()
                {
                    error = playback.toggle().err();
                }
                let mut volume = playback.volume;
                let mut muted = playback.muted;
                if components::playback_volume(
                    ui,
                    language,
                    &mut volume,
                    &mut muted,
                    (width * 0.15).clamp(32.0, 70.0),
                    false,
                ) {
                    playback.set_volume(volume, muted);
                }
                ui.label(
                    egui::RichText::new(format!(
                        "{} / {}",
                        format_duration(playback.position()),
                        format_duration(playback.duration())
                    ))
                    .size(12.0)
                    .weak(),
                );
                if width >= 520.0 {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Err(value) = clip_selector(playback, language, ui) {
                            error = Some(value);
                        }
                    });
                } else if let Err(value) = clip_selector(playback, language, ui) {
                    error = Some(value);
                }
            });
        });
    });
    if let Some(error) = error {
        controller.error = Some(error);
    }
    if playback.playing() {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
    }
    ui.add_space(8.0);
}

fn clip_selector(
    playback: &mut MeetingPlayback,
    language: crate::i18n::UiLanguage,
    ui: &mut egui::Ui,
) -> Result<(), String> {
    let label = |index: usize| {
        let source = tr(
            language,
            match playback.clips[index].source {
                SegmentSource::Microphone => "Microphone",
                SegmentSource::SystemAudio => "System audio",
                SegmentSource::ImportedAudio => "Imported audio",
            },
        );
        if playback.clips.len() == 1 {
            source.to_owned()
        } else {
            format!("{} · {}", index + 1, source)
        }
    };
    if playback.clips.len() > 1 {
        let mut selected = playback.selected;
        egui::ComboBox::from_id_salt("meeting_recording")
            .selected_text(label(selected))
            .show_ui(ui, |ui| {
                for index in 0..playback.clips.len() {
                    ui.selectable_value(&mut selected, index, label(index));
                }
            });
        if selected != playback.selected {
            playback.select(selected)?;
        }
    } else {
        ui.weak(label(0));
    }
    Ok(())
}
