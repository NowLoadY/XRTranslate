//! A small perspective viewport for data skeletons. No mesh renderer or game
//! engine is needed; depth-sorted bones remain independent of model topology.
use eframe::egui::{self, Color32, Pos2, Sense, Stroke, Vec2};
use glam::Vec3;
use xrtranslate_plugin::Scene as SceneData;

pub(super) struct Scene {
    yaw: f32,
    pitch: f32,
    distance: f32,
}
impl Default for Scene {
    fn default() -> Self {
        Self {
            yaw: 0.35,
            pitch: 0.15,
            distance: 3.8,
        }
    }
}
impl Scene {
    pub(super) fn show(&mut self, ui: &mut egui::Ui, data: &SceneData) {
        let height = (ui.available_height() - 170.0).clamp(220.0, 650.0);
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::drag());
        if response.dragged() {
            let delta = ui.input(|i| i.pointer.delta());
            self.yaw -= delta.x * 0.008;
            self.pitch = (self.pitch + delta.y * 0.006).clamp(-0.25, 1.25);
        }
        if response.hovered() {
            self.distance = (self.distance
                * (-ui.input(|i| i.smooth_scroll_delta.y) * 0.003).exp())
            .clamp(1.5, 12.0);
        }
        if response.double_clicked() {
            *self = Self::default();
        }
        let dark = ui.visuals().dark_mode;
        let painter = ui.painter().with_clip_rect(rect);
        painter.rect_filled(
            rect,
            20.0,
            if dark {
                Color32::from_rgb(23, 27, 34)
            } else {
                Color32::from_rgb(245, 248, 252)
            },
        );
        let root = Vec3::from_array(data.target);
        let target = Vec3::new(root.x, 0.95, root.z);
        let direction = Vec3::new(
            self.yaw.sin() * self.pitch.cos(),
            self.pitch.sin(),
            self.yaw.cos() * self.pitch.cos(),
        );
        let eye = target + direction * self.distance;
        let forward = (target - eye).normalize();
        let right = forward.cross(Vec3::Y).normalize();
        let up = right.cross(forward);
        let project = |world: Vec3| -> Option<(Pos2, f32)> {
            let v = world - eye;
            let depth = v.dot(forward);
            (depth > 0.05).then(|| {
                let scale = rect.height().min(rect.width()) * 0.9 / depth;
                (
                    Pos2::new(
                        rect.center().x + v.dot(right) * scale,
                        rect.center().y - v.dot(up) * scale,
                    ),
                    depth,
                )
            })
        };
        let grid = if dark {
            Color32::from_rgb(46, 54, 65)
        } else {
            Color32::from_rgb(214, 223, 234)
        };
        let cx = root.x.floor();
        let cz = root.z.floor();
        for i in -6..=6 {
            let i = i as f32;
            for (a, b) in [
                (
                    Vec3::new(cx + i, 0.0, cz - 6.0),
                    Vec3::new(cx + i, 0.0, cz + 6.0),
                ),
                (
                    Vec3::new(cx - 6.0, 0.0, cz + i),
                    Vec3::new(cx + 6.0, 0.0, cz + i),
                ),
            ] {
                if let (Some((a, _)), Some((b, _))) = (project(a), project(b)) {
                    painter.line_segment([a, b], Stroke::new(1.0, grid));
                }
            }
        }
        let points: Vec<_> = data
            .points
            .iter()
            .map(|p| project(Vec3::from_array(*p)))
            .collect();
        let mut bones = Vec::new();
        for &[parent, child] in &data.lines {
            if let (Some(Some(a)), Some(Some(b))) = (points.get(parent), points.get(child)) {
                bones.push((a.0, b.0, (a.1 + b.1) * 0.5));
            }
        }
        bones.sort_by(|a, b| b.2.total_cmp(&a.2));
        let blue = Color32::from_rgb(47, 108, 235);
        for (a, b, depth) in bones {
            let width = (12.0 / depth).clamp(2.0, 5.0);
            painter.line_segment([a, b], Stroke::new(width, blue));
        }
        for (point, depth) in points.into_iter().flatten() {
            let radius = (12.0 / depth).clamp(2.5, 5.5);
            if rect.contains(point) {
                painter.circle_filled(
                    point,
                    radius,
                    if dark {
                        Color32::from_rgb(210, 225, 255)
                    } else {
                        Color32::WHITE
                    },
                );
                painter.circle_stroke(point, radius, Stroke::new(1.5, blue));
            }
        }
    }
}
