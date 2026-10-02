//! Direct manipulation of the head-locked subtitle surface, with optional precision controls.

use super::renderer::{VrSubtitleCard, paint_cards};
use super::runtime::{VrOverlaySettings, VrRuntimeStatus};
use crate::ui::components::{ModernSlider, card};
use crate::ui::theme;
use eframe::egui;

#[derive(Clone, Copy)]
pub struct VrOverlayPageContext<'a> {
    pub language: crate::i18n::UiLanguage,
    pub status: &'a VrRuntimeStatus,
}

#[derive(Debug, PartialEq)]
pub enum VrOverlayUiAction {
    SettingsChanged,
    ClearSubtitles,
    ConnectSteamVr,
    DisconnectSteamVr,
    RecenterAvatar,
}

// Reserve a complete button before wrapped layout chooses the next row.
// A nested frame alone can squeeze its label into the row's remaining sliver.
fn header_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let label = ui.painter().layout_no_wrap(
        text.to_owned(),
        egui::FontId::proportional(13.0),
        egui::Color32::WHITE,
    );
    ui.allocate_ui_with_layout(
        label.size() + egui::vec2(30.0, 14.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| crate::ui::components::secondary_button(ui, text),
    )
    .inner
}

fn render_header(
    ui: &mut egui::Ui,
    context: VrOverlayPageContext<'_>,
    actions: &mut Vec<VrOverlayUiAction>,
) {
    let lang = context.language;
    let tr = |s| crate::i18n::tr(lang, s);
    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new("SteamVR")
                .size(20.0)
                .color(theme::text_strong())
                .strong(),
        );
        ui.add_space(8.0);
        if context.status.steamvr_connected {
            crate::ui::components::status_badge(ui, &tr("SteamVR Connected"), true, false);
            if context.status.avatar_available && header_button(ui, tr("Come here")).clicked() {
                actions.push(VrOverlayUiAction::RecenterAvatar);
            }
            if header_button(ui, tr("Disconnect")).clicked() {
                actions.push(VrOverlayUiAction::DisconnectSteamVr);
            }
        } else {
            crate::ui::components::status_badge(ui, &tr("Not Connected"), false, false);
            if crate::ui::components::animated_button(ui, tr("Connect SteamVR")).clicked() {
                actions.push(VrOverlayUiAction::ConnectSteamVr);
            }
        }
    });
    if let Some(error) = &context.status.last_error {
        crate::ui::components::error_notice(ui, lang, error);
    }
    if let Some(error) = &context.status.avatar_error {
        ui.push_id("vr_avatar_error", |ui| {
            crate::ui::components::error_notice(
                ui,
                lang,
                &format!("{}: {error}", tr("Avatar unavailable")),
            );
        });
    }
}

fn render_position_card(
    settings: &mut VrOverlaySettings,
    ui: &mut egui::Ui,
    lang: crate::i18n::UiLanguage,
) {
    let tr = |s| crate::i18n::tr(lang, s);
    card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(tr("Position Controls")).strong());
            if crate::ui::components::reset_button(ui, "vr_spatial_all_reset").clicked() {
                settings.distance_meters = VrOverlaySettings::DEFAULT_DISTANCE;
                settings.vertical_offset_meters = VrOverlaySettings::DEFAULT_VERTICAL_OFFSET;
                settings.overlay_width_meters = VrOverlaySettings::DEFAULT_OVERLAY_WIDTH;
            }
        });
        ui.add_space(6.0);
        ModernSlider::new(
            &tr("Distance in Front"),
            &mut settings.distance_meters,
            0.5..=2.5,
            VrOverlaySettings::DEFAULT_DISTANCE,
        )
        .step(0.05)
        .precision(2)
        .suffix(" m")
        .id_salt("vr_dist")
        .label_width(92.0)
        .show(ui);
        egui::CollapsingHeader::new(tr("Precise Position"))
            .id_salt("vr_precise_position")
            .default_open(true)
            .show(ui, |ui| {
                ModernSlider::new(
                    &tr("Height Offset"),
                    &mut settings.vertical_offset_meters,
                    -0.8..=0.4,
                    VrOverlaySettings::DEFAULT_VERTICAL_OFFSET,
                )
                .step(0.02)
                .precision(2)
                .suffix(" m")
                .id_salt("vr_v_offset")
                .label_width(92.0)
                .show(ui);
                ModernSlider::new(
                    &tr("Overlay Width"),
                    &mut settings.overlay_width_meters,
                    0.3..=1.5,
                    VrOverlaySettings::DEFAULT_OVERLAY_WIDTH,
                )
                .step(0.02)
                .precision(2)
                .suffix(" m")
                .id_salt("vr_width")
                .label_width(92.0)
                .show(ui);
            });
    });
}

fn render_display_card(
    settings: &mut VrOverlaySettings,
    ui: &mut egui::Ui,
    lang: crate::i18n::UiLanguage,
) {
    let tr = |s| crate::i18n::tr(lang, s);
    card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(tr("Display Settings")).strong());
            if crate::ui::components::reset_button(ui, "vr_display_all_reset").clicked() {
                settings.max_items = VrOverlaySettings::DEFAULT_MAX_ITEMS;
                settings.bilingual = true;
                settings.font_size = VrOverlaySettings::DEFAULT_FONT_SIZE;
                settings.opacity = VrOverlaySettings::DEFAULT_OPACITY;
                settings.display_timeout_seconds = VrOverlaySettings::DEFAULT_TIMEOUT;
            }
        });
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(tr("Max Subtitle Count"));
            for count in 1..=5 {
                ui.selectable_value(&mut settings.max_items, count, count.to_string());
            }
            ui.add_space(8.0);
            ui.checkbox(&mut settings.bilingual, tr("Bilingual Subtitles"));
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(tr("Font Size"));
            ui.add(
                egui::DragValue::new(&mut settings.font_size)
                    .range(12.0..=36.0)
                    .speed(0.5)
                    .suffix(" px"),
            );
            ui.add_space(8.0);
            ui.label(tr("Display Duration"));
            ui.add(
                egui::DragValue::new(&mut settings.display_timeout_seconds)
                    .range(3.0..=30.0)
                    .speed(0.5)
                    .suffix(" s"),
            );
        });
        ModernSlider::new(
            &tr("Opacity"),
            &mut settings.opacity,
            0.2..=1.0,
            VrOverlaySettings::DEFAULT_OPACITY,
        )
        .step(0.01)
        .percentage(true)
        .id_salt("vr_opacity")
        .label_width(92.0)
        .show(ui);
    });
}

fn render_preview_card(
    settings: &mut VrOverlaySettings,
    ui: &mut egui::Ui,
    context: VrOverlayPageContext<'_>,
    actions: &mut Vec<VrOverlayUiAction>,
    preview_height: f32,
) {
    let lang = context.language;
    let tr = |s| crate::i18n::tr(lang, s);
    card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(tr("Live Caption Preview")).strong());
            if context.status.cards.is_empty() {
                ui.label(
                    egui::RichText::new(tr("Sample"))
                        .small()
                        .color(theme::text_weak()),
                );
            }
            if !context.status.cards.is_empty()
                && crate::ui::components::secondary_button(ui, tr("Clear Subtitles")).clicked()
            {
                actions.push(VrOverlayUiAction::ClearSubtitles);
            }
        });
        ui.add_space(8.0);
        let demo = [VrSubtitleCard {
            source: if lang == crate::i18n::UiLanguage::English {
                "天气真好。"
            } else {
                "It's a beautiful day."
            }
            .into(),
            translated: tr("A beautiful day.").to_owned(),
            speaker: String::new(),
            live: true,
        }];
        let cards = if context.status.cards.is_empty() {
            &demo[..]
        } else {
            &context.status.cards
        };
        spatial_preview(ui, settings, cards, preview_height);
        ui.label(
            egui::RichText::new(tr("Drag up or down · Drag a corner to resize"))
                .small()
                .color(theme::text_weak()),
        );
    });
}

fn render_combined_preview_and_position_card(
    settings: &mut VrOverlaySettings,
    ui: &mut egui::Ui,
    context: VrOverlayPageContext<'_>,
    actions: &mut Vec<VrOverlayUiAction>,
    preview_height: f32,
) {
    let lang = context.language;
    let tr = |s| crate::i18n::tr(lang, s);
    card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(tr("Live Caption Preview")).strong());
            if context.status.cards.is_empty() {
                ui.label(
                    egui::RichText::new(tr("Sample"))
                        .small()
                        .color(theme::text_weak()),
                );
            }
            if crate::ui::components::reset_button(ui, "vr_spatial_all_reset").clicked() {
                settings.distance_meters = VrOverlaySettings::DEFAULT_DISTANCE;
                settings.vertical_offset_meters = VrOverlaySettings::DEFAULT_VERTICAL_OFFSET;
                settings.overlay_width_meters = VrOverlaySettings::DEFAULT_OVERLAY_WIDTH;
            }
            if !context.status.cards.is_empty()
                && crate::ui::components::secondary_button(ui, tr("Clear Subtitles")).clicked()
            {
                actions.push(VrOverlayUiAction::ClearSubtitles);
            }
        });
        ui.add_space(8.0);
        let demo = [VrSubtitleCard {
            source: if lang == crate::i18n::UiLanguage::English {
                "天气真好。"
            } else {
                "It's a beautiful day."
            }
            .into(),
            translated: tr("A beautiful day.").to_owned(),
            speaker: String::new(),
            live: true,
        }];
        let cards = if context.status.cards.is_empty() {
            &demo[..]
        } else {
            &context.status.cards
        };
        spatial_preview(ui, settings, cards, preview_height);
        ui.label(
            egui::RichText::new(tr("Drag up or down · Drag a corner to resize"))
                .small()
                .color(theme::text_weak()),
        );
        ModernSlider::new(
            &tr("Distance in Front"),
            &mut settings.distance_meters,
            0.5..=2.5,
            VrOverlaySettings::DEFAULT_DISTANCE,
        )
        .step(0.05)
        .precision(2)
        .suffix(" m")
        .id_salt("vr_dist")
        .label_width(92.0)
        .show(ui);
        egui::CollapsingHeader::new(tr("Precise Position"))
            .id_salt("vr_precise_position")
            .show(ui, |ui| {
                ModernSlider::new(
                    &tr("Height Offset"),
                    &mut settings.vertical_offset_meters,
                    -0.8..=0.4,
                    VrOverlaySettings::DEFAULT_VERTICAL_OFFSET,
                )
                .step(0.02)
                .precision(2)
                .suffix(" m")
                .id_salt("vr_v_offset")
                .label_width(92.0)
                .show(ui);
                ModernSlider::new(
                    &tr("Overlay Width"),
                    &mut settings.overlay_width_meters,
                    0.3..=1.5,
                    VrOverlaySettings::DEFAULT_OVERLAY_WIDTH,
                )
                .step(0.02)
                .precision(2)
                .suffix(" m")
                .id_salt("vr_width")
                .label_width(92.0)
                .show(ui);
            });
    });
}

pub fn render(
    settings: &mut VrOverlaySettings,
    ui: &mut egui::Ui,
    context: VrOverlayPageContext<'_>,
) -> Vec<VrOverlayUiAction> {
    let mut actions = Vec::new();
    let lang = context.language;
    let before = settings.clone();
    settings.normalize();
    if settings.vertical_offset_meters.abs() < 1e-6 {
        settings.vertical_offset_meters = 0.0;
    }

    render_header(ui, context, &mut actions);
    ui.add_space(10.0);

    let is_wide = (ui.available_width() > ui.available_height() && ui.available_width() >= 540.0)
        || ui.available_width() >= 800.0;

    if is_wide {
        let total_width = ui.available_width();
        let left_width = (total_width * 0.46).clamp(300.0, 420.0);
        let right_width = (total_width - left_width - ui.spacing().item_spacing.x).max(240.0);
        let avail_h = ui.available_height();

        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(left_width, avail_h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("vr_overlay_landscape_controls_scroll")
                        .show(ui, |ui| {
                            render_position_card(settings, ui, lang);
                            ui.add_space(10.0);
                            render_display_card(settings, ui, lang);
                        });
                },
            );
            ui.allocate_ui_with_layout(
                egui::vec2(right_width, avail_h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("vr_overlay_landscape_preview_scroll")
                        .show(ui, |ui| {
                            let preview_h = (avail_h - 70.0).clamp(210.0, 380.0);
                            render_preview_card(settings, ui, context, &mut actions, preview_h);
                        });
                },
            );
        });
    } else {
        egui::ScrollArea::vertical()
            .id_salt("vr_overlay_page_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                render_combined_preview_and_position_card(
                    settings,
                    ui,
                    context,
                    &mut actions,
                    230.0,
                );
                ui.add_space(12.0);
                render_display_card(settings, ui, lang);
            });
    }

    if settings.vertical_offset_meters.abs() < 1e-6 {
        settings.vertical_offset_meters = 0.0;
    }
    if *settings != before {
        actions.push(VrOverlayUiAction::SettingsChanged);
    }
    actions
}

/// This is a cropped central view, not a headset FOV or distortion simulation.
/// Keep the existing centered horizontal placement: only height and size are edited.
fn preview_geometry(view: egui::Rect, settings: &VrOverlaySettings) -> (egui::Rect, egui::Vec2) {
    let focal = view.width() * 1.2;
    let distance = settings.distance_meters.max(0.5);
    let mut center = view.center()
        - egui::vec2(
            0.0,
            settings.vertical_offset_meters * view.height() * 0.65 / distance,
        );
    let width = settings.overlay_width_meters * focal / distance;
    // Keep an editable edge visible even if the configured surface is off-view.
    center.y = center.y.clamp(
        view.top() - width * 0.25 + 32.0,
        view.bottom() + width * 0.25 - 32.0,
    );
    (
        egui::Rect::from_center_size(center, egui::vec2(width, width * 0.5)),
        egui::vec2(focal / distance, view.height() * 0.65 / distance),
    )
}

fn spatial_preview(
    ui: &mut egui::Ui,
    settings: &mut VrOverlaySettings,
    cards: &[VrSubtitleCard],
    preview_height: f32,
) {
    let (view, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), preview_height),
        egui::Sense::hover(),
    );
    let painter = ui.painter().with_clip_rect(view);
    painter.rect_filled(view, 10.0, theme::surface_subtle());
    painter.line_segment(
        [
            egui::pos2(view.left() + 12.0, view.center().y),
            egui::pos2(view.right() - 12.0, view.center().y),
        ],
        egui::Stroke::new(1.0, theme::border()),
    );
    let (surface, units) = preview_geometry(view, settings);
    let visible = surface.intersect(view.shrink(8.0));
    // Place the affordance on the visible edge, including oversized/off-center surfaces.
    let handle = egui::Rect::from_center_size(visible.right_bottom(), egui::vec2(22.0, 22.0))
        .intersect(view);
    let move_response = ui
        .interact(
            visible,
            ui.id().with("vr_preview_move"),
            egui::Sense::drag(),
        )
        .on_hover_cursor(egui::CursorIcon::ResizeVertical);
    let resize = ui
        .interact(
            handle,
            ui.id().with("vr_preview_resize"),
            egui::Sense::drag(),
        )
        .on_hover_cursor(egui::CursorIcon::ResizeNwSe);
    let moving = move_response.dragged() && !resize.dragged();
    if resize.dragged() {
        let delta = resize.drag_delta();
        // Project the corner gesture onto the fixed 2:1 surface diagonal.
        let width_delta = (delta.x + delta.y * 0.5) * 1.6 / units.x;
        settings.overlay_width_meters =
            (settings.overlay_width_meters + width_delta).clamp(0.3, 1.5);
    } else if moving {
        settings.vertical_offset_meters = (settings.vertical_offset_meters
            - move_response.drag_delta().y / units.y)
            .clamp(-0.8, 0.4);
    }
    let (surface, _) = preview_geometry(view, settings);
    let mut subtitle_painter = painter.clone();
    subtitle_painter.multiply_opacity(settings.opacity);
    paint_cards(
        &subtitle_painter,
        surface,
        &cards[cards.len().saturating_sub(settings.max_items.clamp(1, 5))..],
        settings.bilingual,
        settings.font_size * (0.6 * 640.0 / surface.width()).max(1.0),
    );
    painter.rect_stroke(
        surface.intersect(view.shrink(8.0)),
        8.0,
        egui::Stroke::new(1.0, theme::primary()),
        egui::StrokeKind::Inside,
    );
    let corner = handle.center();
    painter.line_segment(
        [corner - egui::vec2(7.0, 0.0), corner - egui::vec2(0.0, 7.0)],
        egui::Stroke::new(2.0, theme::primary()),
    );
    painter.line_segment(
        [corner - egui::vec2(4.0, 0.0), corner - egui::vec2(0.0, 4.0)],
        egui::Stroke::new(2.0, theme::primary()),
    );
}

pub fn render_settings_contribution(
    settings: &mut VrOverlaySettings,
    ui: &mut egui::Ui,
    language: crate::i18n::UiLanguage,
) -> bool {
    let before = settings.clone();
    settings.normalize();
    ui.horizontal_wrapped(|ui| {
        ui.label(crate::i18n::tr(language, "Max Lines:"));
        for count in 1..=5 {
            ui.selectable_value(&mut settings.max_items, count, count.to_string());
        }
        ui.checkbox(
            &mut settings.bilingual,
            crate::i18n::tr(language, "Bilingual"),
        );
    });
    *settings != before
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extreme_settings_retain_a_visible_drag_handle_in_narrow_views() {
        for width in [280.0, 780.0] {
            for distance in [0.5, 2.5] {
                for offset in [-0.8, 0.4] {
                    for overlay_width in [0.3, 1.5] {
                        let view =
                            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 230.0));
                        let settings = VrOverlaySettings {
                            distance_meters: distance,
                            vertical_offset_meters: offset,
                            overlay_width_meters: overlay_width,
                            ..Default::default()
                        };
                        assert!(
                            preview_geometry(view, &settings)
                                .0
                                .intersect(view.shrink(8.0))
                                .is_positive()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn preview_projects_height_size_and_distance_consistently() {
        let view = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 230.0));
        let mut settings = VrOverlaySettings::default();
        let (default, scale) = preview_geometry(view, &settings);
        settings.vertical_offset_meters = 0.1;
        settings.overlay_width_meters *= 2.0;
        let (changed, _) = preview_geometry(view, &settings);
        assert!((default.center().y - changed.center().y - 0.1 * scale.y).abs() < 0.01);
        assert_eq!(changed.width(), default.width() * 2.0);
        settings.distance_meters *= 2.0;
        assert_eq!(preview_geometry(view, &settings).0.width(), default.width());
    }

    #[test]
    fn recenter_button_wraps_and_dispatches_in_every_language() {
        let driver = crate::ui::automation::driver();
        for language in crate::i18n::UiLanguage::ALL {
            for width in [280.0, 780.0] {
                let ctx = egui::Context::default();
                crate::ui::fonts::configure_multilingual_fonts(&ctx);
                crate::ui::theme::install_context(&ctx, Default::default());
                let mut settings = VrOverlaySettings::default();
                let status = VrRuntimeStatus {
                    steamvr_connected: true,
                    avatar_available: true,
                    ..Default::default()
                };
                let label = crate::i18n::tr(language, "Come here");
                let mut actions = Vec::new();
                // Settle wrapped egui layout before clicking the visible control.
                let mut pointer = egui::Pos2::ZERO;
                for frame in 0..5 {
                    if frame == 2 {
                        let state = driver.frame_state.lock().unwrap();
                        let rect = state.last_snapshot.find_element(label).unwrap().rect;
                        pointer = egui::pos2(rect[0] + rect[2] * 0.5, rect[1] + rect[3] * 0.5);
                    }
                    let mut events = Vec::new();
                    if frame >= 2 {
                        events.push(egui::Event::PointerMoved(pointer));
                    }
                    if frame >= 3 {
                        events.push(egui::Event::PointerButton {
                            pos: pointer,
                            button: egui::PointerButton::Primary,
                            pressed: frame == 3,
                            modifiers: egui::Modifiers::NONE,
                        });
                    }
                    driver.begin_frame(&ctx, "vr_overlay");
                    let mut output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 800.0),
                            )),
                            events,
                            ..Default::default()
                        },
                        |ui| {
                            actions.extend(render(
                                &mut settings,
                                ui,
                                VrOverlayPageContext {
                                    language,
                                    status: &status,
                                },
                            ));
                        },
                    );
                    output.textures_delta.clear();
                    driver.finish_frame("vr_overlay");
                }
                assert!(
                    actions.contains(&VrOverlayUiAction::RecenterAvatar),
                    "{language:?}, width={width}, pointer={pointer:?}, actions={actions:?}, widgets={:?}",
                    driver.frame_state.lock().unwrap().last_snapshot.elements
                );
                assert!(!actions.contains(&VrOverlayUiAction::SettingsChanged));
                let state = driver.frame_state.lock().unwrap();
                let button = state.last_snapshot.find_element(label).unwrap();
                assert!(button.enabled);
                assert!(button.rect.iter().all(|value| value.is_finite()));
                assert!(button.rect[0] >= 0.0);
                assert!(
                    button.rect[0] + button.rect[2] <= width + 1.0,
                    "{language:?} at {width}: {:?}",
                    button.rect
                );
            }
        }
    }
}
