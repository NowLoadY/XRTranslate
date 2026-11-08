use eframe::egui;

/// One placement and appearance policy for player menus and their nested menus.
pub fn playback_popup(response: &egui::Response, overlay: bool) -> egui::Popup<'static> {
    let popup = egui::Popup::from_toggle_button_response(response).align(if overlay {
        egui::RectAlign::BOTTOM_END
    } else {
        egui::RectAlign::TOP_END
    });
    if overlay {
        popup.style(overlay_popup_style)
    } else {
        popup
    }
}

fn overlay_popup_style(style: &mut egui::Style) {
    let visuals = &mut style.visuals;
    let accent = egui::Color32::from_rgb(155, 220, 255);
    visuals.override_text_color = None;
    visuals.weak_text_color = Some(egui::Color32::from_white_alpha(190));
    visuals.window_fill = egui::Color32::from_black_alpha(88);
    visuals.window_stroke = egui::Stroke::NONE;
    visuals.window_shadow = egui::Shadow::NONE;
    visuals.popup_shadow = egui::Shadow::NONE;
    visuals.panel_fill = egui::Color32::TRANSPARENT;
    visuals.faint_bg_color = egui::Color32::TRANSPARENT;
    visuals.extreme_bg_color = egui::Color32::TRANSPARENT;
    visuals.text_edit_bg_color = Some(egui::Color32::TRANSPARENT);
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.bg_fill = egui::Color32::TRANSPARENT;
        widget.weak_bg_fill = egui::Color32::TRANSPARENT;
        widget.bg_stroke = egui::Stroke::NONE;
        widget.fg_stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
    }
    visuals.widgets.hovered.weak_bg_fill = egui::Color32::from_white_alpha(20);
    visuals.widgets.active.weak_bg_fill = egui::Color32::from_white_alpha(32);
    visuals.widgets.hovered.fg_stroke.color = accent;
    visuals.widgets.active.fg_stroke.color = accent;
    visuals.widgets.open.fg_stroke.color = accent;
    visuals.selection.bg_fill = egui::Color32::from_white_alpha(20);
    visuals.selection.stroke = egui::Stroke::new(0.0, accent);
}

pub fn playback_tooltip(response: egui::Response, label: &str, overlay: bool) -> egui::Response {
    if !overlay {
        return response.on_hover_text(label);
    }
    let mut tooltip = egui::Tooltip::for_enabled(&response);
    tooltip.popup = tooltip
        .popup
        .align(egui::RectAlign::BOTTOM_END)
        .style(overlay_popup_style);
    tooltip.show(|ui| {
        ui.label(label);
    });
    response
}

/// Compact volume and mute controls shared by media and meeting playback.
pub fn playback_volume(
    ui: &mut egui::Ui,
    language: crate::i18n::UiLanguage,
    volume: &mut f32,
    muted: &mut bool,
    width: f32,
    overlay: bool,
) -> bool {
    let mut changed = false;
    ui.allocate_ui_with_layout(
        egui::vec2(width + 40.0, 36.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if playback_button(
                ui,
                crate::i18n::tr(language, if *muted { "Unmute" } else { "Mute" }),
                if *muted {
                    egui::include_image!("../../../resources/icons/media-muted.svg")
                } else {
                    egui::include_image!("../../../resources/icons/media-volume.svg")
                },
                overlay,
                false,
            )
            .clicked()
            {
                *muted = !*muted;
                changed = true;
            }
            let mut value = f64::from(*volume);
            let label = crate::i18n::tr(language, "Volume");
            let response = playback_slider(ui, &mut value, 1.0, width, true, overlay);
            let response =
                playback_tooltip(response, &format!("{label} {:.0}%", value * 100.0), overlay);
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Slider, ui.is_enabled(), label)
            });
            let automated = ui_director::egui::record_slider(
                ui,
                response.id,
                label,
                value,
                ui.is_enabled(),
                response.rect,
            );
            if let Some(value) = automated {
                *volume = value.clamp(0.0, 1.0) as f32;
            } else if response.changed() {
                *volume = value as f32;
            }
            changed |= response.changed() || automated.is_some();
        },
    );
    changed
}

pub fn playback_button(
    ui: &mut egui::Ui,
    label: &str,
    icon: egui::ImageSource<'static>,
    overlay: bool,
    selected: bool,
) -> egui::Response {
    let response = ui.add_sized([36.0, 36.0], egui::Button::new("").frame(false));
    let response =
        ui_director::egui::button_response(ui, response.id, label, ui.is_enabled(), response);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), label));
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
    let rect = egui::Rect::from_center_size(
        response.rect.center(),
        egui::Vec2::splat(if active { 24.0 } else { 22.0 }),
    );
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
    playback_tooltip(
        response.on_hover_cursor(egui::CursorIcon::PointingHand),
        label,
        overlay,
    )
}

pub fn playback_slider(
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
            // Touch row height must not scale the slider's artwork.
            ui.spacing_mut().interact_size.y = 20.0;
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
                    .handle_shape(egui::style::HandleShape::Circle)
                    .show_value(false)
                    .trailing_fill(true),
            )
        },
    )
    .inner
}
