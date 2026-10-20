//! Shared playback controls; native video is an optional surface above them.
use super::super::format_time_ms;
use crate::{
    plugins::media::{backend::PlaybackStatus, controller::MediaController, i18n::tr},
    ui::{components, layout::flow_row},
};
use eframe::egui;

pub(super) fn render_audio_card(
    controller: &mut MediaController,
    language: crate::i18n::UiLanguage,
    ui: &mut egui::Ui,
) {
    components::card(ui, |ui| {
        ui.set_width(ui.available_width());
        playback_controls(controller, language, ui);
    });
}

pub(super) fn render_viewport_card(
    controller: &mut MediaController,
    language: crate::i18n::UiLanguage,
    ui: &mut egui::Ui,
    max_height: f32,
) {
    if controller.fullscreen_mode {
        render_fullscreen(controller, language, ui);
        return;
    }
    components::card(ui, |ui| {
        let height = (ui.available_width() * 9.0 / 16.0).clamp(100.0, max_height.max(100.0));
        video_surface(controller, egui::vec2(ui.available_width(), height), ui);
        ui.add_space(8.0);
        playback_controls(controller, language, ui);
    });
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

fn render_fullscreen(
    controller: &mut MediaController,
    language: crate::i18n::UiLanguage,
    ui: &mut egui::Ui,
) {
    let controls_id = ui.id().with("fullscreen_controls_rect");
    let controls_rect = ui.data(|data| data.get_temp::<egui::Rect>(controls_id));
    let interaction = ui.input(|input| {
        input.pointer.delta() != egui::Vec2::ZERO
            || input.pointer.any_down()
            || input
                .events
                .iter()
                .any(|event| matches!(event, egui::Event::Key { pressed: true, .. }))
            || controls_rect.is_some_and(|rect| {
                input
                    .pointer
                    .hover_pos()
                    .is_some_and(|pos| rect.contains(pos))
            })
    });
    if interaction {
        controller.fullscreen_last_interaction = std::time::Instant::now();
    }
    if !ui.ctx().text_edit_focused() {
        if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Space)) {
            controller.toggle_play();
        }
        for (key, offset) in [
            (egui::Key::ArrowLeft, -5_000),
            (egui::Key::ArrowRight, 5_000),
        ] {
            if !ui.ctx().egui_wants_keyboard_input()
                && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, key))
            {
                let duration = controller.get_duration_ms();
                if duration > 0 {
                    controller.seek_to(
                        (controller.get_time_ms() + offset).clamp(0, duration),
                        false,
                    );
                }
            }
        }
    }
    let visible = controller.get_status() != PlaybackStatus::Playing
        || controller.fullscreen_last_interaction.elapsed() < std::time::Duration::from_secs(3);
    if visible {
        // The native video surface sits above egui. Reserve only the actual
        // control height while visible so neither surface can cover the other.
        let panel = egui::Panel::bottom("media_fullscreen_controls")
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(crate::ui::theme::surface_subtle())
                    .inner_margin(12),
            )
            .show(ui, |ui| playback_controls(controller, language, ui));
        ui.data_mut(|data| data.insert_temp(controls_id, panel.response.rect));
    } else {
        ui.data_mut(|data| data.remove::<egui::Rect>(controls_id));
        if ui.rect_contains_pointer(ui.available_rect_before_wrap()) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::None);
        }
    }
    video_surface(
        controller,
        ui.available_size().max(egui::vec2(1.0, 1.0)),
        ui,
    );
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(100));
}

fn video_surface(controller: &mut MediaController, size: egui::Vec2, ui: &mut egui::Ui) {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    ui.painter().rect_filled(rect, 0, egui::Color32::BLACK);
    if response.double_clicked() {
        fullscreen(controller, ui);
    }
    if controller.native_host.is_none() {
        match crate::plugins::media::backend::window::NativeVideoHost::new(controller.parent_window)
        {
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
}

fn playback_controls(
    controller: &mut MediaController,
    language: crate::i18n::UiLanguage,
    ui: &mut egui::Ui,
) {
    let duration = controller.get_duration_ms().max(0) as f64 / 1000.0;
    let mut position = (controller.get_time_ms().max(0) as f64 / 1000.0).min(duration);
    let response = playback_slider(
        ui,
        &mut position,
        duration.max(1.0),
        ui.available_width(),
        duration > 0.0,
    );
    let automated = crate::ui::automation::record_slider(
        ui,
        response.id,
        tr(language, "Playback position"),
        position,
        duration > 0.0,
        response.rect,
    );
    if let Some(value) = automated {
        position = value.clamp(0.0, duration);
    }
    if response.changed() || automated.is_some() {
        controller.seek_to((position * 1000.0) as i64, false);
    }
    // Native playback changes on an audio worker, including reaching EOF.
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(100));
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
        let mut volume = f64::from(controller.volume);
        if playback_slider(ui, &mut volume, 1.0, 70.0, true)
            .on_hover_text(tr(language, "Volume"))
            .changed()
        {
            controller.set_volume(volume as f32);
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
            .on_hover_text(tr(language, "Double-click video or press Esc to leave fullscreen. Space to play/pause, arrows to seek."))
            .clicked()
            {
                fullscreen(controller, ui);
            }
        }
    });
}

fn playback_slider(
    ui: &mut egui::Ui,
    value: &mut f64,
    maximum: f64,
    width: f32,
    enabled: bool,
) -> egui::Response {
    ui.allocate_ui_with_layout(
        egui::vec2(width, ui.spacing().interact_size.y),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.spacing_mut().slider_width = width;
            ui.spacing_mut().slider_rail_height = 4.0;
            // The page's transparent controls otherwise make the rail disappear.
            ui.visuals_mut().widgets.inactive.bg_fill = crate::ui::theme::border();
            ui.add_enabled(
                enabled,
                egui::Slider::new(value, 0.0..=maximum)
                    .show_value(false)
                    .trailing_fill(true),
            )
        },
    )
    .inner
}

fn fullscreen(controller: &mut MediaController, ui: &egui::Ui) {
    controller.toggle_fullscreen();
    ui.ctx()
        .send_viewport_cmd(egui::ViewportCommand::Fullscreen(
            controller.fullscreen_mode,
        ));
}
