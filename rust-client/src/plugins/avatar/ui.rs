use super::AvatarPlugin;
use crate::{
    i18n::{UiLanguage, tr},
    plugins::text_output::{MessageFormat, render_banner_selector},
    ui::{automation, components, theme},
};
use eframe::egui;

impl AvatarPlugin {
    pub(crate) fn render_page(
        &mut self,
        ui: &mut egui::Ui,
        language: UiLanguage,
        lyrics: bool,
    ) -> bool {
        let before = self.settings.clone();
        let selection_id = ui.id().with("avatar_region");
        let mut selected = ui
            .data(|data| data.get_temp::<usize>(selection_id))
            .unwrap_or(1);
        egui::ScrollArea::vertical()
            .id_salt("avatar_page_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(tr(language, "Avatar"))
                        .size(22.0)
                        .color(theme::text_strong())
                        .strong(),
                );
                ui.add_space(12.0);
                let width = ui.available_width();
                if width >= 720.0 {
                    let left_width = (width * 0.46).clamp(320.0, 420.0);
                    let right_width = width - left_width - ui.spacing().item_spacing.x;
                    ui.horizontal_top(|ui| {
                        ui.allocate_ui_with_layout(
                            egui::vec2(left_width, 0.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                components::card(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    self.render_settings(ui, language);
                                });
                                ui.add_space(12.0);
                                self.render_region_settings(ui, language, lyrics, selected);
                            },
                        );
                        ui.allocate_ui_with_layout(
                            egui::vec2(right_width, 0.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| self.render_preview(ui, language, &mut selected),
                        );
                    });
                } else {
                    components::card(ui, |ui| {
                        ui.set_width(ui.available_width());
                        self.render_settings(ui, language);
                    });
                    ui.add_space(12.0);
                    self.render_preview(ui, language, &mut selected);
                    ui.add_space(12.0);
                    self.render_region_settings(ui, language, lyrics, selected);
                }
                ui.add_space(16.0);
                render_package_card(ui, language);
                ui.add_space(8.0);
            });
        ui.data_mut(|data| data.insert_temp(selection_id, selected));
        before != self.settings
    }

    fn render_preview(&mut self, ui: &mut egui::Ui, language: UiLanguage, selected: &mut usize) {
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.settings.captions, tr(language, "Display content"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if components::delete_button(ui, tr(language, "Clear messages")).clicked() {
                    self.output.clear_messages();
                }
            });
        });
        ui.add_space(8.0);
        let snapshot = self.output.preview();
        let texts = [
            snapshot.header.unwrap_or_default(),
            snapshot
                .messages
                .iter()
                .map(|message| message.text.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            snapshot.footer.unwrap_or_default(),
        ];
        components::card(ui, |ui| {
            for (index, label) in ["Header", "Messages", "Footer"].into_iter().enumerate() {
                let text = if texts[index].is_empty() {
                    tr(language, label).to_owned()
                } else {
                    texts[index].clone()
                };
                let height = if index == 1 { 160.0 } else { 42.0 };
                let button =
                    egui::Button::new(egui::RichText::new(text).color(theme::text_strong()))
                        .selected(*selected == index)
                        .wrap();
                if ui
                    .add_sized([ui.available_width(), height], button)
                    .on_hover_text(tr(language, label))
                    .clicked()
                {
                    *selected = index;
                    ui.ctx().request_repaint();
                }
            }
        });
    }

    fn render_region_settings(
        &mut self,
        ui: &mut egui::Ui,
        language: UiLanguage,
        lyrics: bool,
        selected: usize,
    ) {
        components::card(ui, |ui| {
            ui.set_width(ui.available_width());
            if selected != 1 {
                let (label, banner) = if selected == 0 {
                    ("Header:", &mut self.settings.header)
                } else {
                    ("Footer:", &mut self.settings.footer)
                };
                render_banner_selector(
                    ui,
                    tr(language, label),
                    "avatar_banner",
                    banner,
                    language,
                    lyrics,
                );
            } else {
                ui.checkbox(&mut self.settings.messages, tr(language, "Messages"));
                components::audio_source_filter(
                    ui,
                    &mut self.settings.audio_source_filter,
                    language,
                );
                ui.label(tr(language, "Format:"));
                egui::ComboBox::from_id_salt("avatar_message_format")
                    .width(ui.available_width())
                    .truncate()
                    .selected_text(self.settings.format.label(language))
                    .show_ui(ui, |ui| {
                        for mode in [
                            MessageFormat::TargetOnly,
                            MessageFormat::BilingualSourceFirst,
                            MessageFormat::BilingualTargetFirst,
                            MessageFormat::Inline,
                        ] {
                            ui.selectable_value(
                                &mut self.settings.format,
                                mode,
                                mode.label(language),
                            );
                        }
                    });
                ui.add(
                    egui::Slider::new(&mut self.settings.message_seconds, 1..=15)
                        .text(tr(language, "Display seconds")),
                );
            }
        });
    }
}

#[derive(Clone, Default)]
struct PackagePreview {
    index: usize,
    paused: bool,
    next_at: Option<f64>,
}

fn render_package_card(ui: &mut egui::Ui, language: UiLanguage) {
    let slides = [
        (
            "VRChat",
            egui::include_image!("../../../../assets/avatar-plugin-preview.jpg"),
        ),
        (
            "ChilloutVR",
            egui::include_image!("../../../../assets/avatar-plugin-chilloutvr.webp"),
        ),
        (
            "Overte",
            egui::include_image!("../../../../assets/avatar-plugin-overte.webp"),
        ),
        (
            "Resonite",
            egui::include_image!("../../../../assets/avatar-plugin-resonite.webp"),
        ),
        (
            "Second Life",
            egui::include_image!("../../../../assets/avatar-plugin-secondlife.webp"),
        ),
    ];
    let id = ui.id().with("avatar_package_preview");
    let mut preview = ui
        .data(|data| data.get_temp::<PackagePreview>(id))
        .unwrap_or_default();
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
            let response = ui.link(egui::RichText::new("XRT-Avatar-Plugin").strong());
            let enabled = ui.is_enabled();
            let response = automation::button_response(
                ui,
                response.id,
                "XRT-Avatar-Plugin",
                enabled,
                response,
            )
            .on_hover_text("GitHub · XRT-Avatar-Plugin");
            focused |= response.has_focus();
            if response.clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(
                    "https://github.com/NowLoadY/XRT-Avatar-Plugin",
                ));
            }
            ui.add(egui::Label::new(tr(language, "Add live translations to your avatar!")).wrap());
            ui.add_space(4.0);
            let width = ui.available_width();
            let height = ((ui.ctx().content_rect().height() * 0.30).clamp(160.0, 280.0) + 40.0)
                .min(width * 2.0 / 3.0);
            let (image_area, _) =
                ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
            let image_rect =
                egui::Rect::from_center_size(image_area.center(), egui::vec2(height * 1.5, height));
            ui.place(
                image_rect,
                egui::Image::new(slides[preview.index].1.clone())
                    .fit_to_exact_size(image_rect.size())
                    .alt_text(slides[preview.index].0)
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
            egui::Frame::new()
                .fill(overlay_fill)
                .corner_radius(theme::container_radius(12))
                .inner_margin(egui::Margin::symmetric(8, 4))
                .show(&mut controls, |ui| {
                    ui.add(egui::Label::new(slides[preview.index].0).truncate());
                });
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
            for (index, (game, _)) in slides.iter().enumerate() {
                let response = dots.add_sized([22.0, 24.0], egui::Button::new("").frame(false));
                let response =
                    automation::button_response(&mut dots, response.id, game, enabled, response)
                        .on_hover_text(*game)
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                response.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::WidgetType::RadioButton,
                        enabled,
                        preview.index == index,
                        *game,
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
    if preview.paused
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
    let response =
        components::compact_icon_button(ui, "avatar_preview_playback", false, label, None);
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
