//! User interface for the SteamVR overlay plugin page and settings contribution.

use eframe::egui;
use crate::ui::components::card;
use crate::ui::theme;
use super::runtime::{VrOverlaySettings, VrRuntimeStatus};

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
}

pub fn render(
    settings: &mut VrOverlaySettings,
    ui: &mut egui::Ui,
    context: VrOverlayPageContext<'_>,
) -> Vec<VrOverlayUiAction> {
    let mut actions = Vec::new();
    let lang = context.language;

    egui::ScrollArea::vertical()
        .id_salt("vr_overlay_page_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(4.0);

            // Title & Status Header
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(crate::i18n::tr(lang, "SteamVR In-Game Overlay"))
                        .size(20.0)
                        .color(theme::text_strong())
                        .strong(),
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if context.status.steamvr_connected {
                        if crate::ui::components::animated_button(
                            ui,
                            crate::i18n::tr(lang, "Disconnect"),
                        )
                        .clicked()
                        {
                            actions.push(VrOverlayUiAction::DisconnectSteamVr);
                        }
                        crate::ui::components::status_badge(
                            ui,
                            &crate::i18n::tr(lang, "SteamVR Connected"),
                            true,
                            false,
                        );
                    } else {
                        if crate::ui::components::animated_button(
                            ui,
                            crate::i18n::tr(lang, "Connect SteamVR"),
                        )
                        .clicked()
                        {
                            actions.push(VrOverlayUiAction::ConnectSteamVr);
                        }
                        let (status_text, is_active, is_error) = if context.status.steamvr_installed {
                            (crate::i18n::tr(lang, "Not Connected"), false, false)
                        } else {
                            (crate::i18n::tr(lang, "SteamVR not detected"), false, true)
                        };
                        crate::ui::components::status_badge(ui, &status_text, is_active, is_error);
                    }
                });
            });

            ui.label(
                egui::RichText::new(crate::i18n::tr(
                    lang,
                    "Overlay private bilingual real-time subtitles in VR games (HMD-locked mode, safe from anti-cheat).",
                ))
                .size(13.0)
                .color(theme::text_weak()),
            );

            ui.add_space(10.0);

            if let Some(error) = &context.status.last_error {
                card(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("⚠").color(egui::Color32::from_rgb(220, 38, 38)),
                        );
                        ui.label(
                            egui::RichText::new(error).color(egui::Color32::from_rgb(220, 38, 38)),
                        );
                    });
                });
                ui.add_space(6.0);
            }

            ui.add_space(6.0);

            // 1. Controls & Settings Card
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(crate::i18n::tr(lang, "Display Settings"))
                            .size(15.0)
                            .color(theme::text_strong())
                            .strong(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let mut reset_all = crate::ui::components::reset_button(ui, "vr_display_all_reset");
                        if reset_all.clicked() {
                            settings.max_items = VrOverlaySettings::DEFAULT_MAX_ITEMS;
                            settings.bilingual = true;
                            settings.font_size = VrOverlaySettings::DEFAULT_FONT_SIZE;
                            settings.opacity = VrOverlaySettings::DEFAULT_OPACITY;
                            settings.display_timeout_seconds = VrOverlaySettings::DEFAULT_TIMEOUT;
                            reset_all.mark_changed();
                            actions.push(VrOverlayUiAction::SettingsChanged);
                        }
                    });
                });
                ui.add_space(8.0);

                let mut changed = false;

                // Max Items
                if slider_with_reset_usize(
                    ui,
                    &crate::i18n::tr(lang, "Max Subtitle Count"),
                    &mut settings.max_items,
                    1..=5,
                    VrOverlaySettings::DEFAULT_MAX_ITEMS,
                    &crate::i18n::tr(lang, "lines"),
                    "vr_max_items",
                ) {
                    changed = true;
                }

                // Bilingual Toggle
                ui.horizontal(|ui| {
                    let label_w = 120.0;
                    ui.allocate_ui_with_layout(
                        egui::Vec2::new(label_w, 20.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.label(
                                egui::RichText::new(crate::i18n::tr(lang, "Bilingual Subtitles"))
                                    .color(theme::text_strong())
                                    .size(13.0)
                                    .strong(),
                            );
                        },
                    );
                    if ui
                        .checkbox(&mut settings.bilingual, crate::i18n::tr(lang, "Source + Target"))
                        .changed()
                    {
                        changed = true;
                    }
                });

                // Font Size
                if slider_with_reset_f32(
                    ui,
                    &crate::i18n::tr(lang, "Font Size"),
                    &mut settings.font_size,
                    12.0..=36.0,
                    1.0,
                    VrOverlaySettings::DEFAULT_FONT_SIZE,
                    " px",
                    1,
                    "vr_font_size",
                ) {
                    changed = true;
                }

                // Opacity
                if slider_opacity_with_reset(
                    ui,
                    &crate::i18n::tr(lang, "Opacity"),
                    &mut settings.opacity,
                    VrOverlaySettings::DEFAULT_OPACITY,
                    "vr_opacity",
                ) {
                    changed = true;
                }

                // Display Timeout
                if slider_with_reset_f32(
                    ui,
                    &crate::i18n::tr(lang, "Display Duration"),
                    &mut settings.display_timeout_seconds,
                    3.0..=30.0,
                    0.5,
                    VrOverlaySettings::DEFAULT_TIMEOUT,
                    " s",
                    1,
                    "vr_timeout",
                ) {
                    changed = true;
                }

                if changed {
                    actions.push(VrOverlayUiAction::SettingsChanged);
                }
            });

            ui.add_space(14.0);

            // 2. Spatial HUD Positioning Card
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(crate::i18n::tr(
                            lang,
                            "VR Spatial HUD Position (HMD-Locked)",
                        ))
                        .size(15.0)
                        .color(theme::text_strong())
                        .strong(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let mut reset_all = crate::ui::components::reset_button(ui, "vr_spatial_all_reset");
                        if reset_all.clicked() {
                            settings.distance_meters = VrOverlaySettings::DEFAULT_DISTANCE;
                            settings.vertical_offset_meters = VrOverlaySettings::DEFAULT_VERTICAL_OFFSET;
                            settings.overlay_width_meters = VrOverlaySettings::DEFAULT_OVERLAY_WIDTH;
                            reset_all.mark_changed();
                            actions.push(VrOverlayUiAction::SettingsChanged);
                        }
                    });
                });
                ui.add_space(8.0);

                let mut changed = false;

                // Distance
                if slider_with_reset_f32(
                    ui,
                    &crate::i18n::tr(lang, "Distance in Front"),
                    &mut settings.distance_meters,
                    0.5..=2.5,
                    0.05,
                    VrOverlaySettings::DEFAULT_DISTANCE,
                    " m",
                    2,
                    "vr_dist",
                ) {
                    changed = true;
                }

                // Vertical offset
                if slider_with_reset_f32(
                    ui,
                    &crate::i18n::tr(lang, "Height Offset"),
                    &mut settings.vertical_offset_meters,
                    -0.80..=0.40,
                    0.02,
                    VrOverlaySettings::DEFAULT_VERTICAL_OFFSET,
                    " m",
                    2,
                    "vr_v_offset",
                ) {
                    changed = true;
                }

                // Overlay Width
                if slider_with_reset_f32(
                    ui,
                    &crate::i18n::tr(lang, "Overlay Width"),
                    &mut settings.overlay_width_meters,
                    0.30..=1.50,
                    0.02,
                    VrOverlaySettings::DEFAULT_OVERLAY_WIDTH,
                    " m",
                    3,
                    "vr_width",
                ) {
                    changed = true;
                }

                if changed {
                    actions.push(VrOverlayUiAction::SettingsChanged);
                }
            });

            ui.add_space(14.0);

            // 3. Live Preview Card
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(crate::i18n::tr(lang, "Live Caption Preview"))
                            .size(15.0)
                            .color(theme::text_strong())
                            .strong(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if crate::ui::components::secondary_button(ui, crate::i18n::tr(lang, "Clear Subtitles")).clicked() {
                            actions.push(VrOverlayUiAction::ClearSubtitles);
                        }
                    });
                });

                ui.add_space(8.0);

                if let Some(preview) = &context.status.latest_caption_preview {
                    ui.label(
                        egui::RichText::new(preview)
                            .color(theme::text_strong())
                            .size(14.0),
                    );
                } else {
                    ui.label(
                        egui::RichText::new(crate::i18n::tr(lang, "No active captions (waiting for speech input)..."))
                            .color(theme::text_weak())
                            .italics(),
                    );
                }
            });
        });

    actions
}

pub fn render_settings_contribution(
    settings: &mut VrOverlaySettings,
    ui: &mut egui::Ui,
    language: crate::i18n::UiLanguage,
) -> bool {
    let mut changed = false;

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(crate::i18n::tr(language, "Max Lines:"))
                .color(theme::text_normal()),
        );
        let mut max_items = settings.max_items;
        if ui.add(egui::Slider::new(&mut max_items, 1..=5)).changed() {
            settings.max_items = max_items;
            changed = true;
        }

        ui.add_space(16.0);

        if ui.checkbox(&mut settings.bilingual, crate::i18n::tr(language, "Bilingual")).changed() {
            changed = true;
        }
    });

    changed
}

fn slider_with_reset_f32(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    step: f64,
    default: f32,
    suffix: &str,
    format_precision: usize,
    id_salt: &str,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let label_w = 120.0;
        ui.allocate_ui_with_layout(
            egui::Vec2::new(label_w, 20.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(
                    egui::RichText::new(label)
                        .color(theme::text_strong())
                        .size(13.0)
                        .strong(),
                );
            },
        );

        let slider_w = (ui.available_width() - 95.0).max(60.0);
        let slider = egui::Slider::new(value, range)
            .show_value(false)
            .step_by(step)
            .trailing_fill(true);
        if ui.add_sized(egui::vec2(slider_w, 20.0), slider).changed() {
            changed = true;
        }
        ui.add_space(4.0);
        let badge_text = format!("{:.prec$}{}", *value, suffix, prec = format_precision);
        crate::ui::components::tech_numeric_badge(ui, &badge_text);
        let mut reset = crate::ui::components::reset_button(ui, id_salt);
        if reset.clicked() && (*value - default).abs() > 0.0001 {
            *value = default;
            reset.mark_changed();
            changed = true;
        }
    });
    changed
}

fn slider_with_reset_usize(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut usize,
    range: std::ops::RangeInclusive<usize>,
    default: usize,
    suffix: &str,
    id_salt: &str,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let label_w = 120.0;
        ui.allocate_ui_with_layout(
            egui::Vec2::new(label_w, 20.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(
                    egui::RichText::new(label)
                        .color(theme::text_strong())
                        .size(13.0)
                        .strong(),
                );
            },
        );

        let slider_w = (ui.available_width() - 95.0).max(60.0);
        let slider = egui::Slider::new(value, range)
            .show_value(false)
            .trailing_fill(true);
        if ui.add_sized(egui::vec2(slider_w, 20.0), slider).changed() {
            changed = true;
        }
        ui.add_space(4.0);
        let badge_text = format!("{} {}", *value, suffix);
        crate::ui::components::tech_numeric_badge(ui, &badge_text);
        let mut reset = crate::ui::components::reset_button(ui, id_salt);
        if reset.clicked() && *value != default {
            *value = default;
            reset.mark_changed();
            changed = true;
        }
    });
    changed
}

fn slider_opacity_with_reset(
    ui: &mut egui::Ui,
    label: &str,
    opacity: &mut f32,
    default: f32,
    id_salt: &str,
) -> bool {
    let mut changed = false;
    let mut percent = (*opacity * 100.0).round() as u32;
    ui.horizontal(|ui| {
        let label_w = 120.0;
        ui.allocate_ui_with_layout(
            egui::Vec2::new(label_w, 20.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(
                    egui::RichText::new(label)
                        .color(theme::text_strong())
                        .size(13.0)
                        .strong(),
                );
            },
        );

        let slider_w = (ui.available_width() - 95.0).max(60.0);
        let slider = egui::Slider::new(&mut percent, 20..=100)
            .show_value(false)
            .trailing_fill(true);
        if ui.add_sized(egui::vec2(slider_w, 20.0), slider).changed() {
            *opacity = (percent as f32) / 100.0;
            changed = true;
        }
        ui.add_space(4.0);
        let badge_text = format!("{} %", percent);
        crate::ui::components::tech_numeric_badge(ui, &badge_text);
        let mut reset = crate::ui::components::reset_button(ui, id_salt);
        let default_percent = (default * 100.0).round() as u32;
        if reset.clicked() && percent != default_percent {
            *opacity = default;
            reset.mark_changed();
            changed = true;
        }
    });
    changed
}
