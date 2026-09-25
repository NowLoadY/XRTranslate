//! Shared, catalog-driven model comparison widgets for onboarding.

use eframe::egui::{self, Align2, Color32, FontId, RichText, Stroke, StrokeKind, Vec2};
use xrtranslate_assets::{
    ModelAssetId, ModelAssetManifest, ModelBenchmark, ModelCapability, manifest_for,
    manifests_for_capability,
};

use crate::{i18n, runtime_install::LocalModelAvailability};

const GIB: f64 = (1024 * 1024 * 1024) as f64;
const BLUE: Color32 = Color32::from_rgb(48, 113, 210);
const GRID: Color32 = Color32::from_rgb(225, 231, 239);
const INK: Color32 = Color32::from_rgb(55, 65, 81);
const MODEL_COLORS: [Color32; 6] = [
    BLUE,
    Color32::from_rgb(13, 142, 108),
    Color32::from_rgb(137, 91, 183),
    Color32::from_rgb(196, 112, 41),
    Color32::from_rgb(178, 72, 108),
    Color32::from_rgb(28, 132, 159),
];

#[derive(Clone, Copy)]
struct ChartSpec {
    benchmark: &'static str,
    source_metric: &'static str,
    display_metric: &'static str,
    score: fn(f32) -> f32,
}

impl ChartSpec {
    fn for_capability(capability: ModelCapability) -> Option<Self> {
        match capability {
            ModelCapability::Asr => Some(Self {
                benchmark: "AMI",
                source_metric: "WER",
                display_metric: "100 − WER (%)",
                score: |value| 100.0 - value,
            }),
            ModelCapability::Translation => Some(Self {
                benchmark: "FLORES-200 XX↔XX",
                source_metric: "XCOMET-XXL",
                display_metric: "XCOMET-XXL",
                score: |value| value,
            }),
            ModelCapability::Tts => None,
        }
    }

    fn benchmark(self, model: &ModelAssetManifest) -> Option<ModelBenchmark> {
        model.benchmark.filter(|result| {
            result.benchmark == self.benchmark && result.metric == self.source_metric
        })
    }

    fn value(self, model: &ModelAssetManifest) -> Option<f32> {
        self.benchmark(model)
            .map(|result| (self.score)(result.score_centipercent as f32 / 100.0))
    }
}

pub(super) fn vram_budget(
    ui: &mut egui::Ui,
    language: i18n::UiLanguage,
    availability: &LocalModelAvailability,
    selected_assets: &[ModelAssetId],
) {
    let (gpu, capacity) = match availability {
        LocalModelAvailability::Available { gpu, memory_bytes }
        | LocalModelAvailability::InsufficientVram {
            gpu, memory_bytes, ..
        } => (gpu, *memory_bytes),
        _ => return,
    };
    let mut models = Vec::new();
    for id in selected_assets {
        if !models
            .iter()
            .any(|model: &&ModelAssetManifest| model.id == *id)
        {
            models.push(manifest_for(*id));
        }
    }
    let estimated: u64 = models.iter().map(|model| model.estimated_vram_bytes).sum();
    let over = estimated > capacity;
    ui.horizontal(|ui| {
        ui.label(RichText::new(gpu).size(11.5));
        let bar_width = (ui.available_width() * 0.65).clamp(200.0, 480.0);
        let (bar, _) = ui.allocate_exact_size(Vec2::new(bar_width, 18.0), egui::Sense::hover());
        let painter = ui.painter_at(bar);
        let radius = egui::CornerRadius::same(5);
        painter.rect_filled(bar, radius, Color32::from_rgb(221, 228, 238));
        // Selected packages occupy the track from left to right; the gray tail
        // remains visible up to the GPU's full reported capacity.
        let mut left = bar.left();
        for (index, model) in models.iter().enumerate() {
            if capacity == 0 || left >= bar.right() {
                break;
            }
            let width = bar.width() * model.estimated_vram_bytes as f32 / capacity as f32;
            let segment = egui::Rect::from_min_max(
                egui::pos2(left, bar.top()),
                egui::pos2((left + width).min(bar.right()), bar.bottom()),
            );
            painter.rect_filled(segment, 0.0, MODEL_COLORS[index % MODEL_COLORS.len()]);
            if segment.width() >= 18.0 {
                painter.text(
                    segment.center(),
                    Align2::CENTER_CENTER,
                    compact_gib(model.estimated_vram_bytes),
                    FontId::proportional(10.0),
                    Color32::WHITE,
                );
            }
            ui.interact(
                segment,
                ui.make_persistent_id(("vram_segment", model.id)),
                egui::Sense::hover(),
            )
            .on_hover_text(format!(
                    "{} · {:.2} GiB\n{}",
                    model.label,
                    model.estimated_vram_bytes as f64 / GIB,
                    i18n::tr(language, "Estimated model VRAM includes weights and runtime overhead; actual use varies with context and concurrent workloads."),
                ));
            left = segment.right();
        }
        painter.rect_stroke(
            bar,
            radius,
            Stroke::new(
                1.0,
                if over {
                    Color32::from_rgb(185, 54, 48)
                } else {
                    Color32::from_rgb(100, 116, 139)
                },
            ),
            StrokeKind::Inside,
        );
        ui.label(
            RichText::new(format!(
                "{} {:.2} / {:.1} GiB",
                i18n::tr(language, "Selected models ≈"),
                estimated as f64 / GIB,
                capacity as f64 / GIB
            ))
            .size(11.5)
            .color(if over {
                Color32::from_rgb(185, 54, 48)
            } else {
                INK
            }),
        );
    });
}

fn compact_gib(bytes: u64) -> String {
    let value = format!("{:.2}", bytes as f64 / GIB);
    value.trim_end_matches('0').trim_end_matches('.').to_owned()
}

pub(super) fn model_chart(
    ui: &mut egui::Ui,
    language: i18n::UiLanguage,
    capability: ModelCapability,
    selected: Option<ModelAssetId>,
) {
    let models: Vec<_> = manifests_for_capability(capability).collect();
    if models.is_empty() {
        return;
    }
    let Some(spec) = ChartSpec::for_capability(capability) else {
        return;
    };
    ui.label(
        RichText::new(format!(
            "{} · {}",
            i18n::tr(language, "Official model results"),
            spec.display_metric
        ))
        .size(12.0)
        .strong()
        .color(INK),
    );
    ui.label(
        RichText::new(spec.benchmark)
            .size(10.5)
            .color(Color32::from_gray(105)),
    );
    ui.add_space(4.0);
    draw_plot(ui, language, &models, selected, spec);
}

fn draw_plot(
    ui: &mut egui::Ui,
    language: i18n::UiLanguage,
    models: &[&ModelAssetManifest],
    selected: Option<ModelAssetId>,
    spec: ChartSpec,
) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 145.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let plot = egui::Rect::from_min_max(
        rect.min + Vec2::new(35.0, 10.0),
        rect.max - Vec2::new(10.0, 28.0),
    );
    let max_parameters = models
        .iter()
        .filter(|model| spec.value(model).is_some())
        .filter_map(|m| m.parameters_millions)
        .max()
        .unwrap_or(1000) as f32
        / 1000.0;
    let x_max = (max_parameters / 2.0).ceil().max(1.0) * 2.0;
    let scores: Vec<f32> = models
        .iter()
        .filter_map(|model| spec.value(model))
        .collect();
    let min_score = scores.iter().copied().reduce(f32::min).unwrap_or(70.0);
    let max_score = scores.iter().copied().reduce(f32::max).unwrap_or(90.0);
    let y_min = ((min_score - 3.0) / 5.0).floor() * 5.0;
    let y_max = ((max_score + 3.0) / 5.0).ceil() * 5.0;
    for i in 0..=2 {
        let y = egui::lerp(plot.bottom()..=plot.top(), i as f32 / 2.0);
        painter.line_segment(
            [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
            Stroke::new(1.0, GRID),
        );
        painter.text(
            egui::pos2(plot.left() - 5.0, y),
            Align2::RIGHT_CENTER,
            format!("{:.0}", y_min + (y_max - y_min) * i as f32 / 2.0),
            FontId::proportional(10.0),
            INK,
        );
        let x = egui::lerp(plot.left()..=plot.right(), i as f32 / 2.0);
        painter.line_segment(
            [egui::pos2(x, plot.top()), egui::pos2(x, plot.bottom())],
            Stroke::new(1.0, GRID),
        );
        painter.text(
            egui::pos2(x, plot.bottom() + 4.0),
            Align2::CENTER_TOP,
            format!("{:.0}", x_max * i as f32 / 2.0),
            FontId::proportional(10.0),
            INK,
        );
    }
    painter.text(
        egui::pos2(plot.center().x, rect.bottom() - 1.0),
        Align2::CENTER_BOTTOM,
        i18n::tr(language, "Parameters (B)"),
        FontId::proportional(10.0),
        INK,
    );
    for model in models {
        let (Some(parameters), Some(percent)) = (model.parameters_millions, spec.value(model))
        else {
            continue;
        };
        let x = egui::lerp(
            plot.left()..=plot.right(),
            (parameters as f32 / 1000.0 / x_max).clamp(0.0, 1.0),
        );
        let y = egui::lerp(
            plot.bottom()..=plot.top(),
            ((percent - y_min) / (y_max - y_min)).clamp(0.0, 1.0),
        );
        let point = egui::pos2(x, y);
        let response = ui.interact(
            egui::Rect::from_center_size(point, Vec2::splat(18.0)),
            ui.make_persistent_id(("model_chart_point", model.id)),
            egui::Sense::hover(),
        );
        let selected_point = selected == Some(model.id);
        let color = if selected_point {
            BLUE
        } else {
            Color32::from_rgb(134, 154, 178)
        };
        painter.circle_filled(
            point,
            if selected_point {
                6.5
            } else if response.hovered() {
                5.0
            } else {
                4.0
            },
            color,
        );
        if selected_point {
            painter.circle_stroke(point, 7.5, Stroke::new(2.0, Color32::from_rgb(23, 69, 145)));
        }
        if response.hovered() {
            egui::Tooltip::for_widget(&response)
                .at_pointer()
                .show(|ui| {
                    ui.label(model.label);
                });
        }
    }
    painter.rect_stroke(plot, 0.0, Stroke::new(1.0, GRID), StrokeKind::Inside);
}
