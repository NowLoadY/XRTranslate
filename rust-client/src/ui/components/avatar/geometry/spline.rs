//! Shape-preserving interpolation of landmarks, with automatically fitted tangents.
pub(in super::super) struct Spline<const D: usize, const N: usize> {
    knots: [f32; N],
    values: [[f32; D]; N],
    slopes: [[f32; D]; N],
}

impl<const D: usize, const N: usize> Spline<D, N> {
    pub fn new(knots: [f32; N], values: [[f32; D]; N]) -> Self {
        let slopes = std::array::from_fn(|i| {
            std::array::from_fn(|axis| {
                if i == 0 || i == N - 1 {
                    let j = if i == 0 { 0 } else { N - 2 };
                    return (values[j + 1][axis] - values[j][axis]) / (knots[j + 1] - knots[j]);
                }
                let (left, right) = (knots[i] - knots[i - 1], knots[i + 1] - knots[i]);
                let a = (values[i][axis] - values[i - 1][axis]) / left;
                let b = (values[i + 1][axis] - values[i][axis]) / right;
                if a * b <= 0.0 {
                    return 0.0;
                }
                let (wa, wb) = (2.0 * right + left, right + 2.0 * left);
                (wa + wb) / (wa / a + wb / b)
            })
        });
        Self {
            knots,
            values,
            slopes,
        }
    }

    pub fn flat_ends(mut self, axes: &[usize]) -> Self {
        for &axis in axes {
            self.slopes[0][axis] = 0.0;
            self.slopes[N - 1][axis] = 0.0;
        }
        self
    }

    pub fn sample(&self, t: f32) -> [f32; D] {
        let t = t.clamp(self.knots[0], self.knots[N - 1]);
        let i = self
            .knots
            .partition_point(|k| *k <= t)
            .saturating_sub(1)
            .min(N - 2);
        let h = self.knots[i + 1] - self.knots[i];
        let u = (t - self.knots[i]) / h;
        std::array::from_fn(|axis| {
            self.values[i][axis] * (2.0 * u.powi(3) - 3.0 * u * u + 1.0)
                + self.slopes[i][axis] * (h * (u.powi(3) - 2.0 * u * u + u))
                + self.values[i + 1][axis] * (-2.0 * u.powi(3) + 3.0 * u * u)
                + self.slopes[i + 1][axis] * (h * (u.powi(3) - u * u))
        })
    }
}
