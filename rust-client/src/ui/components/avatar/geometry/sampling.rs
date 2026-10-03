//! Curvature-guided sampling keeps small sculpted details smooth without a denser whole mesh.
use glam::Vec3;

pub(in super::super) struct SurfaceSamples {
    columns: Vec<f32>,
    rows: Vec<Vec<f32>>,
}

fn weights(points: &[Vec3]) -> Vec<f32> {
    (1..points.len())
        .map(|i| {
            let j = i.min(points.len() - 2);
            (points[j - 1] - points[j] * 2.0 + points[j + 1])
                .length()
                .sqrt()
                + points[i].distance(points[i - 1])
        })
        .collect()
}

fn distribute(weights: &[f32], count: usize) -> Vec<f32> {
    let mut cumulative = vec![0.0; weights.len() + 1];
    for (i, weight) in weights.iter().enumerate() {
        cumulative[i + 1] = cumulative[i] + weight;
    }
    (0..=count)
        .map(|sample| {
            if sample == 0 || sample == count {
                return sample as f32 / count as f32;
            }
            let distance = cumulative[weights.len()] * sample as f32 / count as f32;
            let i = cumulative
                .partition_point(|d| *d < distance)
                .clamp(1, weights.len());
            let fraction = (distance - cumulative[i - 1])
                / (cumulative[i] - cumulative[i - 1]).max(f32::EPSILON);
            (i as f32 - 1.0 + fraction) / weights.len() as f32
        })
        .collect()
}

impl SurfaceSamples {
    pub fn new(columns: usize, rows: usize, surface: impl Fn(f32, f32) -> Vec3) -> Self {
        let mut density = vec![0.0_f32; columns * 8];
        for row in 1..rows * 2 {
            let points: Vec<_> = (0..=density.len())
                .map(|i| {
                    surface(
                        i as f32 / density.len() as f32,
                        row as f32 / (rows * 2) as f32,
                    )
                })
                .collect();
            for (density, weight) in density.iter_mut().zip(weights(&points)) {
                *density = density.max(weight);
            }
        }
        let columns = distribute(&density, columns);
        let rows = columns
            .iter()
            .map(|&u| {
                let points: Vec<_> = (0..=rows * 8)
                    .map(|i| surface(u, i as f32 / (rows * 8) as f32))
                    .collect();
                distribute(&weights(&points), rows)
            })
            .collect();
        Self { columns, rows }
    }

    pub fn at(&self, u: f32, v: f32) -> (f32, f32) {
        let column = u.rem_euclid(1.0) * (self.columns.len() - 1) as f32;
        let i = column as usize;
        let sample = |values: &[f32]| {
            let row = v.clamp(0.0, 1.0) * (values.len() - 1) as f32;
            let j = (row as usize).min(values.len() - 2);
            values[j] + (values[j + 1] - values[j]) * (row - j as f32)
        };
        let a = sample(&self.rows[i]);
        (
            self.columns[i] + (self.columns[i + 1] - self.columns[i]) * column.fract(),
            a + (sample(&self.rows[i + 1]) - a) * column.fract(),
        )
    }
}
