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

    pub(super) fn following(&self) -> bool {
        self.following
    }

    pub(super) fn take_follow_request(&mut self) -> bool {
        self.following && std::mem::take(&mut self.first_frame)
    }

    pub(super) fn pause(&mut self) {
        self.following = false;
    }

    pub(super) fn observe_input(&mut self, ui: &egui::Ui) {
        let viewport = ui.available_rect_before_wrap();
        if ui.input(|input| {
            input.pointer.hover_pos().is_some_and(|pos| viewport.contains(pos))
                && (input.pointer.primary_down()
                    || input.events.iter().any(|event| {
                        matches!(event, egui::Event::MouseWheel { delta, .. } if delta.y != 0.0)
                    }))
        }) {
            self.pause();
        }
    }

    pub(super) fn follow_button(
        &mut self,
        ui: &mut egui::Ui,
        language: crate::i18n::UiLanguage,
    ) -> bool {
        if !self.following
            && components::icon_button(
                ui,
                tr(language, "Follow latest"),
                egui::include_image!("../../../../resources/icons/chevron-down.svg"),
            )
            .clicked()
        {
            self.enter();
            return true;
        }
        false
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

    let mut back = super::history::exit_on_escape(app, ui.ctx());
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
            if app.history_presentation.follow_button(ui, language) {
                app.history_selection.clear();
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
    app.history_presentation.observe_input(ui);
    let lines = super::history::history_segments(app)
        .iter()
        .flat_map(|turn| &turn.translations)
        .enumerate()
        .map(|(index, entry)| {
            let id = ui.make_persistent_id((
                "presentation_line",
                entry.stream_id,
                &entry.turn_id,
                entry.segment_index,
                if entry.turn_id.is_empty() { index } else { 0 },
            ));
            let mut text = entry.translated.clone();
            let mut ranges = Vec::new();
            if !text.trim().is_empty() {
                ranges.push(0..text.len());
            }
            for extra in &entry.additional_translations {
                if extra.translated_text.trim().is_empty() {
                    continue;
                }
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&extra.target_lang);
                text.push_str(" · ");
                let start = text.len();
                text.push_str(&extra.translated_text);
                ranges.push(start..text.len());
            }
            (id, text, ranges)
        })
        .filter(|(_, text, _)| !text.trim().is_empty())
        .collect::<Vec<_>>();
    render_lines(
        &lines,
        &mut app.history_presentation,
        &mut app.history_selection,
        language,
        backdrop,
        foreground,
        ui,
    );
    super::history::show_lookup(app, ui.ctx());
}

fn render_lines(
    lines: &[(egui::Id, String, Vec<std::ops::Range<usize>>)],
    state: &mut Presentation,
    selection: &mut components::selectable_text::SelectionState,
    language: crate::i18n::UiLanguage,
    backdrop: Color32,
    foreground: Color32,
    ui: &mut egui::Ui,
) {
    let viewport = ui.available_rect_before_wrap();
    let size = (viewport.width() * 0.05).clamp(30.0, 64.0);
    let width = (viewport.width() * 0.84).min(1120.0).max(1.0);
    let breathing_room = (viewport.height() * 0.13).clamp(32.0, 100.0);
    let gap = size * 0.65;
    egui::ScrollArea::vertical()
        .id_salt("translation_presentation")
        .auto_shrink([false, false])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .scroll_source(if cfg!(any(windows, target_os = "linux")) {
            egui::scroll_area::ScrollSource::SCROLL_BAR
                | egui::scroll_area::ScrollSource::MOUSE_WHEEL
        } else {
            egui::scroll_area::ScrollSource::ALL
        })
        .show(ui, |ui| {
            let origin = ui.cursor().top();
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            // A fixed lead-in lets even the first few sentences move upward
            // through a real scroll animation instead of jumping between layouts.
            ui.add_space((viewport.height() - breathing_room - size * 1.4).max(0.0));
            if lines.is_empty() {
                let galley = lyric_layout(
                    ui,
                    tr(language, "No results yet"),
                    width,
                    size * 0.65,
                    foreground.gamma_multiply(0.45),
                    false,
                );
                paint_lyric(ui, galley, foreground);
            }
            for (index, (id, text, ranges)) in lines.iter().enumerate() {
                let age = lines.len() - 1 - index;
                let latest = age == 0;
                let opacity = if latest {
                    1.0
                } else {
                    (0.58 - (age - 1) as f32 * 0.10).max(0.24)
                };
                let galley = lyric_layout(
                    ui,
                    text,
                    width,
                    if latest { size } else { size * 0.65 },
                    foreground.gamma_multiply(opacity),
                    latest,
                );
                if cfg!(any(windows, target_os = "linux")) {
                    selection.show(ui, *id, galley, ranges);
                } else {
                    paint_lyric(ui, galley, foreground);
                }
                if !latest {
                    ui.add_space(gap);
                }
            }
            ui.add_space(breathing_room);
            let layout = egui::vec2(ui.cursor().top() - origin, viewport.width());
            if selection.is_active() {
                state.pause();
            }
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

fn lyric_layout(
    ui: &egui::Ui,
    text: &str,
    width: f32,
    size: f32,
    color: Color32,
    latest: bool,
) -> std::sync::Arc<egui::Galley> {
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
    ui.fonts_mut(|fonts| fonts.layout_job(job))
}

fn paint_lyric(ui: &mut egui::Ui, galley: std::sync::Arc<egui::Galley>, color: Color32) {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), galley.size().y),
        egui::Sense::hover(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, galley.text()));
    if ui.is_rect_visible(rect) {
        ui.painter().galley(rect.center_top(), galley, color);
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
