//! Shape-preserving lofts from measured sections; no authored Bézier handles.
use super::Spline;
use glam::Vec4;
use std::f32::consts::PI;

pub(super) struct Profile<const N: usize> {
    sections: [Vec4; N], // Half-width, front, back, height, ordered from top to bottom.
    curve: Spline<3, N>,
}

impl<const N: usize> Profile<N> {
    pub fn new(sections: [Vec4; N]) -> Self {
        let height = sections[0].w - sections[N - 1].w;
        // Cosine spacing gives a rounded pole even when its width is zero.
        let knots = sections.map(|s| {
            ((2.0 * (s.w - sections[N - 1].w) / height - 1.0).clamp(-1.0, 1.0)).acos() / PI
        });
        let values = sections.map(|s| [s.x, (s.y + s.z) * 0.5, (s.y - s.z) * 0.5]);
        Self {
            sections,
            curve: Spline::new(knots, values).flat_ends(&[1]),
        }
    }

    pub fn sample(&self, t: f32) -> Vec4 {
        let t = t.clamp(0.0, 1.0);
        let [width, center, depth] = self.curve.sample(t);
        let height = self.sections[N - 1].w
            + (self.sections[0].w - self.sections[N - 1].w) * (1.0 + (PI * t).cos()) * 0.5;
        Vec4::new(width, center + depth, center - depth, height)
    }

    pub fn at_height(&self, y: f32) -> Vec4 {
        self.sample(
            (2.0 * self.height_fraction(y) - 1.0)
                .clamp(-1.0, 1.0)
                .acos()
                / PI,
        )
    }

    pub fn height_fraction(&self, y: f32) -> f32 {
        (y - self.sections[N - 1].w) / (self.sections[0].w - self.sections[N - 1].w)
    }

    pub fn sections(&self) -> &[Vec4; N] {
        &self.sections
    }
}
