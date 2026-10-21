//! Shared, catalog-driven model comparison widgets for onboarding.

use eframe::egui::{self, Align2, Color32, FontId, RichText, Stroke, StrokeKind, Vec2};
use xrtranslate_assets::{
    ModelAssetId, ModelAssetManifest, ModelBenchmark, ModelCapability, manifest_for,
    manifests_for_capability,
};

use crate::{i18n, runtime_install::LocalModelAvailability};

const GIB: f64 = (1024 * 1024 * 1024) as f64;
const BLUE: Color32 = Color32::from_rgb(48, 113, 210);
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
            ModelCapability::Tts
            | ModelCapability::Ocr
            | ModelCapability::SourceSeparation
            | ModelCapability::NoiseSuppression => None,
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
) -> Option<String> {
    let (gpu, capacity) = match availability {
        LocalModelAvailability::Available {
            gpu, memory_bytes, ..
        }
        | LocalModelAvailability::InsufficientVram {
            gpu, memory_bytes, ..
        } => (gpu.as_str(), *memory_bytes),
        #[cfg(target_os = "android")]
        LocalModelAvailability::Cpu {
            available_memory_bytes,
        } => ("CPU · RAM", *available_memory_bytes),
        _ => return None,
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
    let mut selected_gpu_changed = None;
    ui.horizontal_wrapped(|ui| {
        let available_gpus = availability.available_gpus();
        if available_gpus.len() > 1 {
            egui::ComboBox::from_id_salt("vram_budget_gpu_select")
                .selected_text(RichText::new(gpu).size(11.5).strong())
                .show_ui(ui, |ui| {
                    for dev in available_gpus {
                        let text = format!("{} · {:.1} GiB", dev.name, dev.memory_bytes as f64 / GIB);
                        let is_selected = dev.name == gpu;
                        if ui.selectable_label(is_selected, text).clicked() && !is_selected {
                            selected_gpu_changed = Some(dev.name.clone());
                        }
                    }
                });
        } else {
            ui.label(RichText::new(gpu).size(11.5));
        }
        let bar_width = (ui.max_rect().width() * 0.65)
            .clamp(200.0, 480.0)
            .min(ui.max_rect().width());
        let (bar, _) = ui.allocate_exact_size(Vec2::new(bar_width, 18.0), egui::Sense::hover());
        let painter = ui.painter_at(bar);
        let radius = egui::CornerRadius::same(5);
        painter.rect_filled(bar, radius, Color32::from_rgb(221, 228, 238));
        // Selected packages occupy the track from left to right; the gray tail
        // remains visible up to the reported memory capacity.
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
                    i18n::tr(language, "Estimated model memory includes weights and runtime overhead; actual use varies with context and concurrent workloads."),
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
    });
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
    selected_gpu_changed
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
            i18n::tr(language, "Local model reference"),
            spec.display_metric
        ))
        .size(12.0)
        .strong()
        .color(INK),
    );
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(spec.benchmark).size(10.5).weak());
        ui.label(
            RichText::new(format!("↑ {}", i18n::tr(language, "Higher is better")))
                .size(10.5)
                .weak(),
        );
    });
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
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 154.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let plot = egui::Rect::from_min_max(
        rect.min + Vec2::new(32.0, 8.0),
        rect.max - Vec2::new(12.0, 32.0),
    );
    let dark = ui.visuals().dark_mode;
    let ink = ui.visuals().weak_text_color();
    let fill = if dark {
        Color32::from_rgb(35, 45, 54)
    } else if spec.source_metric == "WER" {
        Color32::from_rgb(236, 243, 255)
    } else {
        Color32::from_rgb(234, 247, 240)
    };
    let guide = if dark {
        Color32::from_rgb(77, 91, 104)
    } else {
        Color32::from_rgb(198, 211, 222)
    };
    painter.rect_filled(plot.expand(7.0), 14.0, fill);
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
        painter.extend(egui::Shape::dashed_line(
            &[egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
            Stroke::new(1.0, guide),
            3.0,
            5.0,
        ));
        painter.text(
            egui::pos2(plot.left() - 11.0, y),
            Align2::RIGHT_CENTER,
            format!("{:.0}", y_min + (y_max - y_min) * i as f32 / 2.0),
            FontId::proportional(10.0),
            ink,
        );
        let x = egui::lerp(plot.left()..=plot.right(), i as f32 / 2.0);
        painter.text(
            egui::pos2(x, plot.bottom() + 11.0),
            Align2::CENTER_TOP,
            format!("{:.0}", x_max * i as f32 / 2.0),
            FontId::proportional(10.0),
            ink,
        );
    }
    painter.text(
        egui::pos2(plot.center().x, rect.bottom()),
        Align2::CENTER_BOTTOM,
        i18n::tr(language, "Parameters (B)"),
        FontId::proportional(10.0),
        ink,
    );
    for (index, model) in models.iter().enumerate() {
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
            egui::Rect::from_center_size(point, Vec2::splat(20.0)),
            ui.make_persistent_id(("model_chart_point", model.id)),
            egui::Sense::hover(),
        );
        let active = selected == Some(model.id) || response.hovered();
        let color = MODEL_COLORS[index % MODEL_COLORS.len()];
        if active {
            painter.circle_filled(point, 11.0, color.gamma_multiply(0.16));
        }
        painter.circle_filled(point, if active { 6.0 } else { 4.5 }, color);
        if selected == Some(model.id) {
            painter.circle_stroke(point, 8.0, Stroke::new(1.5, color));
            painter.circle_filled(point, 2.0, Color32::WHITE);
        }
        response.on_hover_ui(|ui| {
            ui.set_max_width(300.0);
            ui.label(RichText::new(model.label).strong());
            ui.label(format!("{}: {percent:.2}", spec.display_metric));
            ui.label(format!(
                "{}: {:.2}",
                i18n::tr(language, "Parameters (B)"),
                parameters as f32 / 1000.0
            ));
            if let Some(result) = spec.benchmark(model) {
                ui.label(
                    RichText::new(i18n::tr(language, result.note))
                        .small()
                        .weak(),
                );
                ui.hyperlink_to(
                    i18n::tr(language, "View benchmark source"),
                    result.source_url,
                );
            }
        });
    }
}
