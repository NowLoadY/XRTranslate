use crate::{
    i18n::{UiLanguage, tr},
    ui::{automation, theme},
};
use eframe::egui::{self, load::SizedTexture};

const SLIDE_INTERVAL: f64 = 3.0;

#[derive(Clone, Default)]
struct ShowcaseState {
    index: usize,
    paused: bool,
    next_at: Option<f64>,
    images: ShowcaseImages,
}

#[derive(Clone, Default)]
struct ShowcaseImages {
    target: Option<usize>,
    started: Option<f64>,
    layers: Vec<ImageLayer>,
}

#[derive(Clone)]
struct ImageLayer {
    index: usize,
    texture: SizedTexture,
    from_opacity: f32,
    opacity: f32,
}

impl ShowcaseImages {
    fn update(&mut self, index: usize, ready: Option<SizedTexture>, now: f64, duration: f64) {
        if let Some(started) = self.started {
            let t = if duration > 0.0 {
                ((now - started) / duration).clamp(0.0, 1.0) as f32
            } else {
                1.0
            };
            let eased = t * t * (3.0 - 2.0 * t);
            for layer in &mut self.layers {
                let target = if Some(layer.index) == self.target {
                    1.0
                } else {
                    0.0
                };
                layer.opacity = layer.from_opacity + (target - layer.from_opacity) * eased;
            }
            if t >= 1.0 {
                self.layers.retain(|layer| Some(layer.index) == self.target);
                self.started = None;
            }
        }
        // Loading never replaces the visible artwork. Retarget from the current
        // blend so even rapid navigation keeps every layer's opacity continuous.
        if let Some(texture) = ready
            && self.target != Some(index)
        {
            for layer in &mut self.layers {
                layer.from_opacity = layer.opacity;
            }
            if !self.layers.iter().any(|layer| layer.index == index) {
                self.layers.push(ImageLayer {
                    index,
                    texture,
                    from_opacity: 0.0,
                    opacity: 0.0,
                });
            }
            self.target = Some(index);
            self.started = Some(now);
        }
    }
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
    let id = ui.make_persistent_id("showcase");
    let mut preview = ui
        .data(|data| data.get_temp::<ShowcaseState>(id))
        .unwrap_or_default();
    preview.index = preview.index.min(slides.len() - 1);
    let now = ui.input(|input| input.time);
    let height = (ui.ctx().content_rect().height() * 0.34).clamp(180.0, 320.0) + 40.0;
    let image_size = egui::vec2((ui.available_width() - 32.0).max(1.0), height);
    // Start decoding every slide on the existing asynchronous image loader and
    // warm its texture cache before navigation needs it. No loading UI is drawn.
    let images: Vec<_> = slides
        .iter()
        .map(|slide| egui::Image::new(slide.image.clone()).load_for_size(ui.ctx(), image_size))
        .collect();
    let ready = match images[preview.index] {
        Ok(egui::load::TexturePoll::Ready { texture }) => Some(texture),
        _ => None,
    };
    preview.images.update(
        preview.index,
        ready,
        now,
        f64::from(theme::animation_timings(ui.ctx()).page),
    );
    let slide = &slides[preview.images.target.unwrap_or(preview.index)];
    let title = tr(language, slide.title);
    let mut selected = None;
    let mut focused = false;
    let radius = theme::container_radius(16);
    let card = egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, egui::Color32::BLACK))
        .corner_radius(radius)
        .inner_margin(16)
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
                ui.label(
                    egui::RichText::new(title)
                        .strong()
                        .color(theme::text_strong()),
                );
            }
            ui.add(egui::Label::new(tr(language, slide.description)).wrap());
            ui.add_space(4.0);
            let width = ui.available_width();
            let (image_area, _) =
                ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
            for layer in &preview.images.layers {
                let slide = &slides[layer.index];
                ui.place(
                    image_area,
                    egui::Image::from_texture(layer.texture)
                        .fit_to_exact_size(image_area.size())
                        .tint(egui::Color32::WHITE.gamma_multiply(layer.opacity))
                        .show_loading_spinner(false)
                        .alt_text(tr(language, slide.detail.unwrap_or(slide.title)))
                        .corner_radius(theme::container_radius(8)),
                );
            }
            if preview.images.layers.is_empty() && images[preview.index].is_err() {
                // Preserve egui's error details and accessible fallback on failure.
                ui.place(
                    image_area,
                    egui::Image::new(slide.image.clone())
                        .fit_to_exact_size(image_area.size())
                        .show_loading_spinner(false)
                        .alt_text(tr(language, slide.detail.unwrap_or(slide.title))),
                );
            }
            if preview.images.started.is_some() && ui.is_rect_visible(image_area) {
                ui.ctx().request_repaint();
            }
            // Leave the credits along the bottom edge of the artwork visible.
            let controls_rect = egui::Rect::from_min_max(
                egui::pos2(image_area.left() + 8.0, image_area.bottom() - 60.0),
                egui::pos2(image_area.right() - 8.0, image_area.bottom() - 24.0),
            );
            let mut controls = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt("preview_controls")
                    .max_rect(controls_rect)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            controls.set_clip_rect(image_area.intersect(ui.clip_rect()));
            if let Some(detail) = slide.detail {
                controls.add(
                    egui::Label::new(
                        egui::RichText::new(tr(language, detail))
                            .strong()
                            .color(theme::text_strong()),
                    )
                    .truncate(),
                );
            }
            if slides.len() == 1 {
                return;
            }
            controls.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let playback = flat_control(
                    ui,
                    tr(language, if preview.paused { "Play" } else { "Pause" }),
                    if preview.paused {
                        egui::include_image!("../../../resources/icons/play.svg")
                    } else {
                        egui::include_image!("../../../resources/icons/pause.svg")
                    },
                );
                focused |= playback.has_focus();
                if playback.clicked() {
                    preview.paused = !preview.paused;
                    preview.next_at = None;
                }
                let next = flat_control(
                    ui,
                    tr(language, "Next"),
                    egui::include_image!("../../../resources/icons/chevron-right.svg"),
                );
                focused |= next.has_focus();
                if next.clicked() {
                    selected = Some((preview.index + 1) % slides.len());
                }
                let previous = flat_control(
                    ui,
                    tr(language, "Previous"),
                    egui::include_image!("../../../resources/icons/chevron-left.svg"),
                );
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
                        egui::Role::RadioButton,
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
        || preview.images.target != Some(preview.index)
        || preview.images.started.is_some()
        || preview.paused
        || focused
        || !ui.is_visible()
        || ui.rect_contains_pointer(card.response.rect)
        || !ui.is_rect_visible(card.response.rect)
        || !ui.input(|input| input.focused)
    {
        preview.next_at = None;
    } else {
        let next = preview.next_at.get_or_insert(now + SLIDE_INTERVAL);
        if now >= *next {
            preview.index = (preview.index + 1) % slides.len();
            *next = now + SLIDE_INTERVAL;
            ui.ctx().request_repaint();
        }
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f64(*next - now));
    }
    ui.data_mut(|data| data.insert_temp(id, preview));
}

fn flat_control(
    ui: &mut egui::Ui,
    label: &str,
    icon: egui::ImageSource<'static>,
) -> egui::Response {
    let response = ui.add_sized([32.0, 32.0], egui::Button::new("").frame(false));
    let response = automation::button_response(ui, response.id, label, ui.is_enabled(), response);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), label));
    ui.painter().circle_filled(
        response.rect.center(),
        16.0,
        if response.hovered() || response.has_focus() {
            egui::Color32::WHITE
        } else {
            egui::Color32::from_white_alpha(170)
        },
    );
    egui::Image::new(icon)
        .tint(theme::text_strong())
        .paint_at(ui, response.rect.shrink(8.0));
    response.on_hover_text(label)
}
