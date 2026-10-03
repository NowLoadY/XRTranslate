//! Curvature-guided sampling keeps small sculpted details smooth without a denser whole mesh.
use glam::Vec3;

pub(in super::super) struct SurfaceSamples(Vec<Vec<f32>>);

impl SurfaceSamples {
    pub fn new(columns: usize, rows: usize, surface: impl Fn(f32, f32) -> Vec3) -> Self {
        let probes = rows * 8;
        Self(
            (0..=columns)
                .map(|column| {
                    let u = column as f32 / columns as f32;
                    let points: Vec<_> = (0..=probes)
                        .map(|i| surface(u, i as f32 / probes as f32))
                        .collect();
                    let mut cumulative = vec![0.0; probes + 1];
                    for i in 1..=probes {
                        let j = i.min(probes - 1);
                        let curvature = (points[j - 1] - points[j] * 2.0 + points[j + 1])
                            .length()
                            .sqrt();
                        // Arc length also allocates samples to straight portions.
                        cumulative[i] =
                            cumulative[i - 1] + curvature + points[i].distance(points[i - 1]);
                    }
                    (0..=rows)
                        .map(|row| {
                            if row == 0 || row == rows {
                                return row as f32 / rows as f32;
                            }
                            let distance = cumulative[probes] * row as f32 / rows as f32;
                            let i = cumulative
                                .partition_point(|d| *d < distance)
                                .clamp(1, probes);
                            let fraction = (distance - cumulative[i - 1])
                                / (cumulative[i] - cumulative[i - 1]).max(f32::EPSILON);
                            (i as f32 - 1.0 + fraction) / probes as f32
                        })
                        .collect()
                })
                .collect(),
        )
    }

    pub fn at(&self, u: f32, v: f32) -> f32 {
        let column = u.rem_euclid(1.0) * (self.0.len() - 1) as f32;
        let i = column as usize;
        let sample = |values: &[f32]| {
            let row = v.clamp(0.0, 1.0) * (values.len() - 1) as f32;
            let j = (row as usize).min(values.len() - 2);
            values[j] + (values[j + 1] - values[j]) * (row - j as f32)
        };
        let a = sample(&self.0[i]);
        a + (sample(&self.0[i + 1]) - a) * column.fract()
    }
}
