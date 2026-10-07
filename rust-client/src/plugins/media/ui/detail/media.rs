//! Shared playback controls; native video is an optional surface above them.
use super::super::format_time_ms;
use crate::{
    i18n::tr,
    plugins::media::{backend::PlaybackStatus, controller::MediaController},
    ui::{components, layout::flow_row},
};
use eframe::egui;

pub(super) fn render_audio_card(
    controller: &mut MediaController,
    language: crate::i18n::UiLanguage,
    ui: &mut egui::Ui,
) {
    components::card(ui, |ui| playback_controls(controller, language, ui));
}

pub(super) fn render_viewport_card(
    controller: &mut MediaController,
    language: crate::i18n::UiLanguage,
    ui: &mut egui::Ui,
) {
    components::card(ui, |ui| {
        let height = if controller.fullscreen_mode {
            (ui.available_height() - 100.0).max(120.0)
        } else {
            (ui.available_width() * 9.0 / 16.0).clamp(120.0, 400.0)
        };
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), height),
            egui::Sense::click(),
        );
        ui.painter().rect_filled(rect, 8, egui::Color32::BLACK);
        if response.double_clicked() {
            fullscreen(controller, ui);
        }
        if controller.native_host.is_none() {
            match crate::plugins::media::backend::window::NativeVideoHost::new(
                controller.parent_window,
            ) {
                Ok(host) => {
                    if let Some(backend) = &mut controller.backend {
                        backend.attach_native_host(host.hwnd.0);
                    }
                    controller.native_host = Some(host);
                }
                Err(error) => {
                    controller.error = Some(error);
                    controller.video_unavailable = true;
                }
            }
        }
        if let Some(host) = &controller.native_host {
            let rect = rect.intersect(ui.clip_rect());
            if rect.is_positive() {
                let scale = ui.ctx().pixels_per_point();
                host.set_rect(
                    (rect.left() * scale).round() as i32,
                    (rect.top() * scale).round() as i32,
                    (rect.width() * scale).round() as i32,
                    (rect.height() * scale).round() as i32,
                );
                host.show();
            } else {
                host.hide();
            }
        }
        ui.add_space(8.0);
        playback_controls(controller, language, ui);
    });
    if !controller.fullscreen_mode {
        egui::CollapsingHeader::new(tr(language, "Playback details")).show(ui, |ui| {
            let diagnostics = controller.get_diagnostics();
            ui.weak(format!(
                "{} · {} × {} · {:.1} FPS · {} · {} {}",
                diagnostics.video_codec,
                diagnostics.width,
                diagnostics.height,
                diagnostics.fps,
                diagnostics.hwdec_current,
                diagnostics.dropped_frames,
                tr(language, "Dropped frames")
            ));
        });
    }
}

fn playback_controls(
    controller: &mut MediaController,
    language: crate::i18n::UiLanguage,
    ui: &mut egui::Ui,
) {
    let duration = controller.get_duration_ms().max(1) as f64 / 1000.0;
    let mut position = controller.get_time_ms().max(0) as f64 / 1000.0;
    ui.spacing_mut().slider_width = ui.available_width();
    if ui
        .add(egui::Slider::new(&mut position, 0.0..=duration).show_value(false))
        .changed()
        && let Some(backend) = &mut controller.backend
    {
        backend.seek((position * 1000.0) as i64);
    }
    flow_row(ui, |ui| {
        let playing = controller.get_status() == PlaybackStatus::Playing;
        if components::secondary_button(ui, tr(language, if playing { "Pause" } else { "Play" }))
            .clicked()
        {
            controller.toggle_play();
        }
        ui.weak(format!(
            "{} / {}",
            format_time_ms(controller.get_time_ms()),
            format_time_ms(controller.get_duration_ms())
        ));
        if components::secondary_button(
            ui,
            tr(language, if controller.muted { "Unmute" } else { "Mute" }),
        )
        .clicked()
        {
            controller.toggle_mute();
        }
        let mut volume = controller.volume;
        ui.spacing_mut().slider_width = 70.0;
        if ui
            .add(egui::Slider::new(&mut volume, 0.0..=1.0).show_value(false))
            .on_hover_text(tr(language, "Volume"))
            .changed()
        {
            controller.set_volume(volume);
        }
        if controller.can_show_video() {
            ui.checkbox(&mut controller.show_subtitles, tr(language, "Subtitles"));
            if components::secondary_button(
                ui,
                tr(
                    language,
                    if controller.fullscreen_mode {
                        "Exit Fullscreen"
                    } else {
                        "Fullscreen"
                    },
                ),
            )
            .clicked()
            {
                fullscreen(controller, ui);
            }
        }
    });
}
fn fullscreen(controller: &mut MediaController, ui: &egui::Ui) {
    controller.toggle_fullscreen();
    ui.ctx()
        .send_viewport_cmd(egui::ViewportCommand::Fullscreen(
            controller.fullscreen_mode,
        ));
}
