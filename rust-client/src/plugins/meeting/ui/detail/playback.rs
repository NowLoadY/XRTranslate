use super::super::presentation::format_duration;
use crate::{
    plugins::meeting::{controller::MeetingController, i18n::tr, store::SegmentSource},
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
            flow_row(ui, |ui| {
                let playing = playback.playing();
                if components::icon_button(
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
                        egui::include_image!("../../../../../resources/icons/pause.svg")
                    } else {
                        egui::include_image!("../../../../../resources/icons/play.svg")
                    },
                )
                .clicked()
                {
                    error = playback.toggle().err();
                }
                let label = |index: usize| {
                    let clip = &playback.clips[index];
                    let source = tr(
                        language,
                        match clip.source {
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
                        error = playback.select(selected).err();
                    }
                } else {
                    ui.weak(label(0));
                }
                ui.weak(format!(
                    "{} / {}",
                    format_duration(playback.position()),
                    format_duration(playback.duration())
                ));
            });
            ui.scope(|ui| {
                ui.spacing_mut().slider_width = ui.available_width();
                let mut position = playback.position() as f64 / 1000.0;
                let duration = playback.duration() as f64 / 1000.0;
                let response = ui.add_enabled(
                    duration > 0.0,
                    egui::Slider::new(&mut position, 0.0..=duration.max(0.001)).show_value(false),
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
