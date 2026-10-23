//! Compact library actions and supported formats.
use super::*;
use crate::i18n::{UiLanguage, tr};

pub(crate) struct LibraryHint<'a> {
    pub icon: egui::ImageSource<'static>,
    pub caption: &'a str,
    pub tooltip: &'a str,
}

pub(crate) fn library_start(ui: &mut Ui, hints: &[LibraryHint<'_>], actions: impl FnOnce(&mut Ui)) {
    card(ui, |ui| {
        ui.set_width(ui.available_width());
        if ui.available_width() < 480.0 {
            actions(ui);
            ui.add_space(12.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(20.0, 8.0);
                for hint in hints {
                    library_hint(ui, hint);
                }
            });
        } else {
            ui.horizontal(|ui| {
                actions(ui);
                ui.add_space(24.0);
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center).with_main_wrap(true),
                    |ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(20.0, 8.0);
                        for hint in hints.iter().rev() {
                            library_hint(ui, hint);
                        }
                    },
                );
            });
        }
    });
    ui.add_space(16.0);
}

fn library_hint(ui: &mut Ui, hint: &LibraryHint<'_>) {
    let caption = ui.painter().layout_no_wrap(
        hint.caption.to_owned(),
        egui::FontId::proportional(11.0),
        theme::text_weak(),
    );
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(caption.size().x.max(24.0), 40.0),
        egui::Sense::hover(),
    );
    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(rect.center().x, rect.top() + 11.0),
        Vec2::splat(20.0),
    );
    egui::Image::new(hint.icon.clone())
        .tint(theme::text_normal())
        .paint_at(ui, icon_rect);
    ui.painter().galley(
        egui::pos2(
            rect.center().x - caption.size().x * 0.5,
            rect.bottom() - caption.size().y,
        ),
        caption,
        theme::text_weak(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Image, true, hint.tooltip)
    });
    response.on_hover_text(hint.tooltip);
}

pub(crate) fn library_navigation(
    ui: &mut Ui,
    language: UiLanguage,
    expanded: &mut bool,
    query: &mut String,
    count: usize,
    search_hint: &str,
) -> usize {
    let height = 36.0_f32.max(ui.spacing().interact_size.y);
    ui.allocate_ui_with_layout(
        Vec2::new(ui.available_width(), height),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            if count > 3
                && icon_button(
                    ui,
                    tr(language, if *expanded { "Show recent" } else { "View all" }),
                    if *expanded {
                        egui::include_image!("../../../resources/icons/history.svg")
                    } else {
                        egui::include_image!("../../../resources/icons/view-all.svg")
                    },
                )
                .clicked()
            {
                *expanded = !*expanded;
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                search_bar(ui, query, search_hint);
            });
        },
    );
    ui.add_space(10.0);
    if *expanded || !query.trim().is_empty() {
        usize::MAX
    } else {
        3
    }
}
