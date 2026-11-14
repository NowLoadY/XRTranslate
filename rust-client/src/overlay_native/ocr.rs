//! Independently movable, click-through OCR selection.
use super::{GAP, WindowStacking, place_viewport, platform, resize, send_event, theme};
use crate::overlay_ipc::{OverlayEvent, OverlayRegion};
use eframe::egui::{self, Color32, Rect, Stroke, Vec2};
use std::time::{Duration, Instant};

const TITLE: &str = "XRTranslate OCR";

#[derive(Default)]
pub(super) struct OcrWindow {
    pub geometry: Option<Rect>,
    input: Option<platform::InputRegion>,
    shown: bool,
    pending: Option<(OverlayRegion, Instant)>,
    reported: Option<OverlayRegion>,
    stacking: WindowStacking,
}

impl OcrWindow {
    pub fn invalidate(&mut self) {
        self.pending = None;
        if self.reported.take().is_some() {
            send_event(OverlayEvent::RegionChanging);
        }
    }

    pub fn hide(&mut self) {
        self.invalidate();
        self.input = None;
        self.shown = false;
        self.stacking = WindowStacking::default();
    }

    pub fn show(&mut self, ctx: &egui::Context, initial: Rect) -> Result<(), String> {
        let desired = self.geometry.unwrap_or(initial);
        let mut outcome = Ok(());
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("ocr_selection"),
            egui::ViewportBuilder::default()
                .with_title(TITLE)
                .with_inner_size([440.0, 220.0])
                .with_min_inner_size([96.0, 72.0])
                .with_decorations(false)
                .with_transparent(true)
                .with_resizable(true)
                .with_always_on_top()
                .with_active(false)
                .with_taskbar(false)
                .with_visible(false),
            |ui, _| {
                outcome = self.ui(ui, desired);
            },
        );
        ctx.request_repaint_after(Duration::from_millis(100));
        outcome
    }

    fn ui(&mut self, ui: &mut egui::Ui, desired: Rect) -> Result<(), String> {
        if ui.ctx().input(|input| input.viewport().close_requested()) {
            self.hide();
            send_event(OverlayEvent::OcrEnabled(false));
            return Ok(());
        }
        if self.input.is_none() {
            self.input = Some(platform::InputRegion::for_viewport(TITLE)?);
        }
        let scale = ui.ctx().pixels_per_point();
        let actual = ui.ctx().input(|input| input.viewport().inner_rect);
        let mut regions = Vec::new();
        let (selection, moving) = recognition_area(ui, &mut regions);
        self.input.as_mut().unwrap().update(&regions, scale)?;
        place_viewport(ui.ctx(), desired, &mut self.shown);
        if self.shown
            && let Some(actual) = actual
        {
            self.stacking.update(ui.ctx());
            self.geometry = Some(actual * scale);
            if moving {
                self.invalidate();
            }
            self.observe(capture_region(
                selection.translate(actual.min.to_vec2()) * scale,
            ));
        }
        Ok(())
    }

    fn observe(&mut self, region: OverlayRegion) {
        let now = Instant::now();
        if self.pending.is_none_or(|(previous, _)| previous != region) {
            self.invalidate();
            self.pending = Some((region, now));
        }
        if self.reported != Some(region)
            && self.pending.is_some_and(|(_, changed)| {
                now.duration_since(changed) >= Duration::from_millis(300)
            })
        {
            self.reported = Some(region);
            send_event(OverlayEvent::RegionChanged(region));
        }
    }
}

fn frame_rect(window: Rect) -> Rect {
    let mut rect = window.shrink(4.0);
    rect.min.y += 20.0;
    rect
}

fn capture_region(rect: Rect) -> OverlayRegion {
    OverlayRegion {
        x: rect.left().ceil() as i32,
        y: rect.top().ceil() as i32,
        width: (rect.right().floor() - rect.left().ceil()).max(1.0) as u32,
        height: (rect.bottom().floor() - rect.top().ceil()).max(1.0) as u32,
    }
}

// The capture inset excludes the outline and handle. Only the frame accepts
// input; applications underneath remain clickable through the middle.
fn recognition_area(ui: &mut egui::Ui, regions: &mut Vec<(Rect, f32)>) -> (Rect, bool) {
    let rect = frame_rect(ui.max_rect());
    let mut moving = false;
    let painter = ui.painter();
    // Inward grey shadows stay within the native window's narrow input mask.
    // The capture inset below the caller's frame excludes all of these pixels.
    let shadow_rect = rect.shrink(1.0).translate(egui::vec2(0.0, 0.75));
    for (width, alpha) in [(4.0, 36), (2.5, 80)] {
        painter.rect_stroke(
            shadow_rect,
            0.0,
            Stroke::new(width, Color32::from_rgba_unmultiplied(96, 96, 96, alpha)),
            egui::StrokeKind::Inside,
        );
    }
    painter.rect_stroke(
        rect,
        0.0,
        Stroke::new(1.0, Color32::WHITE),
        egui::StrokeKind::Inside,
    );
    for (corner, direction) in [
        (rect.left_top(), egui::vec2(1.0, 1.0)),
        (rect.right_top(), egui::vec2(-1.0, 1.0)),
        (rect.left_bottom(), egui::vec2(1.0, -1.0)),
        (rect.right_bottom(), egui::vec2(-1.0, -1.0)),
    ] {
        let corner = corner + direction * 1.5;
        let points = [
            corner + egui::vec2(direction.x * 9.0, 0.0),
            corner,
            corner + egui::vec2(0.0, direction.y * 9.0),
        ];
        painter.add(egui::Shape::line(
            points.map(|point| point + direction * 1.0).to_vec(),
            Stroke::new(4.0, Color32::from_rgba_unmultiplied(96, 96, 96, 100)),
        ));
        painter.add(egui::Shape::line(
            points.to_vec(),
            Stroke::new(3.0, Color32::WHITE),
        ));
    }

    use egui::viewport::ResizeDirection::*;
    // Wider invisible handles keep the thin outline easy to grab. Register
    // corners last so diagonal resizing wins where handles overlap.
    for (edge, direction) in [
        (
            Rect::from_min_max(rect.left_top(), rect.right_top() + egui::vec2(0.0, GAP)),
            North,
        ),
        (
            Rect::from_min_max(
                rect.left_bottom() - egui::vec2(0.0, GAP),
                rect.right_bottom(),
            ),
            South,
        ),
        (
            Rect::from_min_max(rect.left_top(), rect.left_bottom() + egui::vec2(GAP, 0.0)),
            West,
        ),
        (
            Rect::from_min_max(rect.right_top() - egui::vec2(GAP, 0.0), rect.right_bottom()),
            East,
        ),
        (
            Rect::from_min_size(rect.left_top(), Vec2::splat(12.0)),
            NorthWest,
        ),
        (
            Rect::from_min_size(rect.right_top() - egui::vec2(12.0, 0.0), Vec2::splat(12.0)),
            NorthEast,
        ),
        (
            Rect::from_min_size(
                rect.left_bottom() - egui::vec2(0.0, 12.0),
                Vec2::splat(12.0),
            ),
            SouthWest,
        ),
        (
            Rect::from_min_size(rect.right_bottom() - Vec2::splat(12.0), Vec2::splat(12.0)),
            SouthEast,
        ),
    ] {
        moving |= resize(ui, edge, direction);
        regions.push((edge, 0.0));
    }
    let handle = Rect::from_center_size(
        egui::pos2(rect.center().x, rect.top() - 10.0),
        egui::vec2(56.0, 20.0),
    );
    let drag = ui
        .interact(
            handle,
            ui.make_persistent_id("drag_ocr"),
            egui::Sense::drag(),
        )
        .on_hover_cursor(egui::CursorIcon::Grab);
    ui.painter()
        .rect_filled(handle, 10, theme::content_backdrop(true));
    ui.painter().rect_stroke(
        handle,
        10,
        Stroke::new(1.0, theme::border().gamma_multiply(0.35)),
        egui::StrokeKind::Inside,
    );
    for x in [-8.0, -4.0, 0.0, 4.0, 8.0] {
        ui.painter().circle_filled(
            handle.center() + egui::vec2(x, 0.0),
            1.0,
            theme::text_weak(),
        );
    }
    regions.push((handle, 10.0));
    if drag.drag_started() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        moving = true;
    }
    (rect.shrink(12.0), moving)
}
