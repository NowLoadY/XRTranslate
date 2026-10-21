use eframe::egui;

/// Compact volume and mute controls shared by media and meeting playback.
pub(crate) fn playback_volume(
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
            let response = playback_slider(ui, &mut value, 1.0, width, true, overlay)
                .on_hover_text(format!("{label} {:.0}%", value * 100.0));
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Slider, ui.is_enabled(), label)
            });
            let automated = crate::ui::automation::record_slider(
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

pub(crate) fn playback_button(
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

pub(crate) fn playback_slider(
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
