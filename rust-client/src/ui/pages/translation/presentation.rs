//! A window-filling, bottom-anchored view of the existing live translation feed.
use crate::{i18n::tr, ui::components};
use eframe::egui::{self, Color32};

#[derive(Default)]
pub(crate) struct Presentation {
    following: bool,
    first_frame: bool,
    last_layout: Option<egui::Vec2>,
}

impl Presentation {
    pub fn enter(&mut self) {
        self.following = true;
        self.first_frame = true;
        self.last_layout = None;
    }
}

pub fn background(dark: bool) -> Color32 {
    if dark { Color32::BLACK } else { Color32::WHITE }
}

pub fn render(app: &mut crate::XRTranslateApp, ui: &mut egui::Ui) {
    let language = app.ui_language;
    let dark = app.history_display.presentation_dark;
    let backdrop = background(dark);
    let foreground = background(!dark);
    // Keep this surface independent of the application's theme and wallpaper.
    *ui.visuals_mut() = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    ui.visuals_mut().panel_fill = backdrop;
    ui.visuals_mut().override_text_color = Some(foreground);
    ui.spacing_mut().scroll.fade.strength = 0.0;

    let mut back =
        ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    egui::Frame::new().inner_margin(16).show(ui, |ui| {
        ui.horizontal(|ui| {
            back |= components::icon_button(
                ui,
                tr(language, "Back"),
                egui::include_image!("../../../../resources/icons/chevron-left.svg"),
            )
            .clicked();
            if components::icon_button(
                ui,
                tr(
                    language,
                    if dark {
                        "White background"
                    } else {
                        "Black background"
                    },
                ),
                egui::include_image!("../../../../resources/icons/contrast.svg"),
            )
            .clicked()
            {
                app.history_display.presentation_dark = !dark;
                app.save_settings();
                ui.ctx().request_repaint();
            }
            if !app.history_presentation.following
                && ui.button(tr(language, "Follow latest")).clicked()
            {
                app.history_presentation.following = true;
                app.history_presentation.last_layout = None;
            }
        });
    });
    if back {
        app.fullscreen_history = false;
        ui.ctx().request_repaint();
        return;
    }

    // The same turn/segment ordering and revision selection as the normal feed
    // keeps a streamed preview in place when its final translation arrives.
    let lines = super::history::history_segments(app)
        .iter()
        .flat_map(|turn| &turn.translations)
        .map(|entry| entry.output_text())
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>();
    render_lines(
        &lines,
        &mut app.history_presentation,
        language,
        backdrop,
        foreground,
        ui,
    );
}

fn render_lines(
    lines: &[String],
    state: &mut Presentation,
    language: crate::i18n::UiLanguage,
    backdrop: Color32,
    foreground: Color32,
    ui: &mut egui::Ui,
) {
    let viewport = ui.available_rect_before_wrap();
    let manual_scroll = ui.input(|input| {
        input
            .pointer
            .hover_pos()
            .is_some_and(|pos| viewport.contains(pos))
            && (input.events.iter().any(
                |event| matches!(event, egui::Event::MouseWheel { delta, .. } if delta.y != 0.0),
            ) || (input.pointer.primary_down() && input.pointer.delta().y.abs() > 1.0))
    });
    if manual_scroll {
        state.following = false;
    }
    let size = (viewport.width() * 0.05).clamp(30.0, 64.0);
    let width = (viewport.width() * 0.84).min(1120.0).max(1.0);
    let breathing_room = (viewport.height() * 0.13).clamp(32.0, 100.0);
    let gap = size * 0.65;
    egui::ScrollArea::vertical()
        .id_salt("translation_presentation")
        .auto_shrink([false, false])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .scroll_source(egui::scroll_area::ScrollSource::ALL)
        .show(ui, |ui| {
            let origin = ui.cursor().top();
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            // A fixed lead-in lets even the first few sentences move upward
            // through a real scroll animation instead of jumping between layouts.
            ui.add_space((viewport.height() - breathing_room - size * 1.4).max(0.0));
            if lines.is_empty() {
                lyric_line(
                    ui,
                    tr(language, "No results yet"),
                    width,
                    size * 0.65,
                    foreground.gamma_multiply(0.45),
                    false,
                );
            }
            for (index, text) in lines.iter().enumerate() {
                let age = lines.len() - 1 - index;
                let latest = age == 0;
                let opacity = if latest {
                    1.0
                } else {
                    (0.58 - (age - 1) as f32 * 0.10).max(0.24)
                };
                lyric_line(
                    ui,
                    text,
                    width,
                    if latest { size } else { size * 0.65 },
                    foreground.gamma_multiply(opacity),
                    latest,
                );
                if !latest {
                    ui.add_space(gap);
                }
            }
            ui.add_space(breathing_room);
            let layout = egui::vec2(ui.cursor().top() - origin, viewport.width());
            if state.following && state.last_layout != Some(layout) {
                ui.scroll_to_cursor_animation(
                    Some(egui::Align::Max),
                    if state.first_frame {
                        egui::style::ScrollAnimation::none()
                    } else {
                        egui::style::ScrollAnimation::duration(0.5)
                    },
                );
            }
            state.last_layout = Some(layout);
            state.first_frame = false;
        });
    fade_top(ui, viewport, backdrop);
}

fn lyric_line(ui: &mut egui::Ui, text: &str, width: f32, size: f32, color: Color32, latest: bool) {
    let font = egui::FontId::new(
        size,
        if latest {
            crate::ui::fonts::bold_family()
        } else {
            egui::FontFamily::Proportional
        },
    );
    let mut job = egui::text::LayoutJob::simple(text.to_owned(), font, color, width);
    job.halign = egui::Align::Center;
    job.sections[0].format.line_height = Some(size * 1.4);
    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), galley.size().y),
        egui::Sense::hover(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, text));
    if ui.is_rect_visible(rect) {
        ui.painter()
            .galley(egui::pos2(rect.center().x, rect.top()), galley, color);
    }
}

fn fade_top(ui: &egui::Ui, viewport: egui::Rect, backdrop: Color32) {
    let bottom = viewport.top() + (viewport.height() * 0.12).min(72.0);
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(viewport.left_top(), backdrop);
    mesh.colored_vertex(viewport.right_top(), backdrop);
    mesh.colored_vertex(egui::pos2(viewport.left(), bottom), Color32::TRANSPARENT);
    mesh.colored_vertex(egui::pos2(viewport.right(), bottom), Color32::TRANSPARENT);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(2, 1, 3);
    ui.painter().add(egui::Shape::mesh(mesh));
}
