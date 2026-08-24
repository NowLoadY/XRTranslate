use crate::ui::theme;
use eframe::egui::{self, Color32, CornerRadius, Frame, Margin, Stroke, Ui, Vec2};

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
    Frame::new()
        .fill(Color32::TRANSPARENT)
        .stroke(Stroke::new(1.0, theme::border()))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(speaker)
                    .color(theme::primary_dark())
                    .size(11.5)
                    .strong(),
            );
        });
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
    if visible {
        if updating {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(33));
        }
        let level = raw_fraction.clamp(0.0, 1.0);
        const POINTS: usize = 24;
        const HEIGHT: f32 = 11.0;
        const WIDTH: f32 = 68.0;

        let id = ui.make_persistent_id(("audio_waveform_samples", id_source));
        let mut history = ui.memory(|memory| {
            memory
                .data
                .get_temp::<Vec<f32>>(id)
                .unwrap_or_else(|| vec![0.0; POINTS])
        });
        if history.len() != POINTS {
            history.resize(POINTS, 0.0);
        }
        if updating {
            history.push(level);
            let excess = history.len().saturating_sub(POINTS);
            if excess > 0 {
                history.drain(..excess);
            }
        } else {
            // A stopped session must not keep repainting the previous audio
            // envelope. Reset the visual state immediately on the next frame.
            history.fill(0.0);
        }
        ui.memory_mut(|memory| memory.data.insert_temp(id, history.clone()));

        let (rect, _) = ui.allocate_exact_size(Vec2::new(WIDTH, HEIGHT), egui::Sense::hover());
        if ui.is_rect_visible(rect) {
            let painter = ui.painter();
            let baseline = rect.center().y;
            let max_deviation = (HEIGHT * 0.5 - 0.7).max(1.0);
            let base_color = if !updating {
                theme::border()
            } else if active {
                Color32::from_rgb(48, 145, 98)
            } else {
                theme::primary()
            };
            let mut points = Vec::with_capacity(POINTS);
            for (index, sample) in history.iter().enumerate() {
                let x = egui::lerp(
                    rect.left()..=rect.right(),
                    index as f32 / (POINTS - 1) as f32,
                );
                let envelope = if *sample < 0.025 {
                    0.0
                } else {
                    (sample.sqrt() * 1.65).clamp(0.0, 1.0)
                };
                let phase = index as f32 * std::f32::consts::PI * 0.62;
                let y = baseline + phase.sin() * envelope * max_deviation;
                points.push(egui::pos2(x, y));
            }

            // Keep a visible baseline even when the input is silent.
            painter.line_segment(
                [
                    egui::pos2(rect.left(), baseline),
                    egui::pos2(rect.right(), baseline),
                ],
                Stroke::new(1.2, base_color),
            );

            // Draw one continuous stroke so the waveform keeps a single,
            // stable feedback color regardless of its amplitude.
            painter.add(egui::Shape::line(points, Stroke::new(1.8, base_color)));
        }
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

    let fill = if enabled {
        let hover_bg = theme::surface_control_hover();
        crate::ui::animation::AnimationSystem::lerp_color(theme::surface_control(), hover_bg, hover_factor)
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

    let stroke = if enabled {
        let stroke_color = crate::ui::animation::AnimationSystem::lerp_color(
            theme::border(),
            theme::primary(),
            hover_factor * 0.4,
        );
        Stroke::new(1.0, stroke_color)
    } else {
        Stroke::new(1.0, theme::border())
    };

    let resp = ui
        .add_enabled_ui(enabled, |ui| {
            Frame::new()
                .fill(fill)
                .stroke(stroke)
                .corner_radius(CornerRadius::same(8))
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

    let primary_rgb = theme::primary();
    let (fill, stroke, text_color) = if enabled {
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

    let resp = ui
        .add_enabled_ui(enabled, |ui| {
            Frame::new()
                .fill(fill)
                .stroke(stroke)
                .corner_radius(CornerRadius::same(8))
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
    let mut changed = false;
    let search_id = ui.make_persistent_id(&id).with("combo_search");
    let selected_text = selected_text.into();
    let control_width = crate::ui::layout::control_width(ui, &selected_text, width, 96.0, 240.0);

    ui.scope(|ui| {
        if !frame {
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
                egui::RichText::new(selected_text)
                    .size(12.0)
                    .color(crate::ui::theme::text_strong()),
            )
            .width(control_width)
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
                    ui.add(te);
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
        });
    });

    changed
}

pub fn search_bar(ui: &mut Ui, query: &mut String, hint: &str) -> bool {
    let mut changed = false;
    let has_query = !query.is_empty();
    Frame::new()
        .fill(theme::surface_control())
        .stroke(Stroke::new(1.0, theme::border()))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
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
            });
        });
    changed
}

pub fn input_field(ui: &mut Ui, text: &mut String, hint: &str) -> egui::Response {
    let mut resp = None;
    Frame::new()
        .fill(theme::surface_control())
        .stroke(Stroke::new(1.0, theme::border()))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            let text_frame = Frame::new()
                .fill(Color32::TRANSPARENT)
                .stroke(Stroke::NONE)
                .corner_radius(CornerRadius::ZERO)
                .inner_margin(Margin::ZERO);
            let r = ui.add(
                egui::TextEdit::singleline(text)
                    .hint_text(hint)
                    .frame(text_frame)
                    .margin(Margin::ZERO)
                    .desired_width(ui.available_width()),
            );
            resp = Some(r);
        });
    resp.unwrap()
}

pub fn danger_alert(ui: &mut Ui, text: &str) {
    Frame::new()
        .fill(Color32::from_rgb(254, 242, 242))
        .stroke(Stroke::new(1.0, Color32::from_rgb(254, 202, 202)))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("!")
                        .strong()
                        .color(Color32::from_rgb(220, 38, 38)),
                );
                ui.label(
                    egui::RichText::new(text)
                        .color(Color32::from_rgb(185, 28, 28))
                        .size(12.5),
                );
            });
        });
}

pub fn target_language_pair_selector(
    ui: &mut Ui,
    id_prefix: &str,
    source_language: &str,
    target_language: &mut String,
    language: crate::i18n::UiLanguage,
    label_fn: impl Fn(&str, crate::i18n::UiLanguage) -> String,
) -> bool {
    let mut changed = false;
    if source_language == "auto" {
        let (mut a, mut b) = match target_language.split_once(',') {
            Some((x, y)) => (x.to_string(), y.to_string()),
            None => ("zh".to_string(), "en".to_string()),
        };

        let options_a: Vec<_> = crate::LANGUAGE_OPTIONS
            .iter()
            .filter(|(code, _)| !crate::languages_conflict(code, &b))
            .map(|(code, label)| {
                (
                    (*code).to_string(),
                    crate::i18n::tr(language, label).to_string(),
                )
            })
            .collect();

        let options_b: Vec<_> = crate::LANGUAGE_OPTIONS
            .iter()
            .filter(|(code, _)| !crate::languages_conflict(code, &a))
            .map(|(code, label)| {
                (
                    (*code).to_string(),
                    crate::i18n::tr(language, label).to_string(),
                )
            })
            .collect();

        ui.horizontal(|ui| {
            if searchable_combobox(
                ui,
                format!("{id_prefix}_target_a"),
                label_fn(&a, language),
                &mut a,
                &options_a,
            ) {
                changed = true;
            }
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("↔")
                    .color(crate::ui::theme::text_weak())
                    .strong(),
            );
            ui.add_space(4.0);
            if searchable_combobox(
                ui,
                format!("{id_prefix}_target_b"),
                label_fn(&b, language),
                &mut b,
                &options_b,
            ) {
                changed = true;
            }
        });

        let new_target = format!("{a},{b}");
        if new_target != *target_language {
            *target_language = new_target;
            changed = true;
        }
    } else {
        if target_language.contains(',') {
            if let Some((first, _)) = target_language.split_once(',') {
                *target_language = first.to_string();
                changed = true;
            }
        }
        if crate::languages_conflict(target_language, source_language) {
            let fallback = if crate::languages_conflict(source_language, "zh") {
                "en"
            } else {
                "zh"
            };
            *target_language = fallback.to_string();
            changed = true;
        }

        let mut target_options = Vec::new();
        for (code, label) in crate::LANGUAGE_OPTIONS {
            if !crate::languages_conflict(code, source_language) {
                target_options.push((
                    (*code).to_string(),
                    crate::i18n::tr(language, label).to_string(),
                ));
            }
        }

        if searchable_combobox(
            ui,
            format!("{id_prefix}_target"),
            label_fn(target_language, language),
            target_language,
            &target_options,
        ) {
            changed = true;
        }
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

    let danger_rgb = (239, 68, 68);
    let (fill, stroke, text_color) = if enabled {
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

    let resp = ui
        .add_enabled_ui(enabled, |ui| {
            Frame::new()
                .fill(fill)
                .stroke(stroke)
                .corner_radius(CornerRadius::same(8))
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

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();

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
        painter.rect_stroke(
            rect,
            CornerRadius::same(10),
            Stroke::new(1.0, track_stroke),
            egui::StrokeKind::Inside,
        );

        let knob_radius = 7.5;
        let min_x = rect.min.x + 10.0;
        let max_x = rect.max.x - 10.0;
        let current_x = min_x + (max_x - min_x) * switch_factor;

        let knob_center = egui::pos2(current_x, rect.center().y);
        let knob_color = crate::ui::animation::AnimationSystem::lerp_color(
            interaction_color,
            theme::primary(),
            switch_factor,
        );
        painter.circle_filled(knob_center, knob_radius, knob_color);
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

pub fn toggle_with_label(ui: &mut Ui, checked: &mut bool, label: &str) -> egui::Response {
    ui.horizontal(|ui| {
        let mut resp = pill_toggle(ui, checked);
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

pub fn resource_delete_button(ui: &mut Ui, language: crate::i18n::UiLanguage) -> egui::Response {
    let id = ui.make_persistent_id("resource_delete_btn");
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
        is_hovered,
    );
    let active_factor = crate::ui::animation::AnimationSystem::active(
        ui.ctx(),
        id.with("anim_active"),
        is_active,
    );

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
        .show(ui, |ui| {
            ui.add(icon)
        })
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
    let mut changed = ui
        .add(
            egui::TextEdit::singleline(value)
                .hint_text(hint)
                .desired_width(input_width.min(420.0))
                .margin(egui::vec2(10.0, 8.0)),
        )
        .changed();

    if animated_button(ui, browse_label).clicked()
        && let Some(path) = rfd::FileDialog::new().pick_folder()
    {
        *value = path.display().to_string();
        changed = true;
    }
    changed
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
        .margin(egui::vec2(10.0, 8.0));
    if secret {
        edit = edit.password(true);
    }
    ui.add(edit)
}

pub fn status_badge(ui: &mut Ui, status: &str, is_active: bool, is_error: bool) {
    let (fg_color, dot) = if is_error {
        (Color32::from_rgb(220, 38, 38), "● ")
    } else if is_active {
        (Color32::from_rgb(5, 150, 105), "● ")
    } else {
        (theme::primary_dark(), "")
    };

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
                    if resp.hovered() && !is_selected {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                }
            });
        },
    );
}

pub fn modern_slider_f64(
    ui: &mut Ui,
    value: &mut f64,
    range: std::ops::RangeInclusive<f64>,
    default: f64,
    label: &str,
    suffix: &str,
) -> egui::Response {
    ui.horizontal(|ui| {
        let available = ui.available_width();
        let is_narrow = available < 280.0;
        let label_w = if is_narrow { 85.0 } else { 110.0 };

        if !label.is_empty() {
            ui.allocate_ui_with_layout(
                Vec2::new(label_w, 20.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.label(
                        egui::RichText::new(label)
                            .color(crate::ui::theme::text_strong())
                            .size(if is_narrow { 12.0 } else { 13.0 })
                            .strong(),
                    );
                },
            );
        }

        let slider = egui::Slider::new(value, range)
            .show_value(false)
            .step_by(0.5)
            .trailing_fill(true);

        let slider_w = (ui.available_width() - 82.0).max(50.0);
        let response = ui
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
                ui.add_sized(Vec2::new(slider_w, 20.0), slider)
            })
            .inner;

        let value_text = format!("{:.1}{}", *value, suffix);
        ui.add_space(4.0);
        Frame::new()
            .fill(Color32::TRANSPARENT)
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::symmetric(10, 4))
            .stroke(Stroke::new(1.0, theme::border()))
            .shadow(egui::Shadow::NONE)
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(value_text)
                        .size(12.5)
                        .color(crate::ui::theme::text_strong())
                        .strong(),
                );
            });

        let mut reset = reset_button(ui, label);
        if reset.clicked() && *value != default {
            *value = default;
            reset.mark_changed();
        }

        response | reset
    })
    .inner
}

pub fn modern_slider_f32(
    ui: &mut Ui,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    default: f32,
    label: &str,
    states: &[&str],
) -> egui::Response {
    ui.horizontal(|ui| {
        let available = ui.available_width();
        let is_narrow = available < 280.0;
        let label_w = if is_narrow { 85.0 } else { 110.0 };

        ui.allocate_ui_with_layout(
            Vec2::new(label_w, 20.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(
                    egui::RichText::new(label)
                        .color(crate::ui::theme::text_strong())
                        .size(if is_narrow { 12.0 } else { 13.0 })
                        .strong(),
                );
            },
        );
        let slider_w = (ui.available_width() - 82.0).max(50.0);
        let response = ui
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
                ui.add_sized(
                    Vec2::new(slider_w, 20.0),
                    egui::Slider::new(value, range.clone())
                        .show_value(false)
                        .step_by(0.01)
                        .trailing_fill(true),
                )
            })
            .inner;
        ui.add_space(4.0);
        Frame::new()
            .fill(Color32::TRANSPARENT)
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::symmetric(10, 4))
            .stroke(Stroke::new(1.0, theme::border()))
            .shadow(egui::Shadow::NONE)
            .show(ui, |ui| {
                let label = slider_state_label(*value, &range, states)
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("{value:.2}"));
                ui.label(
                    egui::RichText::new(label)
                        .size(12.5)
                        .color(crate::ui::theme::text_strong())
                        .strong(),
                );
            });
        let mut reset = reset_button(ui, label);
        if reset.clicked() && *value != default {
            *value = default;
            reset.mark_changed();
        }
        response | reset
    })
    .inner
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

    let fill = theme::surface_control();

    let stroke_color = theme::border();

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
            ui.label(
                egui::RichText::new(error)
                    .size(12.0)
                    .color(Color32::from_rgb(220, 38, 38)),
            );
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
