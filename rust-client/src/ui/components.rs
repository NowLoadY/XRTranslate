use crate::ui::theme;
use eframe::egui::{self, Color32, CornerRadius, Frame, Margin, Stroke, Ui, Vec2};

#[cfg(test)]
mod language_tests;

pub fn card<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let border_id = ui.next_auto_id().with("organic_card_border");
    crate::ui::organic_border::show(
        ui,
        border_id,
        Frame::new()
            .fill(Color32::TRANSPARENT)
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::same(16))
            .shadow(egui::Shadow::NONE),
        10.0,
        theme::border(),
        add_contents,
    )
    .inner
}

pub fn action_card<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let border_id = ui.next_auto_id().with("organic_action_card_border");
    crate::ui::organic_border::show(
        ui,
        border_id,
        Frame::new()
            .fill(Color32::TRANSPARENT)
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::symmetric(16, 12)),
        10.0,
        theme::border(),
        add_contents,
    )
    .inner
}

pub fn history_entry_card<R>(
    ui: &mut Ui,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> egui::Response {
    Frame::new()
        .fill(theme::history_surface())
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(12, 9))
        .stroke(Stroke::new(1.0, theme::border()))
        .show(ui, add_contents)
        .response
}

pub fn dark_container_frame<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::new()
        .fill(Color32::from_rgb(15, 23, 42))
        .stroke(Stroke::new(1.0, Color32::from_rgb(51, 65, 85)))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(12))
        .show(ui, add_contents)
        .inner
}

pub fn speaker_badge(ui: &mut Ui, speaker: &str) {
    ui.label(
        egui::RichText::new(speaker)
            .color(theme::primary_dark())
            .size(11.5)
            .strong(),
    );
}

pub fn swap_capsule_button(ui: &mut Ui, enabled: bool) -> egui::Response {
    let id = ui.make_persistent_id("lang_swap_capsule");
    let is_hovered = ui.memory(|m| {
        m.data
            .get_temp::<bool>(id.with("hover_state"))
            .unwrap_or(false)
    });
    let is_active = ui.memory(|m| {
        m.data
            .get_temp::<bool>(id.with("active_state"))
            .unwrap_or(false)
    });

    let hover_factor = crate::ui::animation::AnimationSystem::hover(
        ui.ctx(),
        id.with("hover"),
        is_hovered && enabled,
    );
    let active_factor = crate::ui::animation::AnimationSystem::active(
        ui.ctx(),
        id.with("active"),
        is_active && enabled,
    );

    let current_time = ui.ctx().input(|i| i.time);
    let click_time = ui.memory(|m| m.data.get_temp::<f64>(id.with("click_time")).unwrap_or(0.0));
    let elapsed = (current_time - click_time) as f32;
    let click_duration = crate::ui::animation::AnimationSystem::button_click_duration(ui.ctx());
    let is_animating_click = elapsed >= 0.0 && elapsed < click_duration;
    let click_factor = if is_animating_click {
        ui.ctx().request_repaint();
        let t = (elapsed / click_duration).clamp(0.0, 1.0);
        (1.0 - crate::ui::animation::AnimationSystem::ease_out_cubic(t)).clamp(0.0, 1.0)
    } else {
        0.0
    };

    let fill = theme::surface_control();

    let text_color = if enabled {
        let base = crate::ui::animation::AnimationSystem::lerp_color(
            theme::primary(),
            theme::primary_dark(),
            hover_factor,
        );
        let base = crate::ui::animation::AnimationSystem::lerp_color(
            base,
            theme::primary_dark(),
            active_factor,
        );
        crate::ui::animation::AnimationSystem::lerp_color(base, theme::primary_dark(), click_factor)
    } else {
        Color32::from_rgb(148, 163, 184)
    };

    let stroke_color = theme::border();

    let resp = Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, stroke_color))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(9, 4))
        .shadow(egui::Shadow::NONE)
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new("↔")
                    .color(text_color)
                    .size(13.0)
                    .strong(),
            )
        })
        .response
        .interact(egui::Sense::click());

    if resp.clicked() {
        ui.memory_mut(|m| {
            m.data.insert_temp(id.with("click_time"), current_time);
        });
        ui.ctx().request_repaint();
    }

    ui.memory_mut(|m| {
        m.data.insert_temp(id.with("hover_state"), resp.hovered());
        m.data
            .insert_temp(id.with("active_state"), resp.is_pointer_button_down_on());
    });
    if resp.hovered() && enabled {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

pub fn segmented_audio_meter(
    ui: &mut Ui,
    id_source: &'static str,
    raw_fraction: f32,
    active: bool,
    visible: bool,
    updating: bool,
) {
    if !visible {
        return;
    }

    const HEIGHT: f32 = 13.0;
    const WIDTH: f32 = 68.0;
    const SAMPLES: usize = 20;
    const RENDER_POINTS: usize = 64;

    let id = ui.make_persistent_id(("audio_waveform_meter", id_source));
    let now = ui.ctx().input(|i| i.time);

    let raw_level = if updating { raw_fraction.clamp(0.0, 1.0) } else { 0.0 };
    let target_amplitude = if raw_level < 0.015 {
        0.0
    } else {
        (raw_level.sqrt() * 1.5).clamp(0.0, 1.0)
    };

    let mut history = ui.memory(|m| {
        m.data
            .get_temp::<Vec<f32>>(id.with("history"))
            .unwrap_or_else(|| vec![0.0; SAMPLES])
    });
    if history.len() != SAMPLES {
        history.resize(SAMPLES, 0.0);
    }

    let last_sample_time =
        ui.memory(|m| m.data.get_temp::<f64>(id.with("last_time")).unwrap_or(now));

    if !updating {
        history.fill(0.0);
        ui.memory_mut(|m| {
            m.data.insert_temp(id.with("history"), history.clone());
            m.data.insert_temp(id.with("last_time"), now);
        });
    } else {
        let dt = (now - last_sample_time).clamp(0.0, 0.25);
        let sample_interval = 0.035;
        let mut steps = (dt / sample_interval).floor() as usize;

        if steps > 0 {
            if steps > SAMPLES {
                steps = SAMPLES;
            }
            let prev_val = history.last().copied().unwrap_or(0.0);
            for s in 1..=steps {
                let frac = s as f32 / steps as f32;
                let interp_level = prev_val + (target_amplitude - prev_val) * (0.65 * frac);
                history.push(interp_level);
                if history.len() > SAMPLES {
                    history.remove(0);
                }
            }
            ui.memory_mut(|m| {
                m.data.insert_temp(id.with("history"), history.clone());
                m.data.insert_temp(id.with("last_time"), now);
            });
        }
    }

    let max_in_history = history.iter().copied().fold(0.0_f32, f32::max);
    if updating && (max_in_history > 0.005 || target_amplitude > 0.005) {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(16));
    }

    let (rect, _) = ui.allocate_exact_size(Vec2::new(WIDTH, HEIGHT), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let baseline = rect.center().y;
        let max_deviation = (HEIGHT * 0.5 - 0.8).max(1.0);

        let (base_color, sub_color, glow_color) = if !updating {
            (theme::border(), Color32::TRANSPARENT, Color32::TRANSPARENT)
        } else if active {
            (
                Color32::from_rgb(16, 185, 129),
                Color32::from_rgba_unmultiplied(16, 185, 129, 90),
                Color32::from_rgba_unmultiplied(16, 185, 129, 35),
            )
        } else {
            let primary = theme::primary();
            (
                primary,
                Color32::from_rgba_unmultiplied(primary.r(), primary.g(), primary.b(), 90),
                Color32::from_rgba_unmultiplied(primary.r(), primary.g(), primary.b(), 35),
            )
        };

        let anim_phase = now as f32 * 8.0;
        let mut main_points = Vec::with_capacity(RENDER_POINTS);
        let mut sub_points = Vec::with_capacity(RENDER_POINTS);

        for i in 0..RENDER_POINTS {
            let t = i as f32 / (RENDER_POINTS - 1) as f32;
            let x = rect.left() + t * rect.width();

            // Sample from rolling history envelope
            let sample_pos = t * (SAMPLES - 1) as f32;
            let idx0 = (sample_pos.floor() as usize).min(SAMPLES - 1);
            let idx1 = (idx0 + 1).min(SAMPLES - 1);
            let frac = sample_pos - sample_pos.floor();
            let smooth_frac = (1.0 - (frac * std::f32::consts::PI).cos()) * 0.5;
            let env = history[idx0] + (history[idx1] - history[idx0]) * smooth_frac;

            // Smooth Hann-like window taper at endpoints so wave gracefully terminates at baseline
            let window = (t * std::f32::consts::PI).sin().powf(0.75);

            // Wide, elegant fluid studio voice wave (2.4 cycles across full width instead of dense ripples)
            let tau = std::f32::consts::TAU;
            let wave_main = (t * 2.4 * tau - anim_phase).sin() * 0.78
                + (t * 4.8 * tau - anim_phase * 1.3).sin() * 0.22;
            let y_main = baseline + wave_main * env * window * max_deviation;
            main_points.push(egui::pos2(x, y_main));

            if max_in_history > 0.05 {
                let wave_sub = (t * 2.4 * tau - anim_phase + 1.2).sin() * 0.70
                    + (t * 4.8 * tau - anim_phase * 1.3 + 2.0).sin() * 0.20;
                let y_sub = baseline + wave_sub * env * window * (max_deviation * 0.60);
                sub_points.push(egui::pos2(x, y_sub));
            }
        }

        if max_in_history > 0.08 && glow_color != Color32::TRANSPARENT {
            painter.add(egui::Shape::line(
                main_points.clone(),
                Stroke::new(3.5, glow_color),
            ));
        }

        if !sub_points.is_empty() && sub_color != Color32::TRANSPARENT {
            painter.add(egui::Shape::line(sub_points, Stroke::new(1.2, sub_color)));
        }

        painter.add(egui::Shape::line(main_points, Stroke::new(1.8, base_color)));
    }
}

pub fn wavy_divider_black_shadow(ui: &mut Ui) {
    let available_width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(available_width, 8.0), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let y_center = rect.center().y;
        let amplitude = 2.0;
        let wavelength = 12.0;
        let stroke_width = 1.5;

        let points_shadow: Vec<egui::Pos2> = (0..=(rect.width() as i32))
            .step_by(2)
            .map(|x| {
                let x_pos = rect.min.x + x as f32;
                let phase = (x as f32 / wavelength) * std::f32::consts::TAU;
                let y_pos = y_center + phase.sin() * amplitude + 0.8;
                egui::pos2(x_pos, y_pos)
            })
            .collect();

        let points_main: Vec<egui::Pos2> = (0..=(rect.width() as i32))
            .step_by(2)
            .map(|x| {
                let x_pos = rect.min.x + x as f32;
                let phase = (x as f32 / wavelength) * std::f32::consts::TAU;
                let y_pos = y_center + phase.sin() * amplitude;
                egui::pos2(x_pos, y_pos)
            })
            .collect();

        painter.add(egui::Shape::line(
            points_shadow,
            Stroke::new(stroke_width, Color32::from_black_alpha(35)),
        ));

        painter.add(egui::Shape::line(
            points_main,
            Stroke::new(stroke_width, Color32::from_rgb(15, 23, 42)),
        ));
    }
}

pub fn wavy_divider(ui: &mut Ui, color: Color32) {
    let available_width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(available_width, 8.0), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let y_center = rect.center().y;
        let amplitude = 2.0;
        let wavelength = 12.0;
        let points: Vec<egui::Pos2> = (0..=(rect.width() as i32))
            .step_by(2)
            .map(|x| {
                let x_pos = rect.min.x + x as f32;
                let phase = (x as f32 / wavelength) * std::f32::consts::TAU;
                let y_pos = y_center + phase.sin() * amplitude;
                egui::pos2(x_pos, y_pos)
            })
            .collect();
        painter.add(egui::Shape::line(points, Stroke::new(1.5, color)));
    }
}

pub fn section_heading(ui: &mut Ui, title: &str) {
    ui.label(
        egui::RichText::new(title)
            .size(15.0)
            .color(crate::ui::theme::text_strong())
            .strong(),
    );
    ui.add_space(8.0);
}

pub fn section<R>(ui: &mut Ui, title: &str, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    ui.push_id(title, |ui| {
        let border_id = ui.make_persistent_id("organic_section_border");
        crate::ui::organic_border::show(
            ui,
            border_id,
            Frame::new()
                .fill(Color32::TRANSPARENT)
                .corner_radius(CornerRadius::same(10))
                .inner_margin(Margin::same(16))
                .shadow(egui::Shadow::NONE),
            10.0,
            theme::border(),
            |ui| {
                section_heading(ui, title);
                add_contents(ui)
            },
        )
        .inner
    })
    .inner
}

pub fn animated_button(ui: &mut Ui, text: &str) -> egui::Response {
    animated_button_enabled(ui, text, true)
}

pub fn animated_button_with_id(
    ui: &mut Ui,
    id_source: impl std::hash::Hash + std::fmt::Debug,
    text: &str,
) -> egui::Response {
    animated_button_enabled_with_id(ui, id_source, text, true)
}

/// Formats a byte count for display.
pub fn format_file_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn animated_button_enabled(ui: &mut Ui, text: &str, enabled: bool) -> egui::Response {
    animated_button_enabled_with_id(ui, text, text, enabled)
}

pub fn animated_button_enabled_with_id(
    ui: &mut Ui,
    id_source: impl std::hash::Hash + std::fmt::Debug,
    text: &str,
    enabled: bool,
) -> egui::Response {
    let id = ui.make_persistent_id(id_source);
    let is_hovered = ui.memory(|m| {
        m.data
            .get_temp::<bool>(id.with("hover_state"))
            .unwrap_or(false)
    });
    let is_active = ui.memory(|m| {
        m.data
            .get_temp::<bool>(id.with("active_state"))
            .unwrap_or(false)
    });

    let hover_factor = crate::ui::animation::AnimationSystem::hover(
        ui.ctx(),
        id.with("anim_hover"),
        is_hovered && enabled,
    );
    let active_factor = crate::ui::animation::AnimationSystem::active(
        ui.ctx(),
        id.with("anim_active"),
        is_active && enabled,
    );

    let current_time = ui.ctx().input(|i| i.time);
    let click_time = ui.memory(|m| m.data.get_temp::<f64>(id.with("click_time")).unwrap_or(0.0));
    let elapsed = (current_time - click_time) as f32;
    let click_duration = crate::ui::animation::AnimationSystem::primary_click_duration(ui.ctx());
    let is_animating_click = elapsed >= 0.0 && elapsed < click_duration;
    let click_factor = if is_animating_click {
        ui.ctx().request_repaint();
        let t = (elapsed / click_duration).clamp(0.0, 1.0);
        (1.0 - crate::ui::animation::AnimationSystem::ease_out_cubic(t)).clamp(0.0, 1.0)
    } else {
        0.0
    };

    let is_hand_drawn = theme::is_hand_drawn(ui.ctx());

    let fill = if is_hand_drawn {
        if enabled {
            let hover_bg = theme::surface_control_hover();
            let active_bg = theme::surface_control_active();
            let bg = crate::ui::animation::AnimationSystem::lerp_color(
                Color32::TRANSPARENT,
                hover_bg,
                hover_factor,
            );
            crate::ui::animation::AnimationSystem::lerp_color(bg, active_bg, active_factor)
        } else {
            Color32::TRANSPARENT
        }
    } else if enabled {
        let hover_bg = theme::surface_control_hover();
        crate::ui::animation::AnimationSystem::lerp_color(
            theme::surface_control(),
            hover_bg,
            hover_factor,
        )
    } else {
        theme::surface_control()
    };

    let text_color = if enabled {
        let base = crate::ui::animation::AnimationSystem::lerp_color(
            theme::text_strong(),
            theme::primary(),
            hover_factor,
        );
        let base = crate::ui::animation::AnimationSystem::lerp_color(
            base,
            theme::primary_dark(),
            active_factor,
        );
        crate::ui::animation::AnimationSystem::lerp_color(base, theme::primary_dark(), click_factor)
    } else {
        crate::ui::theme::text_weak()
    };

    let stroke = if is_hand_drawn {
        Stroke::NONE
    } else if enabled {
        let stroke_color = crate::ui::animation::AnimationSystem::lerp_color(
            theme::border(),
            theme::primary(),
            hover_factor * 0.4,
        );
        Stroke::new(1.0, stroke_color)
    } else {
        Stroke::new(1.0, theme::border())
    };

    let corner_radius = if is_hand_drawn {
        CornerRadius::ZERO
    } else {
        CornerRadius::same(8)
    };

    let resp = ui
        .add_enabled_ui(enabled, |ui| {
            Frame::new()
                .fill(fill)
                .stroke(stroke)
                .corner_radius(corner_radius)
                .inner_margin(Margin::symmetric(14, 7))
                .shadow(egui::Shadow::NONE)
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(text)
                            .color(text_color)
                            .size(13.0)
                            .strong(),
                    );
                })
                .response
                .interact(egui::Sense::click())
        })
        .inner;

    if is_hand_drawn && enabled && (hover_factor > 0.01 || active_factor > 0.01) {
        let line_color = crate::ui::animation::AnimationSystem::lerp_color(
            theme::border(),
            theme::primary(),
            hover_factor,
        );
        crate::ui::organic_line::paint_hand_drawn_bottom_line(
            ui.painter(),
            id.with("hand_drawn_btn_bottom"),
            resp.rect,
            Stroke::new(1.3, line_color),
        );
    }

    if resp.clicked() {
        ui.memory_mut(|m| {
            m.data.insert_temp(id.with("click_time"), current_time);
        });
        ui.ctx().request_repaint();
    }

    ui.memory_mut(|m| {
        m.data.insert_temp(id.with("hover_state"), resp.hovered());
        m.data
            .insert_temp(id.with("active_state"), resp.is_pointer_button_down_on());
    });
    if resp.hovered() && enabled {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let simulated_click = crate::ui::automation::record_button(ui, id, text, enabled, resp.rect);
    if simulated_click {
        ui.memory_mut(|m| {
            m.data.insert_temp(id.with("click_time"), current_time);
        });
        simulate_click_on(ui, resp.rect);
        ui.ctx().request_repaint();
    }
    resp
}

fn simulate_click_on(ui: &mut egui::Ui, rect: egui::Rect) {
    ui.ctx().input_mut(|i| {
        i.events.push(egui::Event::PointerButton {
            pos: rect.center(),
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::default(),
        });
        i.events.push(egui::Event::PointerButton {
            pos: rect.center(),
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        });
    });
}

pub fn primary_button(ui: &mut Ui, text: &str) -> egui::Response {
    primary_button_enabled(ui, text, true)
}

pub fn primary_button_with_id(
    ui: &mut Ui,
    id_source: impl std::hash::Hash + std::fmt::Debug,
    text: &str,
) -> egui::Response {
    primary_button_enabled_with_id(ui, id_source, text, true)
}

pub fn primary_button_enabled(ui: &mut Ui, text: &str, enabled: bool) -> egui::Response {
    primary_button_enabled_with_id(ui, text, text, enabled)
}

pub fn primary_button_enabled_with_id(
    ui: &mut Ui,
    id_source: impl std::hash::Hash + std::fmt::Debug,
    text: &str,
    enabled: bool,
) -> egui::Response {
    let id = ui.make_persistent_id(id_source);
    let is_hovered = ui.memory(|m| {
        m.data
            .get_temp::<bool>(id.with("hover_state"))
            .unwrap_or(false)
    });
    let is_active = ui.memory(|m| {
        m.data
            .get_temp::<bool>(id.with("active_state"))
            .unwrap_or(false)
    });

    let hover_factor = crate::ui::animation::AnimationSystem::hover(
        ui.ctx(),
        id.with("anim_hover"),
        is_hovered && enabled,
    );
    let active_factor = crate::ui::animation::AnimationSystem::active(
        ui.ctx(),
        id.with("anim_active"),
        is_active && enabled,
    );

    let is_hand_drawn = theme::is_hand_drawn(ui.ctx());
    let primary_rgb = theme::primary();
    let (fill, stroke, text_color) = if is_hand_drawn {
        let base_fill = Color32::TRANSPARENT;
        let hover_fill = theme::surface_control_hover();
        let active_fill = theme::surface_control_active();
        let fill = crate::ui::animation::AnimationSystem::lerp_color(base_fill, hover_fill, hover_factor);
        let fill = crate::ui::animation::AnimationSystem::lerp_color(fill, active_fill, active_factor);
        let text_color = if enabled {
            crate::ui::animation::AnimationSystem::lerp_color(
                theme::primary_dark(),
                theme::primary(),
                hover_factor,
            )
        } else {
            crate::ui::theme::text_weak()
        };
        (fill, Stroke::NONE, text_color)
    } else if enabled {
        let alpha = (0.08 + 0.12 * hover_factor + 0.10 * active_factor).clamp(0.0, 1.0);
        let border_alpha = (0.35 + 0.45 * hover_factor + 0.20 * active_factor).clamp(0.0, 1.0);
        let current_fill = Color32::from_rgba_unmultiplied(
            primary_rgb.r(),
            primary_rgb.g(),
            primary_rgb.b(),
            (alpha * 255.0) as u8,
        );
        let current_stroke = Stroke::new(
            1.0,
            Color32::from_rgba_unmultiplied(
                primary_rgb.r(),
                primary_rgb.g(),
                primary_rgb.b(),
                (border_alpha * 255.0) as u8,
            ),
        );
        let text_color = crate::ui::animation::AnimationSystem::lerp_color(
            theme::primary_dark(),
            theme::primary(),
            hover_factor,
        );
        (current_fill, current_stroke, text_color)
    } else {
        (
            Color32::TRANSPARENT,
            Stroke::new(1.0, theme::border()),
            crate::ui::theme::text_weak(),
        )
    };

    let corner_radius = if is_hand_drawn {
        CornerRadius::ZERO
    } else {
        CornerRadius::same(8)
    };

    let resp = ui
        .add_enabled_ui(enabled, |ui| {
            Frame::new()
                .fill(fill)
                .stroke(stroke)
                .corner_radius(corner_radius)
                .inner_margin(Margin::symmetric(14, 6))
                .shadow(egui::Shadow::NONE)
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(text)
                            .color(text_color)
                            .size(13.0)
                            .strong(),
                    );
                })
                .response
                .interact(egui::Sense::click())
        })
        .inner;

    if is_hand_drawn && enabled && (hover_factor > 0.01 || active_factor > 0.01) {
        let line_color = crate::ui::animation::AnimationSystem::lerp_color(
            theme::border(),
            theme::primary(),
            hover_factor,
        );
        crate::ui::organic_line::paint_hand_drawn_bottom_line(
            ui.painter(),
            id.with("primary_btn_line"),
            resp.rect,
            Stroke::new(1.3, line_color),
        );
    }

    ui.memory_mut(|m| {
        m.data.insert_temp(id.with("hover_state"), resp.hovered());
        m.data
            .insert_temp(id.with("active_state"), resp.is_pointer_button_down_on());
    });
    if resp.hovered() && enabled {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let simulated_click = crate::ui::automation::record_button(ui, id, text, enabled, resp.rect);
    if simulated_click {
        ui.ctx().memory_mut(|m| {
            m.data.insert_temp(id.with("click_time"), ui.ctx().input(|i| i.time));
        });
        ui.ctx().input_mut(|i| {
            i.events.push(egui::Event::PointerButton {
                pos: resp.rect.center(),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            });
            i.events.push(egui::Event::PointerButton {
                pos: resp.rect.center(),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            });
        });
    }
    resp
}

pub fn secondary_button(ui: &mut Ui, text: &str) -> egui::Response {
    secondary_button_enabled(ui, text, true)
}

pub fn secondary_button_enabled(ui: &mut Ui, text: &str, enabled: bool) -> egui::Response {
    let id = ui.make_persistent_id(text);
    let is_hovered = ui.memory(|m| {
        m.data
            .get_temp::<bool>(id.with("hover_state"))
            .unwrap_or(false)
    });
    let is_active = ui.memory(|m| {
        m.data
            .get_temp::<bool>(id.with("active_state"))
            .unwrap_or(false)
    });

    let hover_factor = crate::ui::animation::AnimationSystem::hover(
        ui.ctx(),
        id.with("anim_hover"),
        is_hovered && enabled,
    );
    let active_factor = crate::ui::animation::AnimationSystem::active(
        ui.ctx(),
        id.with("anim_active"),
        is_active && enabled,
    );

    let is_hand_drawn = theme::is_hand_drawn(ui.ctx());
    let (fill, stroke, text_color) = if is_hand_drawn {
        let base_fill = Color32::TRANSPARENT;
        let hover_fill = theme::surface_control_hover();
        let active_fill = theme::surface_control_active();
        let fill = crate::ui::animation::AnimationSystem::lerp_color(base_fill, hover_fill, hover_factor);
        let fill = crate::ui::animation::AnimationSystem::lerp_color(fill, active_fill, active_factor);
        let text_color = if enabled {
            crate::ui::animation::AnimationSystem::lerp_color(
                theme::text_strong(),
                theme::primary_dark(),
                hover_factor * 0.7 + active_factor * 0.3,
            )
        } else {
            crate::ui::theme::text_weak()
        };
        (fill, Stroke::NONE, text_color)
    } else if enabled {
        let base_fill = theme::surface_control();
        let hover_fill = theme::surface_control_hover();
        let active_fill = theme::surface_control_active();
        let fill = crate::ui::animation::AnimationSystem::lerp_color(base_fill, hover_fill, hover_factor);
        let fill = crate::ui::animation::AnimationSystem::lerp_color(fill, active_fill, active_factor);

        let stroke_color = Color32::from_rgba_unmultiplied(
            188,
            198,
            201,
            ((0.25 + 0.35 * hover_factor) * 255.0) as u8,
        );
        let stroke = Stroke::new(1.0, stroke_color);

        let text_color = crate::ui::animation::AnimationSystem::lerp_color(
            theme::text_strong(),
            theme::primary_dark(),
            hover_factor * 0.7 + active_factor * 0.3,
        );
        (fill, stroke, text_color)
    } else {
        (
            theme::surface_control(),
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(188, 198, 201, 40)),
            crate::ui::theme::text_weak(),
        )
    };

    let corner_radius = if is_hand_drawn {
        CornerRadius::ZERO
    } else {
        CornerRadius::same(8)
    };

    let resp = ui
        .add_enabled_ui(enabled, |ui| {
            Frame::new()
                .fill(fill)
                .stroke(stroke)
                .corner_radius(corner_radius)
                .inner_margin(Margin::symmetric(14, 6))
                .shadow(egui::Shadow::NONE)
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(text)
                            .color(text_color)
                            .size(13.0)
                            .strong(),
                    );
                })
                .response
                .interact(egui::Sense::click())
        })
        .inner;

    if is_hand_drawn && enabled && (hover_factor > 0.01 || active_factor > 0.01) {
        let stroke_color = crate::ui::animation::AnimationSystem::lerp_color(
            theme::border(),
            theme::primary(),
            hover_factor,
        );
        crate::ui::organic_line::paint_hand_drawn_bottom_line(
            ui.painter(),
            id.with("sec_btn_line"),
            resp.rect,
            Stroke::new(1.3, stroke_color),
        );
    }

    ui.memory_mut(|m| {
        m.data.insert_temp(id.with("hover_state"), resp.hovered());
        m.data
            .insert_temp(id.with("active_state"), resp.is_pointer_button_down_on());
    });
    if resp.hovered() && enabled {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let simulated_click = crate::ui::automation::record_button(ui, id, text, enabled, resp.rect);
    if simulated_click {
        simulate_click_on(ui, resp.rect);
        ui.ctx().request_repaint();
    }
    resp
}

pub fn combobox_ui<R>(
    ui: &mut Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    selected_text: impl Into<String>,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> egui::InnerResponse<Option<R>> {
    combobox_ui_with_width(ui, id_salt, selected_text, None, add_contents)
}

pub fn combobox_ui_with_width<R>(
    ui: &mut Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    selected_text: impl Into<String>,
    width: Option<f32>,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> egui::InnerResponse<Option<R>> {
    let is_hand_drawn = crate::ui::theme::is_hand_drawn(ui.ctx());
    let combo_id = ui.make_persistent_id(&id_salt);
    let selected_text = selected_text.into();
    let selected_str = selected_text.clone();
    let control_width = crate::ui::layout::control_width(ui, &selected_text, width, 96.0, 240.0);

    let is_hovered = ui.memory(|m| {
        m.data
            .get_temp::<bool>(combo_id.with("hover_state"))
            .unwrap_or(false)
    });
    let hover_factor = crate::ui::animation::AnimationSystem::hover(
        ui.ctx(),
        combo_id.with("anim_hover"),
        is_hovered,
    );

    let inner_resp = ui.scope(|ui| {
        if is_hand_drawn {
            ui.style_mut().visuals.widgets.inactive.bg_stroke = egui::Stroke::NONE;
            ui.style_mut().visuals.widgets.inactive.bg_fill = egui::Color32::TRANSPARENT;
            ui.style_mut().visuals.widgets.hovered.bg_stroke = egui::Stroke::NONE;
            ui.style_mut().visuals.widgets.hovered.bg_fill =
                crate::ui::theme::surface_control_hover();
            ui.style_mut().visuals.widgets.active.bg_stroke = egui::Stroke::NONE;
            ui.style_mut().visuals.widgets.active.bg_fill =
                crate::ui::theme::surface_control_active();
            ui.style_mut().visuals.widgets.open.bg_stroke = egui::Stroke::NONE;
            ui.style_mut().visuals.widgets.open.bg_fill = egui::Color32::TRANSPARENT;
            ui.style_mut().visuals.widgets.inactive.corner_radius = egui::CornerRadius::ZERO;
            ui.style_mut().visuals.widgets.hovered.corner_radius = egui::CornerRadius::ZERO;
            ui.style_mut().visuals.widgets.active.corner_radius = egui::CornerRadius::ZERO;
            ui.style_mut().visuals.widgets.open.corner_radius = egui::CornerRadius::ZERO;
            ui.spacing_mut().button_padding = egui::vec2(6.0, 4.0);
        }

        let combo = egui::ComboBox::from_id_salt(&id_salt)
            .selected_text(
                egui::RichText::new(selected_text)
                    .size(12.0)
                    .color(crate::ui::theme::text_strong()),
            )
            .width(control_width)
            .truncate()
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);

        combo.show_ui(ui, add_contents)
    });

    let mut resp = inner_resp.inner;
    resp.response = resp.response.on_hover_text(&selected_str);
    crate::ui::automation::record_combobox(
        ui,
        combo_id,
        &selected_str,
        &selected_str,
        true,
        resp.response.rect,
    );
    let hovered = resp.response.hovered() || resp.response.is_pointer_button_down_on();
    ui.memory_mut(|m| {
        m.data.insert_temp(combo_id.with("hover_state"), hovered);
    });

    if is_hand_drawn {
        let stroke_color = crate::ui::animation::AnimationSystem::lerp_color(
            crate::ui::theme::border(),
            crate::ui::theme::primary(),
            hover_factor,
        );
        crate::ui::organic_line::paint_hand_drawn_bottom_line(
            ui.painter(),
            combo_id.with("bottom_line"),
            resp.response.rect,
            egui::Stroke::new(1.3, stroke_color),
        );
        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }

    resp
}

pub fn searchable_combobox<T: PartialEq + Clone>(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    selected_text: impl Into<String>,
    selected: &mut T,
    options: &[(T, String)],
) -> bool {
    searchable_combobox_with_width(ui, id, selected_text, selected, options, None)
}

pub fn searchable_combobox_with_width<T: PartialEq + Clone>(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    selected_text: impl Into<String>,
    selected: &mut T,
    options: &[(T, String)],
    width: Option<f32>,
) -> bool {
    searchable_combobox_with_options(ui, id, selected_text, selected, options, width, true)
}

pub fn searchable_combobox_frameless<T: PartialEq + Clone>(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    selected_text: impl Into<String>,
    selected: &mut T,
    options: &[(T, String)],
    width: Option<f32>,
) -> bool {
    searchable_combobox_with_options(ui, id, selected_text, selected, options, width, false)
}

pub fn searchable_combobox_with_options<T: PartialEq + Clone>(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    selected_text: impl Into<String>,
    selected: &mut T,
    options: &[(T, String)],
    width: Option<f32>,
    frame: bool,
) -> bool {
    let is_hand_drawn = crate::ui::theme::is_hand_drawn(ui.ctx());
    let combo_id = ui.make_persistent_id(&id);
    let search_id = combo_id.with("combo_search");
    let selected_text = selected_text.into();
    let control_width = crate::ui::layout::control_width(ui, &selected_text, width, 96.0, 240.0);

    let is_hovered = ui.memory(|m| {
        m.data
            .get_temp::<bool>(combo_id.with("hover_state"))
            .unwrap_or(false)
    });
    let hover_factor = crate::ui::animation::AnimationSystem::hover(
        ui.ctx(),
        combo_id.with("anim_hover"),
        is_hovered,
    );

    let mut changed = false;
    let inner_resp = ui.scope(|ui| {
        if is_hand_drawn && frame {
            ui.style_mut().visuals.widgets.inactive.bg_stroke = egui::Stroke::NONE;
            ui.style_mut().visuals.widgets.inactive.bg_fill = egui::Color32::TRANSPARENT;
            ui.style_mut().visuals.widgets.hovered.bg_stroke = egui::Stroke::NONE;
            ui.style_mut().visuals.widgets.hovered.bg_fill =
                crate::ui::theme::surface_control_hover();
            ui.style_mut().visuals.widgets.active.bg_stroke = egui::Stroke::NONE;
            ui.style_mut().visuals.widgets.active.bg_fill =
                crate::ui::theme::surface_control_active();
            ui.style_mut().visuals.widgets.open.bg_stroke = egui::Stroke::NONE;
            ui.style_mut().visuals.widgets.open.bg_fill = egui::Color32::TRANSPARENT;
            ui.style_mut().visuals.widgets.inactive.corner_radius = egui::CornerRadius::ZERO;
            ui.style_mut().visuals.widgets.hovered.corner_radius = egui::CornerRadius::ZERO;
            ui.style_mut().visuals.widgets.active.corner_radius = egui::CornerRadius::ZERO;
            ui.style_mut().visuals.widgets.open.corner_radius = egui::CornerRadius::ZERO;
            ui.spacing_mut().button_padding = egui::vec2(6.0, 4.0);
        } else if !frame {
            ui.style_mut().visuals.widgets.inactive.bg_stroke = egui::Stroke::NONE;
            ui.style_mut().visuals.widgets.inactive.bg_fill = egui::Color32::TRANSPARENT;
            ui.style_mut().visuals.widgets.hovered.bg_stroke = egui::Stroke::NONE;
            ui.style_mut().visuals.widgets.hovered.bg_fill =
                crate::ui::theme::surface_control_hover();
            ui.style_mut().visuals.widgets.active.bg_stroke = egui::Stroke::NONE;
            ui.style_mut().visuals.widgets.active.bg_fill =
                crate::ui::theme::surface_control_active();
            ui.style_mut().visuals.widgets.open.bg_stroke = egui::Stroke::NONE;
            ui.style_mut().visuals.widgets.open.bg_fill = egui::Color32::TRANSPARENT;
            ui.spacing_mut().button_padding = egui::vec2(4.0, 2.0);
        }

        let combo = egui::ComboBox::from_id_salt(&id)
            .selected_text(
                egui::RichText::new(&selected_text)
                    .size(12.0)
                    .color(crate::ui::theme::text_strong()),
            )
            .width(control_width)
            .truncate()
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);

        combo.show_ui(ui, |ui| {
            let is_more_than_3 = options.len() > 3;

            let mut search_query = if is_more_than_3 {
                ui.memory(|m| m.data.get_temp::<String>(search_id).unwrap_or_default())
            } else {
                String::new()
            };

            if is_more_than_3 {
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.add_space(2.0);
                    let te = egui::TextEdit::singleline(&mut search_query)
                        .hint_text("Search...")
                        .desired_width(130.0)
                        .margin(Margin::symmetric(6, 4));
                    text_edit_ui(ui, search_id.with("inner_search_edit"), te);
                    ui.add_space(2.0);
                });
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(2.0);

                ui.memory_mut(|m| m.data.insert_temp(search_id, search_query.clone()));
            }

            let query_lower = search_query.trim().to_lowercase();
            let mut match_count = 0;

            for (val, label) in options {
                if !query_lower.is_empty() && !label.to_lowercase().contains(&query_lower) {
                    continue;
                }
                match_count += 1;
                if ui.selectable_value(selected, val.clone(), label).clicked() {
                    changed = true;
                    if is_more_than_3 {
                        ui.memory_mut(|m| m.data.insert_temp(search_id, String::new()));
                    }
                }
            }

            if is_more_than_3 && match_count == 0 {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("No matching items")
                        .size(12.0)
                        .color(crate::ui::theme::text_weak()),
                );
                ui.add_space(4.0);
            }
        })
    });

    let resp = inner_resp.inner;
    if let Some(target_text) = crate::ui::automation::record_combobox(
        ui,
        combo_id,
        &selected_text,
        &selected_text,
        true,
        resp.response.rect,
    ) {
        for (val, label) in options {
            if label.eq_ignore_ascii_case(&target_text)
                || label.to_lowercase().contains(&target_text.to_lowercase())
            {
                *selected = val.clone();
                changed = true;
                break;
            }
        }
    }
    let hovered = resp.response.hovered() || resp.response.is_pointer_button_down_on();
    ui.memory_mut(|m| {
        m.data.insert_temp(combo_id.with("hover_state"), hovered);
    });

    if is_hand_drawn && frame {
        let stroke_color = crate::ui::animation::AnimationSystem::lerp_color(
            crate::ui::theme::border(),
            crate::ui::theme::primary(),
            hover_factor,
        );
        crate::ui::organic_line::paint_hand_drawn_bottom_line(
            ui.painter(),
            combo_id.with("bottom_line"),
            resp.response.rect,
            egui::Stroke::new(1.3, stroke_color),
        );
        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }

    changed
}

pub fn search_bar(ui: &mut Ui, query: &mut String, hint: &str) -> bool {
    let is_hand_drawn = theme::is_hand_drawn(ui.ctx());
    let id = ui.make_persistent_id("search_bar_comp");
    let is_hovered = ui.memory(|m| {
        m.data
            .get_temp::<bool>(id.with("hover_state"))
            .unwrap_or(false)
    });
    let hover_factor =
        crate::ui::animation::AnimationSystem::hover(ui.ctx(), id.with("anim_hover"), is_hovered);
    let mut changed = false;
    let has_query = !query.is_empty();

    let frame = if is_hand_drawn {
        Frame::new()
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::ZERO)
            .inner_margin(Margin::symmetric(8, 6))
    } else {
        Frame::new()
            .fill(theme::surface_control())
            .stroke(Stroke::new(1.0, theme::border()))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::symmetric(12, 8))
    };

    let inner_resp = frame.show(ui, |ui| {
        ui.horizontal(|ui| {
            let text_frame = Frame::new()
                .fill(Color32::TRANSPARENT)
                .stroke(Stroke::NONE)
                .corner_radius(CornerRadius::ZERO)
                .inner_margin(Margin::ZERO);
            let response = ui.add(
                egui::TextEdit::singleline(query)
                    .hint_text(hint)
                    .frame(text_frame)
                    .margin(Margin::symmetric(0, 0))
                    .desired_width(ui.available_width() - if has_query { 24.0 } else { 0.0 }),
            );
            if response.changed() {
                changed = true;
            }
            if !query.is_empty() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let clear_btn = Frame::new()
                        .fill(Color32::from_black_alpha(10))
                        .corner_radius(CornerRadius::same(6))
                        .inner_margin(Margin::symmetric(6, 2))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new("×")
                                    .color(Color32::from_rgb(100, 116, 139))
                                    .size(12.0)
                                    .strong(),
                            )
                        })
                        .response
                        .interact(egui::Sense::click());
                    if clear_btn.clicked() {
                        query.clear();
                        changed = true;
                    }
                });
            }
            response
        }).inner
    });

    let rect = inner_resp.response.rect;
    let has_focus = inner_resp.inner.has_focus();
    let hovered = inner_resp.response.hovered() || inner_resp.inner.hovered();
    ui.memory_mut(|m| {
        m.data.insert_temp(id.with("hover_state"), hovered || has_focus);
    });

    if is_hand_drawn {
        let stroke_color = if has_focus {
            theme::primary()
        } else {
            crate::ui::animation::AnimationSystem::lerp_color(
                theme::border(),
                theme::primary(),
                hover_factor,
            )
        };
        crate::ui::organic_line::paint_hand_drawn_bottom_line(
            ui.painter(),
            id.with("bottom_line"),
            rect,
            Stroke::new(if has_focus { 1.5 } else { 1.3 }, stroke_color),
        );
    }

    if let Some(new_query) = crate::ui::automation::record_text_input(
        ui,
        id,
        if hint.is_empty() { "search" } else { hint },
        query,
        true,
        rect,
    ) {
        *query = new_query;
        changed = true;
    }

    changed
}

pub fn input_field(ui: &mut Ui, text: &mut String, hint: &str) -> egui::Response {
    let edit = egui::TextEdit::singleline(text)
        .hint_text(hint)
        .margin(egui::vec2(8.0, 6.0))
        .desired_width(ui.available_width());
    let id_salt = ui.next_auto_id().with("input_field");
    let id = ui.make_persistent_id(&id_salt);
    let mut resp = text_edit_ui(ui, id_salt, edit);
    if let Some(new_text) = crate::ui::automation::record_text_input(
        ui,
        id,
        if hint.is_empty() { "input" } else { hint },
        text,
        true,
        resp.rect,
    ) {
        *text = new_text;
        resp.mark_changed();
    }
    resp
}

pub fn error_notice(ui: &mut Ui, language: crate::i18n::UiLanguage, details: &str) {
    render_error_notice(ui, language, details, false);
}

pub fn dismissible_error_notice(
    ui: &mut Ui,
    language: crate::i18n::UiLanguage,
    details: &str,
) -> bool {
    render_error_notice(ui, language, details, true)
}

fn render_error_notice(
    ui: &mut Ui,
    language: crate::i18n::UiLanguage,
    details: &str,
    dismissible: bool,
) -> bool {
    let mut dismissed = false;
    Frame::new()
        .fill(crate::ui::theme::modal_backdrop())
        .stroke(Stroke::new(1.0, crate::ui::theme::danger()))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(crate::i18n::tr(language, "Something went wrong"))
                        .strong()
                        .color(crate::ui::theme::text_strong()),
                );
                if dismissible {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        dismissed = ui
                            .small_button("×")
                            .on_hover_text(crate::i18n::tr(language, "Close"))
                            .clicked();
                    });
                }
            });
            ui.label(
                egui::RichText::new(crate::i18n::tr(
                    language,
                    "You can copy the details and report this on GitHub Issues or in QQ group 1009732148.",
                ))
                .size(12.5)
                .color(crate::ui::theme::text_normal()),
            );
            ui.horizontal_wrapped(|ui| {
                error_actions(ui, language, details);
            });
            egui::CollapsingHeader::new(crate::i18n::tr(language, "Error details"))
                .id_salt(details)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(160.0)
                        .show(ui, |ui| {
                            dark_container_frame(ui, |ui| {
                                ui.label(
                                    egui::RichText::new(details)
                                        .family(egui::FontFamily::Monospace)
                                        .color(Color32::from_rgb(240, 244, 255))
                                        .size(12.0),
                                );
                            });
                        });
                });
        });
    dismissed
}

pub fn error_actions(ui: &mut Ui, language: crate::i18n::UiLanguage, details: &str) {
    if secondary_button(ui, crate::i18n::tr(language, "Copy details")).clicked() {
        ui.ctx().copy_text(details.to_owned());
    }
    ui.hyperlink_to(
        crate::i18n::tr(language, "GitHub Issues"),
        "https://github.com/NowLoadY/XRTranslate/issues/new",
    );
}

pub fn validation_notice(ui: &mut Ui, language: crate::i18n::UiLanguage, details: &str) {
    Frame::new()
        .fill(crate::ui::theme::surface_subtle())
        .stroke(Stroke::new(1.0, crate::ui::theme::danger()))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(details).color(crate::ui::theme::danger()));
                if secondary_button(ui, crate::i18n::tr(language, "Copy details")).clicked() {
                    ui.ctx().copy_text(details.to_owned());
                }
            });
        });
}

pub fn translation_language_selector(
    ui: &mut Ui,
    id: &str,
    source: &mut String,
    target: &mut String,
    capabilities: xrtranslate_engine::language::LanguageCapabilities,
    language: crate::i18n::UiLanguage,
) -> bool {
    let tr = |label| crate::i18n::tr(language, label).to_string();
    let label = |code: &str| crate::language_label(language, code).to_string();
    let mut input = if source == "auto" && target.contains(',') {
        "auto-pair".to_owned()
    } else {
        source.clone()
    };
    let mut options = vec![("auto".to_owned(), tr("Auto Detect"))];
    if capabilities.change_input("auto", target, true).is_ok() {
        options.push(("auto-pair".to_owned(), tr("Auto (bidirectional)")));
    }
    options.extend(
        capabilities
            .sources()
            .iter()
            .map(|item| (item.code().to_owned(), tr(item.name()))),
    );
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        let selected = if input == "auto-pair" {
            tr("Auto (bidirectional)")
        } else {
            options
                .iter()
                .find(|(code, _)| code == &input)
                .map(|(_, label)| label.clone())
                .unwrap_or_else(|| label(&input))
        };
        if searchable_combobox(ui, format!("{id}_source"), selected, &mut input, &options) {
            let pair = input == "auto-pair";
            let next_source = if pair { "auto" } else { &input };
            if let Ok(selection) = capabilities.change_input(next_source, target, pair) {
                (*source, *target) = selection.wire();
                changed = true;
            }
        }
        let pair = source == "auto" && target.contains(',');
        if source != "auto"
            && ui
                .push_id(id, |ui| {
                    swap_capsule_button(ui, capabilities.select(target, source).is_ok()).clicked()
                })
                .inner
        {
            std::mem::swap(source, target);
            changed = true;
        }
        let available = if pair {
            capabilities.sources()
        } else {
            capabilities.targets()
        };
        let mut targets: Vec<String> = target.split(',').map(str::to_owned).collect();
        for index in 0..targets.len() {
            ui.label(if pair { "↔" } else { "→" });
            let options: Vec<_> = available
                .iter()
                .filter(|item| {
                    if pair {
                        targets.iter().enumerate().all(|(other, code)| {
                            other == index || !crate::languages_conflict(item.code(), code)
                        })
                    } else {
                        item.code() != source
                    }
                })
                .map(|item| (item.code().to_owned(), tr(item.name())))
                .collect();
            if searchable_combobox(
                ui,
                format!("{id}_target_{index}"),
                label(&targets[index]),
                &mut targets[index],
                &options,
            ) {
                *target = targets.join(",");
                changed = true;
            }
        }
    });
    if let Err(error) = capabilities.select(source, target) {
        validation_notice(ui, language, &error);
    }
    changed
}

pub fn language_selector(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    language: &mut crate::i18n::UiLanguage,
) -> bool {
    let options: Vec<_> = crate::i18n::UiLanguage::ALL
        .into_iter()
        .map(|lang| (lang, lang.display_name().to_string()))
        .collect();
    let current_text = language.display_name().to_string();
    searchable_combobox(ui, id, current_text, language, &options)
}

pub fn danger_button(ui: &mut Ui, text: &str) -> egui::Response {
    danger_button_enabled(ui, text, true)
}

pub fn danger_button_with_id(
    ui: &mut Ui,
    id_source: impl std::hash::Hash + std::fmt::Debug,
    text: &str,
) -> egui::Response {
    danger_button_enabled_with_id(ui, id_source, text, true)
}

pub fn danger_button_enabled(ui: &mut Ui, text: &str, enabled: bool) -> egui::Response {
    danger_button_enabled_with_id(ui, text, text, enabled)
}

pub fn danger_button_enabled_with_id(
    ui: &mut Ui,
    id_source: impl std::hash::Hash + std::fmt::Debug,
    text: &str,
    enabled: bool,
) -> egui::Response {
    let id = ui.make_persistent_id(id_source);
    let is_hovered = ui.memory(|m| {
        m.data
            .get_temp::<bool>(id.with("hover_state"))
            .unwrap_or(false)
    });
    let is_active = ui.memory(|m| {
        m.data
            .get_temp::<bool>(id.with("active_state"))
            .unwrap_or(false)
    });

    let hover_factor = crate::ui::animation::AnimationSystem::hover(
        ui.ctx(),
        id.with("anim_hover"),
        is_hovered && enabled,
    );
    let active_factor = crate::ui::animation::AnimationSystem::active(
        ui.ctx(),
        id.with("anim_active"),
        is_active && enabled,
    );

    let is_hand_drawn = theme::is_hand_drawn(ui.ctx());
    let danger_rgb = (239, 68, 68);
    let (fill, stroke, text_color) = if is_hand_drawn {
        let base_fill = Color32::TRANSPARENT;
        let hover_fill = Color32::from_rgba_unmultiplied(danger_rgb.0, danger_rgb.1, danger_rgb.2, 25);
        let active_fill = Color32::from_rgba_unmultiplied(danger_rgb.0, danger_rgb.1, danger_rgb.2, 45);
        let fill = crate::ui::animation::AnimationSystem::lerp_color(base_fill, hover_fill, hover_factor);
        let fill = crate::ui::animation::AnimationSystem::lerp_color(fill, active_fill, active_factor);
        let text_color = if enabled {
            crate::ui::animation::AnimationSystem::lerp_color(
                Color32::from_rgb(185, 28, 28),
                Color32::from_rgb(220, 38, 38),
                hover_factor,
            )
        } else {
            crate::ui::theme::text_weak()
        };
        (fill, Stroke::NONE, text_color)
    } else if enabled {
        let alpha = (0.08 + 0.12 * hover_factor + 0.10 * active_factor).clamp(0.0, 1.0);
        let border_alpha = (0.35 + 0.45 * hover_factor + 0.20 * active_factor).clamp(0.0, 1.0);
        let current_fill = Color32::from_rgba_unmultiplied(
            danger_rgb.0,
            danger_rgb.1,
            danger_rgb.2,
            (alpha * 255.0) as u8,
        );
        let current_stroke = Stroke::new(
            1.0,
            Color32::from_rgba_unmultiplied(
                danger_rgb.0,
                danger_rgb.1,
                danger_rgb.2,
                (border_alpha * 255.0) as u8,
            ),
        );
        let text_color = crate::ui::animation::AnimationSystem::lerp_color(
            Color32::from_rgb(185, 28, 28),
            Color32::from_rgb(220, 38, 38),
            hover_factor,
        );
        (current_fill, current_stroke, text_color)
    } else {
        (
            Color32::TRANSPARENT,
            Stroke::new(1.0, theme::border()),
            crate::ui::theme::text_weak(),
        )
    };

    let corner_radius = if is_hand_drawn {
        CornerRadius::ZERO
    } else {
        CornerRadius::same(8)
    };

    let resp = ui
        .add_enabled_ui(enabled, |ui| {
            Frame::new()
                .fill(fill)
                .stroke(stroke)
                .corner_radius(corner_radius)
                .inner_margin(Margin::symmetric(14, 6))
                .shadow(egui::Shadow::NONE)
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(text)
                            .color(text_color)
                            .size(13.0)
                            .strong(),
                    );
                })
                .response
                .interact(egui::Sense::click())
        })
        .inner;

    if is_hand_drawn && enabled && (hover_factor > 0.01 || active_factor > 0.01) {
        let line_color = crate::ui::animation::AnimationSystem::lerp_color(
            Color32::from_rgb(185, 28, 28),
            Color32::from_rgb(220, 38, 38),
            hover_factor,
        );
        crate::ui::organic_line::paint_hand_drawn_bottom_line(
            ui.painter(),
            id.with("danger_btn_line"),
            resp.rect,
            Stroke::new(1.3, line_color),
        );
    }

    ui.memory_mut(|m| {
        m.data.insert_temp(id.with("hover_state"), resp.hovered());
        m.data
            .insert_temp(id.with("active_state"), resp.is_pointer_button_down_on());
    });
    if resp.hovered() && enabled {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let simulated_click = crate::ui::automation::record_button(ui, id, text, enabled, resp.rect);
    if simulated_click {
        simulate_click_on(ui, resp.rect);
        ui.ctx().request_repaint();
    }
    resp
}

pub fn pill_toggle(ui: &mut Ui, checked: &mut bool) -> egui::Response {
    let id = ui.next_auto_id();
    let is_hovered = ui.memory(|m| {
        m.data
            .get_temp::<bool>(id.with("hover_state"))
            .unwrap_or(false)
    });
    let is_active = ui.memory(|m| {
        m.data
            .get_temp::<bool>(id.with("active_state"))
            .unwrap_or(false)
    });

    let hover_factor =
        crate::ui::animation::AnimationSystem::hover(ui.ctx(), id.with("hover"), is_hovered);
    let active_factor =
        crate::ui::animation::AnimationSystem::active(ui.ctx(), id.with("active"), is_active);
    let switch_factor =
        crate::ui::animation::AnimationSystem::toggle(ui.ctx(), id.with("switch"), *checked);

    let (rect, mut response) = ui.allocate_exact_size(Vec2::new(36.0, 20.0), egui::Sense::click());
    if response.clicked() {
        *checked = !*checked;
        response.mark_changed();
    }
    if let Some(new_val) =
        crate::ui::automation::record_toggle(ui, id, "toggle", *checked, true, rect)
    {
        *checked = new_val;
        response.mark_changed();
    }

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let is_hand_drawn = theme::is_hand_drawn(ui.ctx());

        let interaction_color = crate::ui::animation::AnimationSystem::lerp_color(
            theme::border_strong(),
            theme::primary(),
            hover_factor,
        );
        let interaction_color = crate::ui::animation::AnimationSystem::lerp_color(
            interaction_color,
            theme::primary_dark(),
            active_factor,
        );
        let track_stroke = crate::ui::animation::AnimationSystem::lerp_color(
            interaction_color,
            theme::primary_dark(),
            switch_factor,
        );

        let knob_radius = 7.0;
        let min_x = rect.min.x + 9.5;
        let max_x = rect.max.x - 9.5;
        let current_x = min_x + (max_x - min_x) * switch_factor;
        let knob_center = egui::pos2(current_x, rect.center().y);
        let knob_color = crate::ui::animation::AnimationSystem::lerp_color(
            interaction_color,
            theme::primary(),
            switch_factor,
        );

        if is_hand_drawn {
            if switch_factor > 0.01 {
                let fill_alpha = (35.0 * switch_factor) as u8;
                painter.rect_filled(
                    rect,
                    CornerRadius::same(5),
                    Color32::from_rgba_unmultiplied(37, 99, 235, fill_alpha),
                );
            }
            crate::ui::organic_line::paint_hand_drawn_rect(
                painter,
                id.with("track"),
                rect,
                Stroke::new(1.2, track_stroke),
            );

            if switch_factor > 0.5 {
                painter.circle_filled(knob_center, knob_radius, knob_color);
                crate::ui::organic_line::paint_hand_drawn_circle(
                    painter,
                    id.with("knob"),
                    knob_center,
                    knob_radius,
                    Stroke::new(1.0, theme::primary_dark()),
                );
            } else {
                crate::ui::organic_line::paint_hand_drawn_circle(
                    painter,
                    id.with("knob"),
                    knob_center,
                    knob_radius,
                    Stroke::new(1.3, track_stroke),
                );
            }
        } else {
            painter.rect_stroke(
                rect,
                CornerRadius::same(10),
                Stroke::new(1.0, track_stroke),
                egui::StrokeKind::Inside,
            );
            painter.circle_filled(knob_center, knob_radius, knob_color);
        }
    }

    ui.memory_mut(|m| {
        m.data
            .insert_temp(id.with("hover_state"), response.hovered());
        m.data.insert_temp(
            id.with("active_state"),
            response.is_pointer_button_down_on(),
        );
    });
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    response
}

/// Minimalist futuristic tech numeric readout (frameless, transparent background, pure crisp typography).
pub fn tech_numeric_badge(ui: &mut Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .font(egui::FontId::monospace(12.0))
            .color(theme::text_strong())
            .strong(),
    );
}

pub fn checkbox(
    ui: &mut Ui,
    checked: &mut bool,
    text: impl Into<egui::WidgetText>,
) -> egui::Response {
    let id = ui.next_auto_id();
    let text = text.into();
    let is_hand_drawn = theme::is_hand_drawn(ui.ctx());

    if !is_hand_drawn {
        let label_text = text.text().to_string();
        let mut resp = ui.checkbox(checked, text);
        if let Some(new_val) = crate::ui::automation::record_checkbox(
            ui,
            resp.id,
            &label_text,
            *checked,
            true,
            resp.rect,
        ) {
            *checked = new_val;
            resp.mark_changed();
        }
        return resp;
    }

    let is_hovered = ui.memory(|m| {
        m.data
            .get_temp::<bool>(id.with("hover_state"))
            .unwrap_or(false)
    });
    let hover_factor =
        crate::ui::animation::AnimationSystem::hover(ui.ctx(), id.with("hover"), is_hovered);
    let check_factor =
        crate::ui::animation::AnimationSystem::toggle(ui.ctx(), id.with("toggle"), *checked);

    let font_id = egui::TextStyle::Body.resolve(ui.style());
    let galley = text.into_galley(ui, None, f32::INFINITY, font_id);

    let box_size = 15.0;
    let spacing = 6.0;
    let total_width = box_size + if galley.text().is_empty() { 0.0 } else { spacing + galley.size().x };
    let total_height = box_size.max(galley.size().y);

    let (rect, mut response) = ui.allocate_exact_size(Vec2::new(total_width, total_height), egui::Sense::click());
    if response.clicked() {
        *checked = !*checked;
        response.mark_changed();
    }
    if let Some(new_val) = crate::ui::automation::record_checkbox(
        ui,
        id,
        &galley.text().to_string(),
        *checked,
        true,
        rect,
    ) {
        *checked = new_val;
        response.mark_changed();
    }

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let box_rect = egui::Rect::from_min_size(
            egui::pos2(rect.min.x, rect.center().y - box_size / 2.0),
            Vec2::splat(box_size),
        );

        let border_color = crate::ui::animation::AnimationSystem::lerp_color(
            theme::border_strong(),
            theme::primary(),
            hover_factor,
        );
        let border_color = crate::ui::animation::AnimationSystem::lerp_color(
            border_color,
            theme::primary_dark(),
            check_factor,
        );

        if check_factor > 0.01 {
            let bg_alpha = (35.0 * check_factor) as u8;
            painter.rect_filled(
                box_rect,
                CornerRadius::same(2),
                Color32::from_rgba_unmultiplied(37, 99, 235, bg_alpha),
            );
        }

        crate::ui::organic_line::paint_hand_drawn_rect(
            painter,
            id.with("box"),
            box_rect,
            Stroke::new(1.2, border_color),
        );

        if check_factor > 0.05 {
            let check_color =
                Color32::from_rgba_unmultiplied(37, 99, 235, (255.0 * check_factor) as u8);
            crate::ui::organic_line::paint_hand_drawn_checkmark(
                painter,
                id.with("check"),
                box_rect,
                Stroke::new(1.6, check_color),
            );
        }

        if !galley.text().is_empty() {
            let text_pos = egui::pos2(
                box_rect.max.x + spacing,
                rect.center().y - galley.size().y / 2.0,
            );
            painter.galley(text_pos, galley, theme::text_strong());
        }
    }

    ui.memory_mut(|m| {
        m.data
            .insert_temp(id.with("hover_state"), response.hovered());
    });
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    response
}

pub fn toggle_with_label(ui: &mut Ui, checked: &mut bool, label: &str) -> egui::Response {
    let id = ui.make_persistent_id(label);
    ui.horizontal(|ui| {
        let mut resp = pill_toggle(ui, checked);
        if let Some(new_val) =
            crate::ui::automation::record_toggle(ui, id, label, *checked, true, resp.rect)
        {
            *checked = new_val;
            resp.mark_changed();
        }
        ui.add_space(4.0);
        let text_resp = ui.label(
            egui::RichText::new(label)
                .color(crate::ui::theme::text_strong())
                .size(13.0),
        );
        if text_resp.clicked() {
            *checked = !*checked;
            resp.mark_changed();
        }
        resp
    })
    .inner
}

pub fn download_mirror_toggle(
    ui: &mut Ui,
    language: crate::i18n::UiLanguage,
    checked: &mut bool,
) -> egui::Response {
    toggle_with_label(ui, checked, crate::i18n::tr(language, "Use mirror")).on_hover_text(
        crate::i18n::tr(
            language,
            "Route supported GitHub and Hugging Face downloads through an alternate mirror.",
        ),
    )
}

pub fn resource_delete_button(
    ui: &mut Ui,
    id_source: impl std::hash::Hash + std::fmt::Debug,
    language: crate::i18n::UiLanguage,
) -> egui::Response {
    let id = ui.make_persistent_id(("resource_delete_btn", id_source));
    let is_hovered = ui.memory(|m| {
        m.data
            .get_temp::<bool>(id.with("hover_state"))
            .unwrap_or(false)
    });
    let is_active = ui.memory(|m| {
        m.data
            .get_temp::<bool>(id.with("active_state"))
            .unwrap_or(false)
    });
    let hover_factor =
        crate::ui::animation::AnimationSystem::hover(ui.ctx(), id.with("anim_hover"), is_hovered);
    let active_factor =
        crate::ui::animation::AnimationSystem::active(ui.ctx(), id.with("anim_active"), is_active);

    let alpha = (0.06 + 0.12 * hover_factor + 0.08 * active_factor).clamp(0.0, 1.0);
    let border_alpha = (0.25 + 0.45 * hover_factor + 0.15 * active_factor).clamp(0.0, 1.0);
    let fill = Color32::from_rgba_unmultiplied(239, 68, 68, (alpha * 255.0) as u8);
    let stroke = Stroke::new(
        1.0,
        Color32::from_rgba_unmultiplied(239, 68, 68, (border_alpha * 255.0) as u8),
    );
    let tint = crate::ui::animation::AnimationSystem::lerp_color(
        Color32::from_rgb(185, 28, 28),
        Color32::from_rgb(239, 68, 68),
        hover_factor,
    );

    let icon = egui::Image::new(egui::include_image!("../../resources/icons/trash.svg"))
        .fit_to_exact_size(egui::Vec2::splat(13.0))
        .tint(tint);

    let resp = Frame::new()
        .fill(fill)
        .stroke(stroke)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(7, 5))
        .show(ui, |ui| ui.add(icon))
        .response
        .interact(egui::Sense::click())
        .on_hover_text(crate::i18n::tr(language, "Delete"));

    ui.memory_mut(|m| {
        m.data.insert_temp(id.with("hover_state"), resp.hovered());
        m.data
            .insert_temp(id.with("active_state"), resp.is_pointer_button_down_on());
    });
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

pub fn feature_checkbox(
    ui: &mut Ui,
    feature: crate::feature_access::Feature,
    language: crate::i18n::UiLanguage,
    checked: &mut bool,
    text: &str,
) -> egui::Response {
    let access = crate::feature_access::access(feature);
    let mut response = ui
        .add_enabled_ui(access.available, |ui| toggle_with_label(ui, checked, text))
        .inner;
    response = decorate_unavailable(response, access, language);
    response
}

pub fn feature_ui<R>(
    ui: &mut Ui,
    feature: crate::feature_access::Feature,
    language: crate::i18n::UiLanguage,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> egui::InnerResponse<R> {
    let access = crate::feature_access::access(feature);
    let mut response = ui.add_enabled_ui(access.available, add_contents);
    response.response = decorate_unavailable(response.response, access, language);
    response
}

fn decorate_unavailable(
    response: egui::Response,
    access: crate::feature_access::FeatureAccess,
    language: crate::i18n::UiLanguage,
) -> egui::Response {
    match access.unavailable_reason {
        Some(reason) if !access.available => {
            response.on_disabled_hover_text(crate::i18n::tr(language, reason))
        }
        _ => response,
    }
}

pub fn directory_path_input(
    ui: &mut Ui,
    value: &mut String,
    hint: &str,
    browse_label: &str,
    input_width: f32,
) -> bool {
    let edit = egui::TextEdit::singleline(value)
        .hint_text(hint)
        .desired_width(input_width.min(420.0))
        .margin(egui::vec2(8.0, 6.0));
    let mut changed = text_edit_ui(ui, "path_picker_input", edit).changed();

    if animated_button(ui, browse_label).clicked()
        && let Some(path) = rfd::FileDialog::new().pick_folder()
    {
        *value = path.display().to_string();
        changed = true;
    }
    changed
}

pub fn text_edit_ui(
    ui: &mut Ui,
    id_source: impl std::hash::Hash + std::fmt::Debug,
    text_edit: egui::TextEdit<'_>,
) -> egui::Response {
    let is_hand_drawn = crate::ui::theme::is_hand_drawn(ui.ctx());
    let id = ui.make_persistent_id(id_source);

    let is_hovered = ui.memory(|m| {
        m.data
            .get_temp::<bool>(id.with("hover_state"))
            .unwrap_or(false)
    });
    let hover_factor =
        crate::ui::animation::AnimationSystem::hover(ui.ctx(), id.with("anim_hover"), is_hovered);

    let (response, rect) = ui.scope(|ui| {
        if is_hand_drawn {
            ui.style_mut().visuals.widgets.inactive.bg_stroke = egui::Stroke::NONE;
            ui.style_mut().visuals.widgets.inactive.bg_fill = egui::Color32::TRANSPARENT;
            ui.style_mut().visuals.widgets.hovered.bg_stroke = egui::Stroke::NONE;
            ui.style_mut().visuals.widgets.hovered.bg_fill =
                crate::ui::theme::surface_control_hover();
            ui.style_mut().visuals.widgets.active.bg_stroke = egui::Stroke::NONE;
            ui.style_mut().visuals.widgets.active.bg_fill =
                crate::ui::theme::surface_control_active();
            ui.style_mut().visuals.widgets.open.bg_stroke = egui::Stroke::NONE;
            ui.style_mut().visuals.widgets.open.bg_fill = egui::Color32::TRANSPARENT;
            ui.style_mut().visuals.extreme_bg_color = egui::Color32::TRANSPARENT;
            ui.style_mut().visuals.widgets.inactive.corner_radius = egui::CornerRadius::ZERO;
            ui.style_mut().visuals.widgets.hovered.corner_radius = egui::CornerRadius::ZERO;
            ui.style_mut().visuals.widgets.active.corner_radius = egui::CornerRadius::ZERO;
            ui.style_mut().visuals.widgets.open.corner_radius = egui::CornerRadius::ZERO;
        }

        let resp = ui.add(text_edit);
        let rect = resp.rect;
        (resp, rect)
    }).inner;

    let has_focus = response.has_focus();
    let hovered = response.hovered() || response.is_pointer_button_down_on();
    ui.memory_mut(|m| {
        m.data.insert_temp(id.with("hover_state"), hovered || has_focus);
    });

    if is_hand_drawn {
        let stroke_color = if has_focus {
            crate::ui::theme::primary()
        } else {
            crate::ui::animation::AnimationSystem::lerp_color(
                crate::ui::theme::border(),
                crate::ui::theme::primary(),
                hover_factor,
            )
        };
        crate::ui::organic_line::paint_hand_drawn_bottom_line(
            ui.painter(),
            id.with("bottom_line"),
            rect,
            egui::Stroke::new(if has_focus { 1.5 } else { 1.3 }, stroke_color),
        );
        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        }
    }

    response
}

pub fn singleline_input(
    ui: &mut Ui,
    value: &mut String,
    hint: &str,
    width: f32,
    secret: bool,
) -> egui::Response {
    let mut edit = egui::TextEdit::singleline(value)
        .hint_text(hint)
        .desired_width(width)
        .margin(egui::vec2(8.0, 6.0));
    if secret {
        edit = edit.password(true);
    }
    let id_salt = ui.next_auto_id().with("singleline_input");
    text_edit_ui(ui, id_salt, edit)
}

pub fn status_badge(ui: &mut Ui, status: &str, is_active: bool, is_error: bool) {
    let (fg_color, dot) = if is_error {
        (Color32::from_rgb(220, 38, 38), "● ")
    } else if is_active {
        (Color32::from_rgb(5, 150, 105), "● ")
    } else {
        (theme::primary_dark(), "")
    };

    let is_hand_drawn = theme::is_hand_drawn(ui.ctx());
    if is_hand_drawn {
        ui.label(
            egui::RichText::new(format!("{dot}{status}"))
                .color(fg_color)
                .size(12.0)
                .strong(),
        );
    } else {
        Frame::new()
            .fill(Color32::TRANSPARENT)
            .corner_radius(CornerRadius::same(14))
            .stroke(Stroke::new(1.0, fg_color))
            .inner_margin(Margin::symmetric(12, 5))
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(format!("{dot}{status}"))
                        .color(fg_color)
                        .size(12.0)
                        .strong(),
                );
            });
    }
}

pub struct SubNavItem<T: Copy + PartialEq> {
    pub id: T,
    pub icon: &'static str,
    pub label: &'static str,
}

pub fn sub_sidebar<T: Copy + PartialEq>(
    ui: &mut Ui,
    selected: &mut T,
    items: &[SubNavItem<T>],
    language: crate::i18n::UiLanguage,
) {
    let width = 150.0;
    let gap = 5.0;
    let item_height = 42.0_f32;

    let border_id = ui.make_persistent_id("sub_sidebar_organic_border");
    crate::ui::organic_border::show(
        ui,
        border_id,
        Frame::new()
            .fill(Color32::TRANSPARENT)
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::symmetric(10, 10)),
        10.0,
        theme::border(),
        |ui| {
            ui.set_width(width);
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(crate::i18n::tr(language, "NAVIGATE"))
                        .size(10.5)
                        .color(crate::ui::theme::text_weak())
                        .strong(),
                );
                ui.add_space(6.0);

                for (idx, item) in items.iter().enumerate() {
                    if idx > 0 {
                        ui.add_space(gap);
                    }

                    let is_selected = *selected == item.id;
                    let id = ui.make_persistent_id(item.label);

                    let is_hovered = ui.memory(|m| {
                        m.data
                            .get_temp::<bool>(id.with("hover_state"))
                            .unwrap_or(false)
                    });
                    let is_active = ui.memory(|m| {
                        m.data
                            .get_temp::<bool>(id.with("active_state"))
                            .unwrap_or(false)
                    });

                    let select_factor = crate::ui::animation::AnimationSystem::selection(
                        ui.ctx(),
                        id.with("select"),
                        is_selected,
                    );

                    let hover_factor = crate::ui::animation::AnimationSystem::hover(
                        ui.ctx(),
                        id.with("hover"),
                        is_hovered && !is_selected,
                    );

                    let active_factor = crate::ui::animation::AnimationSystem::active(
                        ui.ctx(),
                        id.with("active"),
                        is_active && !is_selected,
                    );

                    let bg_fill = Color32::TRANSPARENT;
                    let text_color = crate::ui::animation::AnimationSystem::lerp_color(
                        theme::text_normal(),
                        theme::primary(),
                        hover_factor,
                    );
                    let text_color = crate::ui::animation::AnimationSystem::lerp_color(
                        text_color,
                        theme::primary_dark(),
                        active_factor,
                    );
                    let text_color = crate::ui::animation::AnimationSystem::lerp_color(
                        text_color,
                        theme::primary_dark(),
                        select_factor,
                    );

                    let text = if item.icon.is_empty() {
                        item.label.to_string()
                    } else {
                        format!("{} {}", item.icon, item.label)
                    };

                    let button_h = item_height;
                    let v_padding = ((button_h - 18.0) / 2.0).max(8.0);

                    let resp = Frame::new()
                        .fill(bg_fill)
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(Margin::symmetric(12, v_padding as i8))
                        .stroke(Stroke::NONE)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                if select_factor > 0.1 {
                                    let (bar_rect, _) = ui.allocate_exact_size(
                                        Vec2::new(3.0, 14.0),
                                        egui::Sense::hover(),
                                    );
                                    let accent = theme::primary_dark();
                                    let bar_color = Color32::from_rgba_premultiplied(
                                        accent.r(),
                                        accent.g(),
                                        accent.b(),
                                        (255.0 * select_factor) as u8,
                                    );
                                    ui.painter().rect_filled(
                                        bar_rect,
                                        CornerRadius::same(2),
                                        bar_color,
                                    );
                                    ui.add_space(4.0);
                                }
                                let mut rt =
                                    egui::RichText::new(&text).size(13.5).color(text_color);
                                if is_selected {
                                    rt = rt.strong();
                                }
                                ui.label(rt)
                            })
                        })
                        .response
                        .interact(egui::Sense::click());

                    ui.memory_mut(|m| {
                        m.data.insert_temp(id.with("hover_state"), resp.hovered());
                        m.data
                            .insert_temp(id.with("active_state"), resp.is_pointer_button_down_on());
                    });

                    if resp.clicked() {
                        *selected = item.id;
                    }
                    let simulated_click =
                        crate::ui::automation::record_button(ui, id, &item.label, true, resp.rect);
                    if simulated_click {
                        *selected = item.id;
                        simulate_click_on(ui, resp.rect);
                        ui.ctx().request_repaint();
                    }
                    if resp.hovered() && !is_selected {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                }
            });
        },
    );
}

pub struct ModernSlider<'a, Num: egui::emath::Numeric> {
    label: &'a str,
    value: &'a mut Num,
    range: std::ops::RangeInclusive<Num>,
    default: Num,
    step: Option<f64>,
    suffix: Option<String>,
    precision: Option<usize>,
    states: &'a [&'a str],
    percentage: bool,
    id_salt: Option<String>,
    label_width: Option<f32>,
}

impl<'a, Num: egui::emath::Numeric> ModernSlider<'a, Num> {
    pub fn new(
        label: &'a str,
        value: &'a mut Num,
        range: std::ops::RangeInclusive<Num>,
        default: Num,
    ) -> Self {
        Self {
            label,
            value,
            range,
            default,
            step: None,
            suffix: None,
            precision: None,
            states: &[],
            percentage: false,
            id_salt: None,
            label_width: None,
        }
    }

    pub fn step(mut self, step: f64) -> Self {
        self.step = Some(step);
        self
    }

    pub fn suffix(mut self, suffix: impl Into<String>) -> Self {
        self.suffix = Some(suffix.into());
        self
    }

    pub fn precision(mut self, precision: usize) -> Self {
        self.precision = Some(precision);
        self
    }

    pub fn states(mut self, states: &'a [&'a str]) -> Self {
        self.states = states;
        self
    }

    pub fn percentage(mut self, percentage: bool) -> Self {
        self.percentage = percentage;
        self
    }

    pub fn id_salt(mut self, id_salt: impl Into<String>) -> Self {
        self.id_salt = Some(id_salt.into());
        self
    }

    pub fn label_width(mut self, width: f32) -> Self {
        self.label_width = Some(width);
        self
    }

    pub fn show(self, ui: &mut Ui) -> egui::Response {
        ui.horizontal(|ui| {
            let available = ui.available_width();
            let is_narrow = available < 280.0;
            let default_label_w = if is_narrow { 85.0 } else { 110.0 };
            let label_w = self.label_width.unwrap_or(default_label_w);

            if !self.label.is_empty() {
                ui.allocate_ui_with_layout(
                    Vec2::new(label_w, 20.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.label(
                            egui::RichText::new(self.label)
                                .color(crate::ui::theme::text_strong())
                                .size(if is_narrow { 12.0 } else { 13.0 })
                                .strong(),
                        );
                    },
                );
            }

            let badge_text = if !self.states.is_empty() {
                let val_f = self.value.to_f64() as f32;
                let start_f = self.range.start().to_f64() as f32;
                let end_f = self.range.end().to_f64() as f32;
                let range_f = start_f..=end_f;
                slider_state_label(val_f, &range_f, self.states)
                    .map(str::to_owned)
                    .unwrap_or_else(|| {
                        let prec = self.precision.unwrap_or(2);
                        format!("{:.prec$}", val_f)
                    })
            } else if self.percentage {
                let percent = (self.value.to_f64() * 100.0).round() as i64;
                format!("{percent} %")
            } else {
                let val = self.value.to_f64();
                let suffix = self.suffix.as_deref().unwrap_or("");
                if let Some(prec) = self.precision {
                    format!("{val:.prec$}{suffix}")
                } else if Num::INTEGRAL {
                    format!("{val:.0}{suffix}")
                } else {
                    format!("{val:.2}{suffix}")
                }
            };

            let reserve_w = (badge_text.chars().count() as f32 * 7.5 + 36.0).max(82.0);
            let slider_w = (ui.available_width() - reserve_w).max(50.0);
            let mut response = ui
                .scope(|ui| {
                    let style = ui.style_mut();
                    style.spacing.slider_rail_height = 8.0;
                    style.visuals.widgets.inactive.bg_fill =
                        Color32::from_rgba_unmultiplied(130, 139, 143, 170);
                    style.visuals.widgets.hovered.bg_fill = theme::primary();
                    style.visuals.widgets.hovered.fg_stroke = Stroke::NONE;
                    style.visuals.widgets.active.bg_fill = theme::primary();
                    style.visuals.widgets.active.fg_stroke = Stroke::NONE;
                    style.visuals.widgets.inactive.fg_stroke = Stroke::NONE;
                    style.visuals.selection.bg_fill = theme::primary_fill();
                    style.visuals.widgets.inactive.corner_radius = CornerRadius::same(4);
                    style.visuals.handle_shape = egui::style::HandleShape::Rect { aspect_ratio: 0.0 };

                    let mut slider = egui::Slider::new(self.value, self.range.clone())
                        .show_value(false)
                        .trailing_fill(true);
                    if let Some(step) = self.step {
                        slider = slider.step_by(step);
                    }
                    ui.add_sized(Vec2::new(slider_w, 20.0), slider)
                })
                .inner;

            let salt_str = self.id_salt.as_deref().unwrap_or(self.label);
            let salt = if salt_str.is_empty() { "slider" } else { salt_str };
            let slider_id = ui.make_persistent_id(salt);
            let record_label = if self.label.is_empty() { salt } else { self.label };
            if let Some(new_val) = crate::ui::automation::record_slider(
                ui,
                slider_id,
                record_label,
                self.value.to_f64(),
                true,
                response.rect,
            ) {
                let clamped = new_val.clamp(
                    self.range.start().to_f64(),
                    self.range.end().to_f64(),
                );
                *self.value = Num::from_f64(clamped);
                response.mark_changed();
            }

            ui.add_space(4.0);
            tech_numeric_badge(ui, &badge_text);

            let mut reset = reset_button(ui, salt);
            let diff = (self.value.to_f64() - self.default.to_f64()).abs();
            if reset.clicked() && (diff > 1e-6 || *self.value != self.default) {
                *self.value = self.default;
                reset.mark_changed();
            }

            response | reset
        })
        .inner
    }
}

pub fn modern_slider_f64(
    ui: &mut Ui,
    value: &mut f64,
    range: std::ops::RangeInclusive<f64>,
    default: f64,
    label: &str,
    suffix: &str,
) -> egui::Response {
    ModernSlider::new(label, value, range, default)
        .step(0.5)
        .suffix(suffix)
        .precision(1)
        .show(ui)
}

pub fn modern_slider_f32(
    ui: &mut Ui,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    default: f32,
    label: &str,
    states: &[&str],
) -> egui::Response {
    ModernSlider::new(label, value, range, default)
        .step(0.01)
        .precision(2)
        .states(states)
        .show(ui)
}

fn slider_state_label<'a>(
    value: f32,
    range: &std::ops::RangeInclusive<f32>,
    states: &'a [&str],
) -> Option<&'a str> {
    if states.is_empty() {
        return None;
    }
    let span = *range.end() - *range.start();
    let fraction = if span > 0.0 {
        (value - *range.start()) / span
    } else {
        0.0
    };
    let index = (fraction.clamp(0.0, 1.0) * (states.len() - 1) as f32).round() as usize;
    states.get(index).copied()
}

pub fn reset_button(ui: &mut Ui, id_salt: &str) -> egui::Response {
    let id = ui.make_persistent_id("slider_reset_btn").with(id_salt);
    let is_hovered = ui.memory(|m| {
        m.data
            .get_temp::<bool>(id.with("hover_state"))
            .unwrap_or(false)
    });
    let is_active = ui.memory(|m| {
        m.data
            .get_temp::<bool>(id.with("active_state"))
            .unwrap_or(false)
    });

    let hover_factor =
        crate::ui::animation::AnimationSystem::hover(ui.ctx(), id.with("hover"), is_hovered);
    let active_factor =
        crate::ui::animation::AnimationSystem::active(ui.ctx(), id.with("active"), is_active);

    let current_time = ui.ctx().input(|i| i.time);
    let spin_start = ui.memory(|m| m.data.get_temp::<f64>(id.with("spin_start")).unwrap_or(0.0));
    let elapsed = (current_time - spin_start) as f32;
    let spin_duration = 0.40;
    let is_spinning = elapsed >= 0.0 && elapsed < spin_duration;

    let (rotation_angle, spin_accent_factor) = if is_spinning {
        ui.ctx().request_repaint();
        let t = (elapsed / spin_duration).clamp(0.0, 1.0);
        let progress = crate::ui::animation::AnimationSystem::ease_out_cubic(t);
        let angle = -std::f32::consts::TAU * progress;
        let accent = (1.0 - progress).clamp(0.0, 1.0);
        (angle, accent)
    } else {
        (0.0, 0.0)
    };

    let is_hand_drawn = theme::is_hand_drawn(ui.ctx());
    let (fill, stroke_color) = if is_hand_drawn {
        (Color32::TRANSPARENT, Color32::TRANSPARENT)
    } else {
        (theme::surface_control(), theme::border())
    };

    let icon_rest_tint = crate::ui::animation::AnimationSystem::lerp_color(
        theme::text_normal(),
        theme::primary(),
        hover_factor,
    );
    let icon_rest_tint = crate::ui::animation::AnimationSystem::lerp_color(
        icon_rest_tint,
        theme::primary_dark(),
        active_factor,
    );
    let icon_spin_tint = Color32::from_rgb(37, 99, 235);
    let icon_tint = crate::ui::animation::AnimationSystem::lerp_color(
        icon_rest_tint,
        icon_spin_tint,
        spin_accent_factor,
    );

    let mut image = egui::Image::new(egui::include_image!("../../resources/icons/reset.svg"))
        .fit_to_exact_size(Vec2::splat(13.0))
        .tint(icon_tint);

    if rotation_angle.abs() > 0.0001 {
        image = image.rotate(rotation_angle, Vec2::splat(0.5));
    }

    let resp = Frame::new()
        .fill(fill)
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(6))
        .stroke(Stroke::new(1.0, stroke_color))
        .shadow(egui::Shadow::NONE)
        .show(ui, |ui| {
            ui.add(image);
        })
        .response
        .interact(egui::Sense::click());

    if resp.clicked() {
        ui.memory_mut(|m| {
            m.data.insert_temp(id.with("spin_start"), current_time);
        });
        ui.ctx().request_repaint();
    }

    ui.memory_mut(|m| {
        m.data.insert_temp(id.with("hover_state"), resp.hovered());
        m.data
            .insert_temp(id.with("active_state"), resp.is_pointer_button_down_on());
    });
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

pub fn render_runtime_task_state(
    ui: &mut egui::Ui,
    language: crate::i18n::UiLanguage,
    state: &crate::runtime_install::RuntimeInstallState,
    extracting_text: &'static str,
    installed_text: &'static str,
) {
    use crate::runtime_install::RuntimeInstallState;

    match state {
        RuntimeInstallState::Idle | RuntimeInstallState::Ready => {}
        RuntimeInstallState::Detecting => {
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(crate::i18n::tr(
                    language,
                    "Detecting the recommended runtime...",
                ))
                .size(12.0)
                .color(theme::text_weak()),
            );
        }
        RuntimeInstallState::Downloading {
            asset,
            downloaded,
            total,
        } => {
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(asset)
                    .size(11.0)
                    .color(theme::text_weak()),
            );
            if *total > 0 {
                ui.add(
                    egui::ProgressBar::new(
                        (*downloaded as f64 / *total as f64).clamp(0.0, 1.0) as f32
                    )
                    .text(format!(
                        "{} / {}",
                        format_file_size(*downloaded),
                        format_file_size(*total),
                    )),
                );
            }
        }
        RuntimeInstallState::Extracting => {
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(crate::i18n::tr(language, extracting_text))
                    .size(12.0)
                    .color(theme::text_weak()),
            );
        }
        RuntimeInstallState::Installed => {
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(crate::i18n::tr(language, installed_text))
                    .size(12.0)
                    .color(Color32::from_rgb(5, 150, 105)),
            );
        }
        RuntimeInstallState::Failed(error) => {
            ui.add_space(6.0);
            error_notice(ui, language, error);
        }
    }
}

pub fn render_runtime_fallback_notice(
    ui: &mut egui::Ui,
    language: crate::i18n::UiLanguage,
    installer: &crate::runtime_install::RuntimeInstaller,
) {
    let Some(reason) = installer.fallback_reason() else {
        return;
    };
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(reason)
            .size(11.5)
            .color(Color32::from_rgb(180, 83, 9)),
    );
    if reason.contains(crate::runtime_install::NVIDIA_APP_URL) {
        ui.hyperlink_to(
            crate::i18n::tr(language, "Open NVIDIA App driver download"),
            crate::runtime_install::NVIDIA_APP_URL,
        );
    }
}
