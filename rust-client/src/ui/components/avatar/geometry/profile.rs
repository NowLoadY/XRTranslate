//! Shape-preserving lofts from measured sections; no authored Bézier handles.
use glam::{Vec3, Vec4};
use std::f32::consts::PI;

pub(super) struct Profile<const N: usize> {
    sections: [Vec4; N], // Half-width, front, back, height, ordered from top to bottom.
    knots: [f32; N],
    values: [Vec3; N],
    slopes: [Vec3; N],
}

impl<const N: usize> Profile<N> {
    pub fn new(sections: [Vec4; N]) -> Self {
        let height = sections[0].w - sections[N - 1].w;
        // Cosine spacing gives a rounded pole even when its width is zero.
        let knots = sections.map(|s| {
            ((2.0 * (s.w - sections[N - 1].w) / height - 1.0).clamp(-1.0, 1.0)).acos() / PI
        });
        let values = sections.map(|s| Vec3::new(s.x, (s.y + s.z) * 0.5, (s.y - s.z) * 0.5));
        let slopes = std::array::from_fn(|i| {
            if i == 0 || i == N - 1 {
                let j = if i == 0 { 0 } else { N - 2 };
                let mut tangent = (values[j + 1] - values[j]) / (knots[j + 1] - knots[j]);
                tangent.y = 0.0; // A pole opens around its center, without a tilted seam.
                return tangent;
            }
            let (left, right) = (knots[i] - knots[i - 1], knots[i + 1] - knots[i]);
            let a = (values[i] - values[i - 1]) / left;
            let b = (values[i + 1] - values[i]) / right;
            // Weighted harmonic tangents keep a cheek or chin from overshooting its anchors.
            let (wa, wb) = (2.0 * right + left, right + 2.0 * left);
            Vec3::from_array(std::array::from_fn(|axis| {
                if a[axis] * b[axis] <= 0.0 {
                    0.0
                } else {
                    (wa + wb) / (wa / a[axis] + wb / b[axis])
                }
            }))
        });
        Self {
            sections,
            knots,
            values,
            slopes,
        }
    }

    pub fn sample(&self, t: f32) -> Vec4 {
        let t = t.clamp(0.0, 1.0);
        let i = self
            .knots
            .partition_point(|k| *k <= t)
            .saturating_sub(1)
            .min(N - 2);
        let h = self.knots[i + 1] - self.knots[i];
        let u = (t - self.knots[i]) / h;
        let p = self.values[i] * (2.0 * u.powi(3) - 3.0 * u * u + 1.0)
            + self.slopes[i] * (h * (u.powi(3) - 2.0 * u * u + u))
            + self.values[i + 1] * (-2.0 * u.powi(3) + 3.0 * u * u)
            + self.slopes[i + 1] * (h * (u.powi(3) - u * u));
        let height = self.sections[N - 1].w
            + (self.sections[0].w - self.sections[N - 1].w) * (1.0 + (PI * t).cos()) * 0.5;
        Vec4::new(p.x, p.y + p.z, p.y - p.z, height)
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
