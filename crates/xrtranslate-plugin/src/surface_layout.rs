//! Lightweight content geometry, independent of how a plugin renders its page.
use egui::{Rect, Shape, TextureId, emath::TSTransform};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ContentBounds {
    pub occupied: Vec<Rect>,
    pub heading: Option<Rect>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_keeps_video_and_text_but_not_the_page_backdrop() {
        let ctx = egui::Context::default();
        let page = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let video = Rect::from_min_size(egui::pos2(20.0, 100.0), egui::vec2(360.0, 240.0));
        let mut output = ctx.run_ui(Default::default(), |ui| {
            let painter = ui.painter().with_clip_rect(page);
            painter.rect_filled(page, 0, egui::Color32::WHITE);
            painter.text(
                egui::pos2(20.0, 20.0),
                egui::Align2::LEFT_TOP,
                "Page title",
                egui::FontId::proportional(24.0),
                egui::Color32::BLACK,
            );
            painter.text(
                page.center(),
                egui::Align2::LEFT_TOP,
                "",
                egui::FontId::proportional(14.0),
                egui::Color32::BLACK,
            );
            painter.image(
                TextureId::User(1),
                video,
                Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        });
        let mut content = ContentBounds::default();
        for item in output.shapes {
            content.collect(
                &item.shape,
                item.clip_rect,
                page,
                TSTransform::IDENTITY,
                None,
            );
        }
        output.textures_delta.clear();
        assert!(content.heading.is_some());
        assert!(content.occupied.contains(&video));
        assert!(!content.occupied.contains(&page));
        assert!(content.occupied.iter().all(|rect| rect.is_finite()));
    }
}

impl ContentBounds {
    pub fn collect(
        &mut self,
        shape: &Shape,
        clip: Rect,
        page: Rect,
        transform: TSTransform,
        background: Option<TextureId>,
    ) {
        if let Shape::Vec(shapes) = shape {
            for shape in shapes {
                self.collect(shape, clip, page, transform, background);
            }
            return;
        }
        // Backdrops leave room for overlays; content images and videos do not.
        if matches!(shape, Shape::Mesh(mesh) if Some(mesh.texture_id) == background)
            || matches!(shape, Shape::Rect(rect) if rect.rect.area() > page.area() * 0.15)
        {
            return;
        }
        // Empty text can have infinite bounds. Clipping it first would turn
        // an invisible label into an obstacle covering the entire page.
        let rect = shape.visual_bounding_rect();
        if !rect.is_positive() || !rect.is_finite() {
            return;
        }
        let rect = transform.mul_rect(rect).intersect(clip);
        if !rect.is_positive() {
            return;
        }
        if let Shape::Text(text) = shape
            && rect.top() < page.top() + 100.0
            && text
                .galley
                .job
                .sections
                .iter()
                .any(|section| section.format.font_id.size >= 18.0)
            && self
                .heading
                .is_none_or(|current| rect.top() < current.top())
        {
            self.heading = Some(rect);
        }
        self.occupied.push(rect);
    }
}
