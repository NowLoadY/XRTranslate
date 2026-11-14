//! A window-filling, bottom-anchored view of the existing live translation feed.
use super::history::{toolbar_button, toolbar_group};
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

    fn pause(&mut self) {
        self.following = false;
    }

    fn observe_input(&mut self, ui: &egui::Ui) {
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

    fn follow_button(&mut self, ui: &mut egui::Ui, language: crate::i18n::UiLanguage) -> bool {
        if !self.following
            && toolbar_button(
                ui,
                tr(language, "Follow latest"),
                egui::include_image!(
                    "../../../../../crates/xrtranslate-client-core/resources/icons/chevron-down.svg"
                ),
                false,
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
    ui.visuals_mut().hyperlink_color = foreground;
    ui.spacing_mut().scroll.fade.strength = 0.0;

    let mut back = super::history::exit_on_escape(app, ui.ctx());
    egui::Frame::new().inner_margin(12).show(ui, |ui| {
        ui.horizontal(|ui| {
            back |= toolbar_button(
                ui,
                tr(language, "Back"),
                egui::include_image!(
                    "../../../../../crates/xrtranslate-client-core/resources/icons/chevron-left.svg"
                ),
                false,
            )
            .clicked();
            let previous = app.history_display.presentation_bilingual;
            toolbar_group(ui, |ui| {
                for (bilingual, label, icon) in [
                    (
                        false,
                        "Translation only",
                        egui::include_image!("../../../../../crates/xrtranslate-client-core/resources/icons/text.svg"),
                    ),
                    (
                        true,
                        "Bilingual text",
                        egui::include_image!("../../../../../crates/xrtranslate-client-core/resources/icons/text-lines.svg"),
                    ),
                ] {
                    if toolbar_button(
                        ui,
                        tr(language, label),
                        icon,
                        app.history_display.presentation_bilingual == bilingual,
                    ).clicked() {
                        app.history_display.presentation_bilingual = bilingual;
                    }
                }
            });
            if app.history_display.presentation_bilingual != previous {
                app.history_selection.clear();
                app.history_presentation.enter();
                app.save_settings();
            }
            if toolbar_button(
                ui,
                tr(
                    language,
                    if dark {
                        "White background"
                    } else {
                        "Black background"
                    },
                ),
                egui::include_image!(
                    "../../../../../crates/xrtranslate-client-core/resources/icons/contrast.svg"
                ),
                false,
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
    let bilingual = app.history_display.presentation_bilingual;
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
            PresentationLine::new(id, entry, bilingual)
        })
        .filter(|line| !line.text.is_empty())
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

struct PresentationLine {
    id: egui::Id,
    text: String,
    ranges: Vec<std::ops::Range<usize>>,
    source_end: usize,
}

impl PresentationLine {
    fn new(id: egui::Id, entry: &crate::history::TranslationHistoryEntry, bilingual: bool) -> Self {
        let mut line = Self {
            id,
            text: String::new(),
            ranges: Vec::new(),
            source_end: 0,
        };
        if bilingual && !entry.asr_only {
            line.append(&entry.source);
            if !line.text.is_empty() {
                line.source_end = line.text.len() + 1;
            }
        }
        line.append(&entry.translated);
        for extra in &entry.additional_translations {
            line.append(&extra.translated_text);
        }
        line.source_end = line.source_end.min(line.text.len());
        line
    }

    fn append(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        if !self.text.is_empty() {
            self.text.push('\n');
        }
        let start = self.text.len();
        self.text.push_str(text);
        self.ranges.push(start..self.text.len());
    }
}

fn render_lines(
    lines: &[PresentationLine],
    state: &mut Presentation,
    selection: &mut components::selectable_text::SelectionState,
    language: crate::i18n::UiLanguage,
    backdrop: Color32,
    foreground: Color32,
    ui: &mut egui::Ui,
) {
    state.observe_input(ui);
    let viewport = ui.available_rect_before_wrap();
    let size = (viewport.width() * 0.05).clamp(30.0, 64.0);
    let width = (viewport.width() * 0.84).min(1120.0).max(1.0);
    let breathing_room = (viewport.height() * 0.13).clamp(32.0, 100.0);
    let gap = size * 0.65;
    let output = egui::ScrollArea::vertical()
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
                    0,
                    width,
                    size * 0.65,
                    foreground.gamma_multiply(0.45),
                    false,
                );
                paint_lyric(ui, galley, foreground);
            }
            for (index, line) in lines.iter().enumerate() {
                let age = lines.len() - 1 - index;
                let latest = age == 0;
                let opacity = if latest {
                    1.0
                } else {
                    (0.58 - (age - 1) as f32 * 0.10).max(0.24)
                };
                let galley = lyric_layout(
                    ui,
                    &line.text,
                    line.source_end,
                    width,
                    if latest { size } else { size * 0.65 },
                    foreground.gamma_multiply(opacity),
                    latest,
                );
                let response = if cfg!(any(windows, target_os = "linux")) {
                    selection.show(ui, line.id, galley, &line.ranges)
                } else {
                    paint_lyric(ui, galley, foreground)
                };
                ui_director::egui::record_text_output(ui, &line.text, &response);
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
    // Rejoin the live stream when manual scrolling reaches the bottom. Keep a
    // selected sentence stable until the selection is released or following is requested.
    let at_bottom =
        output.state.clamped_offset().y + output.inner_rect.height() + 2.0 >= output.content_size.y;
    if !state.following
        && at_bottom
        && !selection.is_active()
        && !ui.input(|input| input.pointer.primary_down())
    {
        state.following = true;
        ui.ctx().request_repaint();
    }
    fade_top(ui, viewport, backdrop);
}

fn lyric_layout(
    ui: &egui::Ui,
    text: &str,
    source_end: usize,
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
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = width;
    job.halign = egui::Align::Center;
    if source_end > 0 {
        job.append(
            &text[..source_end],
            0.0,
            egui::TextFormat {
                font_id: egui::FontId::proportional(size * 0.65),
                color: color.gamma_multiply(0.72),
                line_height: Some(size * 0.91),
                ..Default::default()
            },
        );
    }
    job.append(
        &text[source_end..],
        0.0,
        egui::TextFormat {
            font_id: font,
            color,
            line_height: Some(size * 1.4),
            ..Default::default()
        },
    );
    ui.fonts_mut(|fonts| fonts.layout_job(job))
}

fn paint_lyric(
    ui: &mut egui::Ui,
    galley: std::sync::Arc<egui::Galley>,
    color: Color32,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), galley.size().y),
        egui::Sense::hover(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, galley.text()));
    if ui.is_rect_visible(rect) {
        ui.painter().galley(rect.center_top(), galley, color);
    }
    response
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

#[cfg(test)]
mod tests;
