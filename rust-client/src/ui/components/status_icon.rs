use crate::ui::theme;
use eframe::egui;

#[derive(Clone, Copy)]
pub(crate) enum StatusIcon {
    Connected,
    Disconnected,
    Working,
    Recording,
    Paused,
    Complete,
    Imported,
    Warning,
    Idle,
}

impl StatusIcon {
    fn artwork(self) -> egui::ImageSource<'static> {
        match self {
            Self::Connected => egui::include_image!("../../../resources/icons/state-connected.svg"),
            Self::Disconnected => {
                egui::include_image!("../../../resources/icons/state-disconnected.svg")
            }
            Self::Working => egui::include_image!("../../../resources/icons/state-working.svg"),
            Self::Recording => egui::include_image!("../../../resources/icons/record.svg"),
            Self::Paused => egui::include_image!("../../../resources/icons/state-paused.svg"),
            Self::Complete => egui::include_image!("../../../resources/icons/state-complete.svg"),
            Self::Imported => egui::include_image!("../../../resources/icons/import.svg"),
            Self::Warning => egui::include_image!("../../../resources/icons/state-warning.svg"),
            Self::Idle => egui::include_image!("../../../resources/icons/state-idle.svg"),
        }
    }
}

/// Monochrome state symbols with a full accessible label and hover explanation.
pub(crate) fn status_icon(ui: &mut egui::Ui, state: StatusIcon, label: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(34.0), egui::Sense::hover());
    egui::Image::new(state.artwork())
        .tint(theme::text_normal())
        .paint_at(ui, rect.shrink(6.0));
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Image, true, label));
    crate::ui::automation::record_output(
        ui,
        label,
        crate::ui::automation::ElementValue::Text(label.to_owned()),
        rect,
    );
    response.on_hover_text(label)
}
