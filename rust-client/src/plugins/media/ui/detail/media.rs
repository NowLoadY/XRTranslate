//! Shared playback controls and GPU video composed by egui.
use super::super::format_time_ms;
use crate::{
    plugins::media::{backend::PlaybackStatus, controller::MediaController, i18n::tr},
    ui::components,
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
    let video_rect = ui.available_rect_before_wrap();
    video_surface(controller, video_rect.size().max(egui::vec2(1.0, 1.0)), ui);
    if visible {
        let inset = 24.0_f32.min(video_rect.width() * 0.04);
        let overlay = egui::Area::new(controls_id)
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(
                video_rect.left() + inset,
                video_rect.top() + 20.0,
            ))
            .show(ui.ctx(), |ui| {
                ui.set_width((video_rect.width() - inset * 2.0).max(1.0));
                ui.visuals_mut().override_text_color = Some(egui::Color32::WHITE);
                playback_controls(controller, language, ui);
            });
        ui.data_mut(|data| data.insert_temp(controls_id, overlay.response.rect));
    } else {
        ui.data_mut(|data| data.remove::<egui::Rect>(controls_id));
        if ui.rect_contains_pointer(video_rect) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::None);
        }
    }
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(100));
}

fn video_surface(controller: &mut MediaController, size: egui::Vec2, ui: &mut egui::Ui) {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    ui.painter().rect_filled(rect, 0, egui::Color32::BLACK);
    if response.double_clicked() {
        fullscreen(controller, ui);
    }
    if let Some(backend) = &mut controller.backend {
        if ui.is_rect_visible(rect) {
            let scale = ui.ctx().pixels_per_point();
            match backend.video_frame([
                (rect.width() * scale).round() as u32,
                (rect.height() * scale).round() as u32,
            ]) {
                Ok(Some(texture)) => {
                    ui.painter().image(
                        texture,
                        rect,
                        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
                Ok(None) => {}
                Err(error) => {
                    backend.set_video_visible(false);
                    controller.error = Some(error);
                    controller.video_unavailable = true;
                }
            }
        } else {
            backend.set_video_visible(false);
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
        controller.fullscreen_mode,
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
    let overlay = controller.fullscreen_mode;
    let width = ui.available_width();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let playing = controller.get_status() == PlaybackStatus::Playing;
        if media_button(
            ui,
            tr(language, if playing { "Pause" } else { "Play" }),
            if playing {
                egui::include_image!("../../../../../resources/icons/media-pause.svg")
            } else {
                egui::include_image!("../../../../../resources/icons/media-play.svg")
            },
            overlay,
            false,
        )
        .clicked()
        {
            controller.toggle_play();
        }
        if media_button(
            ui,
            tr(language, if controller.muted { "Unmute" } else { "Mute" }),
            if controller.muted {
                egui::include_image!("../../../../../resources/icons/media-muted.svg")
            } else {
                egui::include_image!("../../../../../resources/icons/media-volume.svg")
            },
            overlay,
            false,
        )
        .clicked()
        {
            controller.toggle_mute();
        }
        let mut volume = f64::from(controller.volume);
        let volume_width = (width * 0.15).clamp(32.0, 70.0);
        if playback_slider(ui, &mut volume, 1.0, volume_width, true, overlay)
            .on_hover_text(tr(language, "Volume"))
            .changed()
        {
            controller.set_volume(volume as f32);
        }
        // Keep essential controls on one row, including on narrow screens.
        if width >= 360.0 {
            ui.label(
                egui::RichText::new(format!(
                    "{} / {}",
                    format_time_ms(controller.get_time_ms()),
                    format_time_ms(controller.get_duration_ms())
                ))
                .size(12.0),
            );
        }
        if controller.can_show_video() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if media_button(
                    ui,
                    tr(
                        language,
                        if overlay {
                            "Exit Fullscreen"
                        } else {
                            "Fullscreen"
                        },
                    ),
                    if overlay {
                        egui::include_image!("../../../../../resources/icons/media-collapse.svg")
                    } else {
                        egui::include_image!("../../../../../resources/icons/media-expand.svg")
                    },
                    overlay,
                    false,
                )
                .clicked()
                {
                    fullscreen(controller, ui);
                }
                let response = media_button(
                    ui,
                    tr(language, "Subtitles"),
                    egui::include_image!("../../../../../resources/icons/media-subtitles.svg"),
                    overlay,
                    controller.show_subtitles,
                );
                response.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::WidgetType::Checkbox,
                        ui.is_enabled(),
                        controller.show_subtitles,
                        tr(language, "Subtitles"),
                    )
                });
                if response.clicked() {
                    controller.show_subtitles = !controller.show_subtitles;
                }
            });
        }
    });
}

fn media_button(
    ui: &mut egui::Ui,
    label: &str,
    icon: egui::ImageSource<'static>,
    overlay: bool,
    selected: bool,
) -> egui::Response {
    let response = ui.add_sized([36.0, 36.0], egui::Button::new("").frame(false));
    let response =
        crate::ui::automation::button_response(ui, response.id, label, ui.is_enabled(), response);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    let active = response.hovered() || response.has_focus();
    let tint = if selected || active {
        if overlay {
            egui::Color32::from_rgb(155, 220, 255)
        } else {
            crate::ui::theme::primary()
        }
    } else if overlay {
        egui::Color32::WHITE
    } else {
        crate::ui::theme::text_strong()
    };
    let rect = response.rect.shrink(if active { 6.0 } else { 7.0 });
    if overlay {
        // A small icon outline stays legible on bright footage without a panel.
        for offset in [
            egui::vec2(-1.0, 0.0),
            egui::vec2(1.0, 0.0),
            egui::vec2(0.0, -1.0),
            egui::vec2(0.0, 1.0),
        ] {
            egui::Image::new(icon.clone())
                .tint(egui::Color32::from_black_alpha(160))
                .paint_at(ui, rect.translate(offset));
        }
    }
    egui::Image::new(icon).tint(tint).paint_at(ui, rect);
    if selected {
        ui.painter().circle_filled(
            egui::pos2(rect.center().x, response.rect.bottom() - 3.0),
            1.5,
            tint,
        );
    }
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(label)
}

fn playback_slider(
    ui: &mut egui::Ui,
    value: &mut f64,
    maximum: f64,
    width: f32,
    enabled: bool,
    overlay: bool,
) -> egui::Response {
    ui.allocate_ui_with_layout(
        egui::vec2(width, ui.spacing().interact_size.y),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.spacing_mut().slider_width = width;
            ui.spacing_mut().slider_rail_height = 4.0;
            // The page's transparent controls otherwise make the rail disappear.
            if overlay {
                let visuals = ui.visuals_mut();
                visuals.widgets.inactive.bg_fill = egui::Color32::from_white_alpha(120);
                visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
                visuals.widgets.hovered.bg_fill = egui::Color32::WHITE;
                visuals.widgets.active.bg_fill = egui::Color32::WHITE;
                visuals.selection.bg_fill = egui::Color32::WHITE;
            } else {
                ui.visuals_mut().widgets.inactive.bg_fill = crate::ui::theme::border();
            }
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
