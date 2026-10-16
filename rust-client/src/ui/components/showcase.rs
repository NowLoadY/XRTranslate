use crate::{
    i18n::{UiLanguage, tr},
    ui::{automation, components, theme},
};
use eframe::egui;

#[derive(Clone, Default)]
struct ShowcaseState {
    index: usize,
    paused: bool,
    next_at: Option<f64>,
}

pub(crate) struct ShowcaseSlide {
    pub title: &'static str,
    pub description: &'static str,
    pub image: egui::ImageSource<'static>,
    pub detail: Option<&'static str>,
    pub url: Option<&'static str>,
}

pub(crate) fn showcase(ui: &mut egui::Ui, language: UiLanguage, slides: &[ShowcaseSlide]) {
    if slides.is_empty() {
        return;
    }
    let id = ui.id().with("showcase");
    let mut preview = ui
        .data(|data| data.get_temp::<ShowcaseState>(id))
        .unwrap_or_default();
    preview.index = preview.index.min(slides.len() - 1);
    let slide = &slides[preview.index];
    let title = tr(language, slide.title);
    let mut selected = None;
    let mut focused = false;
    let radius = theme::container_radius(16);
    let card = egui::Frame::new()
        .fill(theme::panel_fill(ui.ctx(), theme::surface_subtle()))
        .stroke(egui::Stroke::new(1.0, theme::border()))
        .corner_radius(radius)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.style_mut().interaction.selectable_labels = false;
            let enabled = ui.is_enabled();
            if let Some(url) = slide.url {
                let response = ui.link(egui::RichText::new(title).strong());
                let response =
                    automation::button_response(ui, response.id, title, enabled, response)
                        .on_hover_text(url);
                focused |= response.has_focus();
                if response.clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                }
            } else {
                ui.label(egui::RichText::new(title).strong().color(theme::text_strong()));
            }
            ui.add(egui::Label::new(tr(language, slide.description)).wrap());
            ui.add_space(4.0);
            let width = ui.available_width();
            let height = (ui.ctx().content_rect().height() * 0.30).clamp(160.0, 280.0) + 40.0;
            let (image_area, _) =
                ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
            ui.place(
                image_area,
                egui::Image::new(slide.image.clone())
                    .fit_to_exact_size(image_area.size())
                    .alt_text(tr(language, slide.detail.unwrap_or(slide.title)))
                    .corner_radius(theme::container_radius(8)),
            );
            // Leave the credits along the bottom edge of the artwork visible.
            let controls_rect = egui::Rect::from_min_max(
                egui::pos2(image_area.left() + 8.0, image_area.bottom() - 60.0),
                egui::pos2(image_area.right() - 8.0, image_area.bottom() - 24.0),
            );
            let overlay_fill = theme::surface_subtle().gamma_multiply(0.92);
            let mut controls = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt("preview_controls")
                    .max_rect(controls_rect)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            controls.set_clip_rect(image_area.intersect(ui.clip_rect()));
            if let Some(detail) = slide.detail {
                egui::Frame::new()
                    .fill(overlay_fill)
                    .corner_radius(theme::container_radius(12))
                    .inner_margin(egui::Margin::symmetric(8, 4))
                    .show(&mut controls, |ui| {
                        ui.add(egui::Label::new(tr(language, detail)).truncate());
                    });
            }
            if slides.len() == 1 {
                return;
            }
            controls.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let playback = render_playback_button(ui, language, preview.paused);
                focused |= playback.has_focus();
                if playback.clicked() {
                    preview.paused = !preview.paused;
                    preview.next_at = None;
                }
                let next = components::next_button(ui, tr(language, "Next"));
                focused |= next.has_focus();
                if next.clicked() {
                    selected = Some((preview.index + 1) % slides.len());
                }
                let previous = components::back_button(ui, tr(language, "Previous"));
                focused |= previous.has_focus();
                if previous.clicked() {
                    selected = Some((preview.index + slides.len() - 1) % slides.len());
                }
            });
            let gap = ui.spacing().item_spacing.x;
            let dots_width = 22.0 * slides.len() as f32 + gap * (slides.len() - 1) as f32;
            let dots_rect = egui::Rect::from_center_size(
                egui::pos2(image_area.center().x, image_area.bottom() - 12.0),
                egui::vec2(dots_width, 24.0),
            );
            let mut dots = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt("preview_dots")
                    .max_rect(dots_rect)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            dots.set_clip_rect(image_area.intersect(ui.clip_rect()));
            for (index, slide) in slides.iter().enumerate() {
                let label = tr(language, slide.detail.unwrap_or(slide.title));
                let response = dots.add_sized([22.0, 24.0], egui::Button::new("").frame(false));
                let response =
                    automation::button_response(&mut dots, response.id, label, enabled, response)
                        .on_hover_text(label)
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                response.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::WidgetType::RadioButton,
                        enabled,
                        preview.index == index,
                        label,
                    )
                });
                focused |= response.has_focus();
                let color = if preview.index == index || response.hovered() || response.has_focus()
                {
                    theme::primary()
                } else {
                    theme::border()
                };
                dots.painter()
                    .circle_filled(response.rect.center(), 4.0, color);
                if response.clicked() {
                    selected = Some(index);
                }
            }
        });
    if let Some(index) = selected {
        preview.index = index;
        preview.next_at = None;
        ui.ctx().request_repaint();
    }
    if slides.len() == 1
        || preview.paused
        || focused
        || ui.rect_contains_pointer(card.response.rect)
        || !ui.is_rect_visible(card.response.rect)
        || !ui.input(|input| input.focused)
    {
        preview.next_at = None;
    } else {
        let now = ui.input(|input| input.time);
        let next = preview.next_at.get_or_insert(now + 8.0);
        if now >= *next {
            preview.index = (preview.index + 1) % slides.len();
            *next = now + 8.0;
            ui.ctx().request_repaint();
        }
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f64(*next - now));
    }
    ui.data_mut(|data| data.insert_temp(id, preview));
}

fn render_playback_button(ui: &mut egui::Ui, language: UiLanguage, paused: bool) -> egui::Response {
    let label = tr(language, if paused { "Play" } else { "Pause" });
    let response = components::compact_icon_button(ui, "showcase_playback", false, label, None);
    let center = response.rect.center();
    let color = theme::text_normal();
    if paused {
        ui.painter().add(egui::Shape::convex_polygon(
            vec![
                center + egui::vec2(-3.5, -6.0),
                center + egui::vec2(5.0, 0.0),
                center + egui::vec2(-3.5, 6.0),
            ],
            color,
            egui::Stroke::NONE,
        ));
    } else {
        for offset in [-3.0, 3.0] {
            ui.painter().rect_filled(
                egui::Rect::from_center_size(
                    center + egui::vec2(offset, 0.0),
                    egui::vec2(3.0, 12.0),
                ),
                1,
                color,
            );
        }
    }
    response
}
