//! User interface for the SteamVR overlay plugin page and settings contribution.

use eframe::egui;
use crate::ui::components::{card, ModernSlider};
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
                crate::ui::components::error_notice(ui, lang, error);
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
                if ModernSlider::new(
                    &crate::i18n::tr(lang, "Max Subtitle Count"),
                    &mut settings.max_items,
                    1..=5,
                    VrOverlaySettings::DEFAULT_MAX_ITEMS,
                )
                .step(1.0)
                .suffix(format!(" {}", crate::i18n::tr(lang, "lines")))
                .id_salt("vr_max_items")
                .label_width(120.0)
                .show(ui)
                .changed()
                {
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
                if ModernSlider::new(
                    &crate::i18n::tr(lang, "Font Size"),
                    &mut settings.font_size,
                    12.0..=36.0,
                    VrOverlaySettings::DEFAULT_FONT_SIZE,
                )
                .step(1.0)
                .precision(1)
                .suffix(" px")
                .id_salt("vr_font_size")
                .label_width(120.0)
                .show(ui)
                .changed()
                {
                    changed = true;
                }

                // Opacity
                if ModernSlider::new(
                    &crate::i18n::tr(lang, "Opacity"),
                    &mut settings.opacity,
                    0.20..=1.00,
                    VrOverlaySettings::DEFAULT_OPACITY,
                )
                .step(0.01)
                .percentage(true)
                .id_salt("vr_opacity")
                .label_width(120.0)
                .show(ui)
                .changed()
                {
                    changed = true;
                }

                // Display Timeout
                if ModernSlider::new(
                    &crate::i18n::tr(lang, "Display Duration"),
                    &mut settings.display_timeout_seconds,
                    3.0..=30.0,
                    VrOverlaySettings::DEFAULT_TIMEOUT,
                )
                .step(0.5)
                .precision(1)
                .suffix(" s")
                .id_salt("vr_timeout")
                .label_width(120.0)
                .show(ui)
                .changed()
                {
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
                if ModernSlider::new(
                    &crate::i18n::tr(lang, "Distance in Front"),
                    &mut settings.distance_meters,
                    0.5..=2.5,
                    VrOverlaySettings::DEFAULT_DISTANCE,
                )
                .step(0.05)
                .precision(2)
                .suffix(" m")
                .id_salt("vr_dist")
                .label_width(120.0)
                .show(ui)
                .changed()
                {
                    changed = true;
                }

                // Vertical offset
                if ModernSlider::new(
                    &crate::i18n::tr(lang, "Height Offset"),
                    &mut settings.vertical_offset_meters,
                    -0.80..=0.40,
                    VrOverlaySettings::DEFAULT_VERTICAL_OFFSET,
                )
                .step(0.02)
                .precision(2)
                .suffix(" m")
                .id_salt("vr_v_offset")
                .label_width(120.0)
                .show(ui)
                .changed()
                {
                    changed = true;
                }

                // Overlay Width
                if ModernSlider::new(
                    &crate::i18n::tr(lang, "Overlay Width"),
                    &mut settings.overlay_width_meters,
                    0.30..=1.50,
                    VrOverlaySettings::DEFAULT_OVERLAY_WIDTH,
                )
                .step(0.02)
                .precision(2)
                .suffix(" m")
                .id_salt("vr_width")
                .label_width(120.0)
                .show(ui)
                .changed()
                {
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
