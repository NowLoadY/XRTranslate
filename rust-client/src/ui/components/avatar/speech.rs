//! Screen-space speech, independent of the model's pose and the page's behavior.
use crate::ui::theme;
use eframe::egui::{self, Color32, FontId, Painter, Pos2, Rect, Vec2};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Default)]
pub struct Speech {
    text: &'static str,
    end: usize,
    next_letter: f64,
    last_letter: Option<f64>,
    dismiss_at: f64,
    position: Option<Pos2>,
    direction: Option<Vec2>,
    size: Vec2,
    opacity: f32,
}

impl Speech {
    /// Reuse dialogue timing on another surface without its desktop position.
    pub(crate) fn on_surface(&self, previous: &Self) -> Self {
        Self {
            position: previous.position,
            direction: previous.direction,
            size: previous.size,
            opacity: previous.opacity,
            ..self.clone()
        }
    }

    pub fn say(&mut self, text: &'static str, now: f64) {
        self.text = text;
        self.end = 0;
        self.next_letter = now;
        self.last_letter = None;
        self.dismiss_at = f64::INFINITY;
        self.direction = None;
    }

    pub fn finished(&self, now: f64) -> bool {
        self.text.is_empty() || now >= self.dismiss_at
    }

    /// Use a caller-owned clock so dialogue pauses with its scene.
    pub fn advance(&mut self, now: f64) -> f32 {
        while self.end < self.text.len() && now >= self.next_letter {
            let letter = self.text[self.end..].graphemes(true).next().unwrap();
            self.end += letter.len();
            let delay = letter_delay(letter);
            self.last_letter =
                (delay < 0.15 && !letter.trim().is_empty()).then_some(self.next_letter);
            self.next_letter += delay;
            if self.end == self.text.len() {
                let reading_time =
                    (self.text.graphemes(true).count() as f64 * 0.035).clamp(2.2, 4.0);
                self.dismiss_at = self.next_letter + reading_time;
            }
        }
        self.last_letter.map_or(0.0, |at| {
            let age = ((now - at) / 0.12).clamp(0.0, 1.0) as f32;
            (age * std::f32::consts::PI).sin().max(0.0) * 0.65
        })
    }

    /// Paint without a widget or hit region; the bubble never steals page input.
    pub fn paint(
        &mut self,
        painter: &Painter,
        bounds: Rect,
        anchor: Pos2,
        radius: f32,
        now: f64,
        dt: f32,
    ) -> bool {
        self.paint_avoiding(painter, bounds, anchor, radius, now, dt, &[])
    }

    pub(crate) fn paint_avoiding(
        &mut self,
        painter: &Painter,
        bounds: Rect,
        anchor: Pos2,
        radius: f32,
        now: f64,
        dt: f32,
        occupied: &[Rect],
    ) -> bool {
        let visible = !self.finished(now);
        let target_opacity = if visible { 1.0 } else { 0.0 };
        self.opacity += (target_opacity - self.opacity) * (1.0 - (-dt / 0.12).exp());
        if self.opacity < 0.005 && !visible {
            self.position = None;
            self.direction = None;
            self.size = Vec2::ZERO;
            return false;
        }

        let width_limit = if bounds.height() < 72.0 { 340.0 } else { 260.0 };
        let max_width = (bounds.width() - radius * 3.0 - 44.0).clamp(80.0, width_limit);
        let galley = painter.layout(
            self.text[..self.end].to_owned(),
            FontId::proportional(14.0),
            theme::text_strong(),
            max_width,
        );
        let desired_size = (galley.size() + egui::vec2(28.0, 22.0))
            .max(egui::vec2(38.0, 40.0))
            .min(bounds.size());
        if self.size == Vec2::ZERO {
            self.size = egui::vec2(38.0, 40.0).min(bounds.size());
        }
        self.size += (desired_size - self.size) * (1.0 - (-dt / 0.09).exp());
        self.size = self.size.min(bounds.size());
        // Judge all four directions using the complete sentence, so typing does
        // not change the choice. Prefer the current side until another is clearer.
        let full_size = (painter
            .layout(
                self.text.to_owned(),
                FontId::proportional(14.0),
                theme::text_strong(),
                max_width,
            )
            .size()
            + egui::vec2(28.0, 22.0))
        .max(egui::vec2(38.0, 40.0))
        .min(bounds.size());
        let body = Rect::from_min_max(
            anchor - egui::vec2(radius * 1.3, radius * 1.7),
            anchor + egui::vec2(radius * 1.3, radius * 1.1),
        )
        .expand(4.0);
        let direction = bubble_direction(
            bounds,
            body,
            anchor,
            radius,
            full_size,
            occupied,
            self.direction,
        );
        if self.direction != Some(direction) {
            self.position = None;
        }
        self.direction = Some(direction);
        let target = bubble_center(anchor, radius, self.size, direction);
        let position = self.position.get_or_insert(target);
        *position += (target - *position) * (1.0 - (-dt / 0.22).exp());
        // The chosen side and final position use the same visible bounds.
        *position = bounds.shrink2(self.size * 0.5).clamp(*position);
        let rect = Rect::from_center_size(*position, self.size);
        let mut painter = painter.with_clip_rect(bounds);
        painter.multiply_opacity(self.opacity);
        let fill = Color32::from_rgba_unmultiplied(252, 252, 253, 245);
        painter.add(
            egui::epaint::Shadow {
                offset: [0, 3],
                blur: 14,
                spread: 0,
                color: Color32::from_black_alpha(16),
            }
            .as_shape(rect, egui::CornerRadius::same(16)),
        );
        if !rect.contains(anchor) {
            let toward = anchor - rect.center();
            let horizontal = toward.x.abs() / rect.width() > toward.y.abs() / rect.height();
            let normal = if horizontal {
                egui::vec2(toward.x.signum(), 0.0)
            } else {
                egui::vec2(0.0, toward.y.signum())
            };
            let edge =
                rect.center() + egui::vec2(normal.x * rect.width(), normal.y * rect.height()) * 0.5;
            let tangent = egui::vec2(-normal.y, normal.x) * 5.0;
            painter.add(egui::Shape::convex_polygon(
                vec![
                    edge - tangent,
                    edge + toward.normalized() * 10.0,
                    edge + tangent,
                ],
                fill,
                egui::Stroke::NONE,
            ));
        }
        painter.rect_filled(rect, 16.0, fill);
        painter
            .with_clip_rect(rect.shrink2(egui::vec2(12.0, 9.0)))
            .galley(
                rect.min + egui::vec2(14.0, 11.0),
                galley,
                theme::text_strong(),
            );
        visible || (self.opacity - target_opacity).abs() > 0.005
    }
}

fn bubble_center(anchor: Pos2, radius: f32, size: Vec2, direction: Vec2) -> Pos2 {
    anchor
        + egui::vec2(
            direction.x * (radius * 1.3 + 20.0 + size.x * 0.5),
            direction.y
                * (radius * if direction.y < 0.0 { 1.7 } else { 1.1 } + 20.0 + size.y * 0.5),
        )
}

fn bubble_direction(
    bounds: Rect,
    body: Rect,
    anchor: Pos2,
    radius: f32,
    size: Vec2,
    occupied: &[Rect],
    previous: Option<Vec2>,
) -> Vec2 {
    let score = |direction| {
        let target = Rect::from_center_size(bubble_center(anchor, radius, size, direction), size);
        let center = bounds.shrink2(size * 0.5).clamp(target.center());
        let rect = Rect::from_center_size(center, size);
        occupied
            .iter()
            .map(|obstacle| rect.intersect(*obstacle).area())
            .sum::<f32>()
            + rect.intersect(body).area() * 8.0
            + (target.area() - target.intersect(bounds).area()) * 4.0
            - if previous == Some(direction) {
                rect.area() * 0.1
            } else {
                0.0
            }
    };
    [
        egui::vec2(-1.0, 0.0),
        egui::vec2(1.0, 0.0),
        egui::vec2(0.0, -1.0),
        egui::vec2(0.0, 1.0),
    ]
    .into_iter()
    .min_by(|a, b| score(*a).total_cmp(&score(*b)))
    .unwrap()
}

fn letter_delay(letter: &str) -> f64 {
    match letter.chars().next().unwrap_or(' ') {
        '.' | '!' | '?' | '。' | '！' | '？' | '؟' | '।' | '॥' | '\n' => 0.46,
        '…' | '—' => 0.55,
        ',' | ';' | ':' | '，' | '、' | '；' | '：' | '،' | '؛' => 0.24,
        c if c.is_whitespace() => 0.025,
        c if c.is_ascii_punctuation()
            || matches!(c, '\u{2000}'..='\u{206f}' | '\u{3001}'..='\u{303f}'
                | '\u{ff01}'..='\u{ff0f}' | '\u{ff1a}'..='\u{ff20}'
                | '\u{ff3b}'..='\u{ff40}' | '\u{ff5b}'..='\u{ff65}') =>
        {
            0.16
        }
        // CJK syllables/ideographs carry more information per character.
        '\u{3040}'..='\u{9fff}' | '\u{ac00}'..='\u{d7af}' => 0.075,
        _ => 0.042,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn another_surface_keeps_dialogue_and_its_own_layout() {
        let mut desktop = Speech::default();
        desktop.say("Hi, I'm right here!", 0.0);
        desktop.advance(1.5);
        desktop.position = Some(egui::pos2(1000.0, 600.0));
        let previous = Speech {
            position: Some(egui::pos2(320.0, 240.0)),
            size: egui::vec2(180.0, 40.0),
            opacity: 0.8,
            ..Default::default()
        };
        let surface = desktop.on_surface(&previous);
        assert_eq!(surface.text, desktop.text);
        assert_eq!(surface.end, desktop.end);
        assert_eq!(surface.dismiss_at, desktop.dismiss_at);
        assert_eq!(surface.position, previous.position);
        assert_eq!(surface.size, previous.size);
        assert_eq!(surface.opacity, previous.opacity);
        assert_eq!(surface.finished(5.0), desktop.finished(5.0));
    }
}
