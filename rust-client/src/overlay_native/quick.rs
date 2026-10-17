//! Shortcut translation uses the floating window's renderer, input mask and placement.
use super::{WindowStacking, fit_rect, platform::InputRegion, send_event};
use crate::{
    i18n::{UiLanguage, tr},
    overlay_ipc::{OverlayEvent, QuickTranslationAction as Action, QuickTranslationState},
};
use eframe::egui::{self, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use std::time::{Duration, Instant};

const DIAMETER: f32 = 56.0;
const MARGIN: f32 = 12.0;
const MAX_MENU_HEIGHT: f32 = 268.0;
const INK: Color32 = Color32::from_rgb(48, 56, 68);
const ACCENT: Color32 = Color32::from_rgb(79, 112, 163);

#[derive(Clone, Copy, PartialEq)]
enum Menu {
    Languages,
    Folders,
}

pub(super) struct Capsule {
    state: QuickTranslationState,
    started: Instant,
    menu: Option<Menu>,
    saving_to: Option<i64>,
    focused: bool,
    anchor: Option<Pos2>,
    last_rect: Option<Rect>,
    dragging: bool,
    shown: bool,
    dismissed: bool,
    copied: Option<Instant>,
    expanded_size: Vec2,
    expansion_ready: bool,
    stacking: WindowStacking,
}
impl Default for Capsule {
    fn default() -> Self {
        Self {
            state: Default::default(),
            started: Instant::now(),
            menu: None,
            saving_to: None,
            focused: false,
            anchor: None,
            last_rect: None,
            dragging: false,
            shown: false,
            dismissed: false,
            copied: None,
            expanded_size: Vec2::splat(DIAMETER),
            expansion_ready: false,
            stacking: Default::default(),
        }
    }
}
impl Capsule {
    pub fn update(&mut self, state: QuickTranslationState) {
        if state.id != self.state.id {
            self.saving_to = None;
            self.copied = None;
            self.started = Instant::now();
            self.menu = None;
            self.dismissed = false;
            self.expansion_ready = false;
        } else if state.copy_count != self.state.copy_count {
            self.copied = Some(Instant::now());
        }
        if !state.saving
            && let Some(folder) = self.saving_to
        {
            if state.folders.iter().any(|f| f.id == folder && f.saved) && state.save_error.is_none()
            {
                self.menu = None;
                self.saving_to = None;
            } else if state.save_error.is_some() {
                self.saving_to = None;
            }
        }
        self.state = state;
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        frame: &eframe::Frame,
        input: &mut InputRegion,
        language: UiLanguage,
    ) -> Result<(), String> {
        let Some(window) = frame.winit_window() else {
            return Ok(());
        };
        let ctx = ui.ctx().clone();
        if self.dismissed {
            window.set_visible(false);
            return Ok(());
        }
        if window.has_focus() {
            self.focused = true;
        } else if self.focused && self.shown {
            self.close();
            window.set_visible(false);
            return Ok(());
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if self.menu.is_some() {
                self.menu = None;
            } else {
                self.close();
                return Ok(());
            }
        }
        let scale = ctx.pixels_per_point();
        let work = (input.work_area(window) / scale).shrink(8.0);
        let position = window.inner_position().map_err(|e| e.to_string())?;
        let position = egui::pos2(position.x as f32, position.y as f32) / scale;
        let anchor = self.anchor.get_or_insert_with(|| {
            let pointer = input
                .pointer_position(window, scale)
                .unwrap_or(egui::pos2(80.0, 40.0));
            position + pointer.to_vec2() + egui::vec2(0.0, 36.0)
        });
        if self.dragging {
            if let Some(previous) = self.last_rect {
                *anchor += position - previous.min;
            }
            if !ctx.input(|i| i.pointer.primary_down()) {
                self.dragging = false;
            }
        }
        let text = match self.state.error.as_deref() {
            Some(error) => crate::ui::notifications::error_text(error, language),
            None => crate::i18n::tr_dynamic(language, &self.state.translated),
        };
        let max_width = (work.width().min(700.0) - 200.0).max(80.0);
        let galley = ui.painter().layout(
            text.into_owned(),
            FontId::proportional(18.0),
            INK,
            max_width,
        );
        let text_height = galley.size().y.min(144.0);
        if !self.state.busy {
            let full_height = (text_height + 28.0).max(DIAMETER);
            self.expanded_size = egui::vec2(
                (galley.size().x + full_height + 36.0).min(work.width() - MARGIN * 2.0),
                full_height,
            );
        }
        if self.state.busy {
            self.expansion_ready = false;
        }
        let expansion = ctx.animate_bool_with_time(
            egui::Id::new("quick_capsule_expanded"),
            self.expansion_ready,
            0.38,
        );
        let expansion = smooth(expansion);
        let height = egui::lerp(DIAMETER..=self.expanded_size.y, expansion);
        let width = egui::lerp(DIAMETER..=self.expanded_size.x, expansion);
        // Reserve only this result's measured bounds before expanding. Keep those
        // bounds through the animation, then shrink to the circle after collapsing.
        let envelope_size = if self.state.busy && expansion == 0.0 {
            Vec2::splat(DIAMETER)
        } else {
            self.expanded_size
        };
        let envelope = fit_rect(envelope_size, *anchor, work.shrink(MARGIN));
        let menu_bounds = self.menu.filter(|_| !self.state.busy).map(|kind| {
            let labels: Vec<_> = match kind {
                Menu::Folders => self
                    .state
                    .folders
                    .iter()
                    .map(|folder| folder.label(language))
                    .collect(),
                Menu::Languages => self
                    .state
                    .targets
                    .iter()
                    .map(|code| crate::language_label(language, code))
                    .collect(),
            };
            let menu_width = labels
                .iter()
                .map(|label| {
                    ui.painter()
                        .layout_no_wrap((*label).to_owned(), FontId::proportional(14.0), INK)
                        .size()
                        .x
                })
                .fold(0.0_f32, f32::max)
                + 40.0;
            let menu_width = menu_width.min(320.0).min(work.width() - MARGIN * 2.0);
            let below = (work.bottom() - MARGIN - envelope.bottom() - 8.0).max(0.0);
            let above = (envelope.top() - work.top() - MARGIN - 8.0).max(0.0);
            let menu_height = (labels.len().max(1) as f32 * 32.0 + 14.0)
                .min(MAX_MENU_HEIGHT)
                .min(below.max(above));
            Rect::from_min_size(
                egui::pos2(
                    (envelope.center().x - menu_width * 0.5)
                        .clamp(work.left() + MARGIN, work.right() - MARGIN - menu_width),
                    if below >= menu_height {
                        envelope.bottom() + 8.0
                    } else {
                        envelope.top() - 8.0 - menu_height
                    },
                ),
                egui::vec2(menu_width, menu_height),
            )
        });
        let desired = menu_bounds
            .map_or(envelope, |menu| envelope.union(menu))
            .expand(MARGIN);
        let size = desired.size();
        let actual_size = window.inner_size().to_logical::<f32>(scale as f64);
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(Vec2::splat(40.0)));
        ctx.send_viewport_cmd(egui::ViewportCommand::Resizable(false));
        let resizing =
            (egui::vec2(actual_size.width, actual_size.height) - size).length() > 1.0 / scale;
        if resizing {
            let _ = window.request_inner_size(winit::dpi::PhysicalSize::new(
                (size.x * scale).round() as u32,
                (size.y * scale).round() as u32,
            ));
        }
        let moving = position.distance(desired.min) > 1.0 / scale;
        if moving {
            window.set_outer_position(winit::dpi::PhysicalPosition::new(
                (desired.left() * scale).round() as i32,
                (desired.top() * scale).round() as i32,
            ));
        }
        self.last_rect = Some(desired);
        if !self.state.busy && !self.expansion_ready && !resizing && !moving {
            self.expansion_ready = true;
            ctx.request_repaint();
        }
        // Paint relative to the actual native origin while a configure is pending,
        // so the loading circle stays at the same screen position during preparation.
        let body = Rect::from_center_size(
            envelope.center() - position.to_vec2(),
            egui::vec2(width, height),
        );
        let painter = ui.painter();
        for (spread, alpha) in [(7.0, 4), (4.0, 7), (2.0, 10)] {
            painter.rect_filled(
                body.expand(spread).translate(egui::vec2(0.0, 2.0)),
                height * 0.5 + spread,
                Color32::from_rgba_unmultiplied(30, 42, 60, alpha),
            );
        }
        painter.rect_filled(body, height * 0.5, Color32::WHITE);
        painter.rect_stroke(
            body,
            height * 0.5,
            Stroke::new(0.75, Color32::from_gray(231)),
            egui::StrokeKind::Inside,
        );
        let mut regions = vec![(body.expand(9.0), height * 0.5 + 9.0)];
        let mut action = None;
        if self.state.busy {
            loading_glyph(
                ui.painter(),
                body.center(),
                self.started.elapsed().as_secs_f32(),
            );
            let drag = ui.interact(body, ui.id().with("quick_loading"), Sense::click_and_drag());
            if drag.drag_started() {
                self.dragging = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if drag.secondary_clicked() {
                action = Some(Action::Close);
            }
            ctx.request_repaint_after(Duration::from_millis(16));
        } else if expansion < 0.55 {
            let mut painter = ui.painter().clone();
            painter.multiply_opacity(1.0 - smooth(expansion / 0.55));
            loading_glyph(
                &painter,
                body.center(),
                self.started.elapsed().as_secs_f32(),
            );
        }
        if !self.state.busy && expansion > 0.55 {
            ui.scope(|ui| {
                ui.set_opacity(smooth((expansion - 0.55) / 0.45));
                let cap = height * 0.5 + 8.0;
                let left = Rect::from_min_size(body.min, egui::vec2(cap, height));
                let right = Rect::from_min_size(
                    egui::pos2(body.right() - cap, body.top()),
                    egui::vec2(cap, height),
                );
                let middle = Rect::from_min_max(
                    egui::pos2(left.right() + 8.0, body.top() + 12.0),
                    egui::pos2(right.left() - 8.0, body.bottom() - 12.0),
                );
                let left_response = icon(
                    ui,
                    left,
                    "quick_languages",
                    !self.state.targets.is_empty(),
                    self.menu == Some(Menu::Languages),
                    egui::include_image!("../../resources/icons/chevron-down.svg"),
                    tr(language, "Target language"),
                );
                if left_response.clicked() {
                    self.menu = if self.menu == Some(Menu::Languages) {
                        None
                    } else {
                        Some(Menu::Languages)
                    };
                    ctx.request_repaint();
                }
                let saved = self.state.folders.iter().any(|folder| folder.saved);
                let saved_label = if saved {
                    "Save to another wordbook"
                } else if self.state.saving {
                    "Saving…"
                } else if !self.state.can_save {
                    "Vocabulary is available for words and short phrases."
                } else {
                    "Save to vocabulary"
                };
                let saved_label = self.state.save_error.as_deref().unwrap_or(saved_label);
                let star = if saved {
                    egui::include_image!("../../resources/icons/star-filled.svg")
                } else {
                    egui::include_image!("../../resources/icons/star.svg")
                };
                let save = icon(
                    ui,
                    right,
                    "quick_save",
                    self.state.can_save && !self.state.saving,
                    saved || self.menu == Some(Menu::Folders),
                    star,
                    crate::i18n::tr_dynamic(language, saved_label).as_ref(),
                );
                if save.clicked() {
                    if !saved {
                        action = Some(Action::Save(
                            crate::history::archive::vocabulary::DEFAULT_FOLDER,
                        ));
                    } else if self.menu == Some(Menu::Folders) {
                        self.menu = None;
                    } else {
                        self.menu = Some(Menu::Folders);
                        action = Some(Action::Folders);
                    }
                    ctx.request_repaint();
                }
                let result = ui
                    .interact(middle, ui.id().with("quick_text"), Sense::click_and_drag())
                    .on_hover_cursor(egui::CursorIcon::Grab);
                result.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::Button,
                        self.state.error.is_none(),
                        self.state
                            .copy_error
                            .as_deref()
                            .unwrap_or(tr(language, "Click to copy · Drag to move · Esc to close")),
                    )
                });
                if galley.size().y > middle.height() {
                    ui.scope_builder(egui::UiBuilder::new().max_rect(middle), |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("quick_text_scroll")
                            .max_height(middle.height())
                            .show(ui, |ui| {
                                ui.add(egui::Label::new(galley.clone()).sense(Sense::hover()));
                            });
                    });
                } else {
                    ui.painter().with_clip_rect(middle).galley(
                        middle.center() - galley.size() * 0.5,
                        galley,
                        INK,
                    );
                }
                if result.clicked() {
                    if self.state.error.is_none() {
                        action = Some(Action::Copy);
                    }
                }
                if result.drag_started() {
                    self.dragging = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                if result.secondary_clicked() {
                    action = Some(Action::Close);
                }
                let copied = self
                    .copied
                    .is_some_and(|at| at.elapsed() < Duration::from_millis(900));
                if copied || self.state.copy_error.is_some() {
                    ui.painter().line_segment(
                        [
                            body.center_bottom() + egui::vec2(-8.0, -5.0),
                            body.center_bottom() + egui::vec2(8.0, -5.0),
                        ],
                        Stroke::new(
                            2.0,
                            if self.state.copy_error.is_some() {
                                Color32::from_rgb(191, 91, 91)
                            } else {
                                ACCENT
                            },
                        ),
                    );
                    if copied {
                        ctx.request_repaint_after(Duration::from_millis(100));
                    }
                }
            });
        }
        if let Some(menu) = menu_bounds.filter(|menu| menu.is_positive()) {
            let menu = menu.translate(-position.to_vec2());
            let menu_height = menu.height();
            ui.scope_builder(egui::UiBuilder::new().max_rect(menu), |ui| {
                ui.style_mut().visuals = egui::Visuals::light();
                ui.spacing_mut().item_spacing.y = 0.0;
                egui::Frame::new()
                    .fill(Color32::WHITE)
                    .corner_radius(16)
                    .inner_margin(6)
                    .stroke(Stroke::new(0.75, Color32::from_gray(231)))
                    .show(ui, |ui| {
                        ui.set_width(menu.width() - 14.0);
                        egui::ScrollArea::vertical()
                            .id_salt("quick_options")
                            .max_height(menu_height - 14.0)
                            .show(ui, |ui| {
                                ui.set_width(menu.width() - 14.0);
                                if self.menu == Some(Menu::Folders) {
                                    for folder in &self.state.folders {
                                        if option(
                                            ui,
                                            folder.label(language),
                                            folder.saved,
                                            !self.state.saving,
                                        )
                                        .clicked()
                                        {
                                            action = Some(Action::Save(folder.id));
                                        }
                                    }
                                } else {
                                    for code in &self.state.targets {
                                        if option(
                                            ui,
                                            &crate::language_label(language, code),
                                            *code == self.state.target_lang,
                                            true,
                                        )
                                        .clicked()
                                        {
                                            action = Some(Action::Target(code.clone()));
                                        }
                                    }
                                }
                            });
                    });
            });
            regions.push((menu, 16.0));
        }
        input.update(&regions, scale)?;
        if !self.shown && !resizing && !moving {
            window.set_visible(true);
            window.focus_window();
            self.shown = true;
        }
        self.stacking.update(&ctx);
        if let Some(action) = action {
            if let Action::Save(folder) = &action {
                self.saving_to = Some(*folder);
            }
            if !matches!(action, Action::Folders | Action::Save(_)) {
                self.menu = None;
            }
            if action == Action::Close {
                self.close();
                window.set_visible(false);
            } else {
                send_event(OverlayEvent::QuickTranslation {
                    id: self.state.id.clone(),
                    action,
                });
            }
        }
        if expansion < 1.0 || !self.shown || resizing || moving {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        Ok(())
    }
    fn close(&mut self) {
        self.dismissed = true;
        send_event(OverlayEvent::QuickTranslation {
            id: self.state.id.clone(),
            action: Action::Close,
        });
    }
}

fn option(ui: &mut egui::Ui, label: &str, selected: bool, enabled: bool) -> egui::Response {
    let (rect, response) = ui
        .add_enabled_ui(enabled, |ui| {
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 32.0), Sense::click())
        })
        .inner;
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    let hover = ui.ctx().animate_bool_with_time(
        response.id.with("hover"),
        enabled && response.hovered(),
        0.12,
    );
    if hover > 0.0 {
        ui.painter().rect_filled(
            rect,
            10,
            Color32::WHITE.lerp_to_gamma(Color32::from_rgb(239, 243, 248), hover),
        );
    }
    let color = if selected { ACCENT } else { INK };
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), FontId::proportional(14.0), color);
    let text_rect = rect.shrink2(egui::vec2(12.0, 0.0));
    ui.painter().with_clip_rect(text_rect).galley(
        egui::pos2(text_rect.left(), rect.center().y - galley.size().y * 0.5),
        galley,
        color,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn icon(
    ui: &mut egui::Ui,
    rect: Rect,
    id: &str,
    enabled: bool,
    active: bool,
    svg: egui::ImageSource<'static>,
    label: &str,
) -> egui::Response {
    let response = ui
        .add_enabled_ui(enabled, |ui| {
            ui.interact(rect, ui.id().with(id), Sense::click())
        })
        .inner;
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    let circle = Rect::from_center_size(rect.center(), Vec2::splat(34.0));
    if active || response.hovered() {
        ui.painter()
            .circle_filled(circle.center(), 17.0, Color32::from_rgb(239, 244, 250));
    }
    egui::Image::new(svg)
        .tint(if active {
            ACCENT
        } else if enabled {
            INK
        } else {
            Color32::from_gray(193)
        })
        .paint_at(ui, Rect::from_center_size(rect.center(), Vec2::splat(19.0)));
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}
fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn loading_glyph(painter: &egui::Painter, center: Pos2, elapsed: f32) {
    use std::f32::consts::{FRAC_PI_2, TAU};
    let phase = elapsed % 4.8;
    let rotation = TAU * 2.0 * smooth(phase / 2.4);
    let morph = smooth((phase - 2.4) / 0.7) * (1.0 - smooth((phase - 4.0) / 0.8));
    // Four equally sampled strokes: dot, horizontal, left-falling, right-falling.
    let strokes = [
        [[-1.0, -13.0], [0.0, -12.0], [2.0, -10.0], [3.0, -9.0]],
        [[-12.0, -6.0], [-4.0, -6.0], [4.0, -6.0], [12.0, -6.0]],
        [[6.0, -5.0], [3.0, 3.0], [-3.0, 9.0], [-12.0, 13.0]],
        [[-6.0, -4.0], [-3.0, 3.0], [4.0, 10.0], [13.0, 13.0]],
    ];
    for (index, stroke) in strokes.iter().enumerate() {
        let points = (0..=32)
            .map(|step| {
                let t = step as f32 / 32.0;
                let angle = rotation - FRAC_PI_2 + index as f32 * FRAC_PI_2 + t * 1.08;
                let arc = egui::vec2(angle.cos(), angle.sin()) * 13.0;
                let p = stroke.map(|p| egui::vec2(p[0], p[1]));
                let glyph = p[0] * (1.0 - t).powi(3)
                    + p[1] * (3.0 * (1.0 - t).powi(2) * t)
                    + p[2] * (3.0 * (1.0 - t) * t * t)
                    + p[3] * t.powi(3);
                center + arc * (1.0 - morph) + glyph * morph
            })
            .collect::<Vec<_>>();
        let color = ACCENT.lerp_to_gamma(INK, morph);
        painter.add(egui::Shape::line(points.clone(), Stroke::new(2.0, color)));
        for point in [points[0], points[32]] {
            painter.circle_filled(point, 1.0, color);
        }
    }
}
