//! Read-only scrolling text with transparent edges, including on translucent surfaces.
use std::sync::Arc;

use eframe::egui::{self, Color32, FontId, Sense};

pub fn show(ui: &mut egui::Ui, text: &str, font: FontId, color: Color32) {
    ui.scope(|ui| {
        // Native scroll fades paint an opaque backdrop, which looks like a highlight on glass.
        ui.spacing_mut().scroll.fade.strength = 0.0;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(ui.available_height())
            .show(ui, |ui| {
                let galley = ui
                    .painter()
                    .layout(text.into(), font, color, ui.available_width());
                let (rect, _) = ui.allocate_exact_size(galley.size(), Sense::hover());
                let clip = ui.clip_rect();
                let fade_top = rect.top() < clip.top() - 0.5;
                let fade_bottom = rect.bottom() > clip.bottom() + 0.5;
                let mut visible = (*galley).clone();
                visible
                    .rows
                    .retain(|row| clip.intersects(row.rect().translate(rect.min.to_vec2())));
                visible.num_vertices = visible
                    .rows
                    .iter()
                    .map(|row| row.visuals.mesh.vertices.len())
                    .sum();
                visible.num_indices = visible
                    .rows
                    .iter()
                    .map(|row| row.visuals.mesh.indices.len())
                    .sum();
                // Layout and glyphs stay cached; only the rows touching an edge need a copy.
                for row in &mut visible.rows {
                    let origin = rect.top() + row.pos.y;
                    let bounds = row.visuals.mesh_bounds.translate(egui::vec2(0.0, origin));
                    if (fade_top && bounds.top() < clip.top() + 24.0)
                        || (fade_bottom && bounds.bottom() > clip.bottom() - 24.0)
                    {
                        for vertex in &mut Arc::make_mut(&mut row.row).visuals.mesh.vertices {
                            let y = origin + vertex.pos.y;
                            let top = if fade_top {
                                (y - clip.top()) / 24.0
                            } else {
                                1.0
                            };
                            let bottom = if fade_bottom {
                                (clip.bottom() - y) / 24.0
                            } else {
                                1.0
                            };
                            vertex.color =
                                vertex.color.gamma_multiply(top.min(bottom).clamp(0.0, 1.0));
                        }
                    }
                }
                ui.painter().galley(rect.min, Arc::new(visible), color);
            });
    });
}
