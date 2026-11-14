use crate::{
    i18n::tr,
    ui::{components, theme},
};
use eframe::egui::{self, Color32};
use parking_lot::Mutex;
use std::{
    ops::Range,
    sync::Arc,
    time::{Duration, Instant},
};
use xrtranslate_prompt::diagnostics::{ModelRequestRecord, PromptPartKind};

type Pending = Arc<Mutex<Option<crate::diagnostics::RequestHistory>>>;

#[derive(Default)]
struct State {
    server: String,
    records: Vec<Arc<ModelRequestRecord>>,
    pending: Option<Pending>,
    checked: Option<Instant>,
    connected: bool,
    selected: Option<u64>,
    details: bool,
    error: Option<String>,
    preview: Option<Preview>,
}

struct Preview {
    id: u64,
    started_ms: u64,
    dark: bool,
    rows: Vec<egui::text::LayoutJob>,
    measurements: crate::ui::virtual_list::Measurements<usize>,
}

impl State {
    fn refresh(&mut self, ctx: &egui::Context, clear: bool) {
        let result = Arc::new(Mutex::new(None));
        self.pending = Some(result.clone());
        crate::diagnostics::fetch_requests(
            self.server.clone(),
            clear,
            ctx.clone(),
            move |records| {
                *result.lock() = Some(records);
            },
        );
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let result = self
            .pending
            .as_ref()
            .and_then(|pending| pending.lock().take());
        if let Some(result) = result {
            self.pending = None;
            self.checked = Some(Instant::now());
            match result {
                Ok(Some(records)) => {
                    self.connected = true;
                    self.records = records.into_iter().map(Arc::new).collect();
                    if !self
                        .records
                        .iter()
                        .any(|record| Some(record.id) == self.selected)
                    {
                        self.selected = self.records.last().map(|record| record.id);
                        self.preview = None;
                    }
                    self.error = None;
                }
                Ok(None) => {
                    self.connected = false;
                    self.error = None;
                }
                Err(error) => self.error = Some(error),
            }
        }
        if self.pending.is_none()
            && self
                .checked
                .is_none_or(|at| at.elapsed() > Duration::from_secs(2))
        {
            self.refresh(ctx, false);
        }
        ctx.request_repaint_after(Duration::from_millis(500));
    }
}

pub(super) fn render(app: &mut crate::XRTranslateApp, ui: &mut egui::Ui) {
    let state = ui.ctx().data_mut(|data| {
        data.get_temp_mut_or_default::<Arc<Mutex<State>>>(egui::Id::unique("developer_requests"))
            .clone()
    });
    let mut state = state.lock();
    if state.server != app.server_url {
        *state = State {
            server: app.server_url.clone(),
            ..Default::default()
        };
    }
    state.poll(ui.ctx());
    render_history(ui, &mut state, app.ui_language);
}

fn render_history(ui: &mut egui::Ui, state: &mut State, language: crate::i18n::UiLanguage) {
    ui.horizontal(|ui| {
        ui.heading(tr(language, "Model requests"));
        ui.weak(format!("{} / 64", state.records.len()));
        if state.pending.is_some() && state.checked.is_none() {
            ui.spinner();
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if components::secondary_button_enabled(
                ui,
                tr(language, "Clear"),
                state.connected && state.pending.is_none() && !state.records.is_empty(),
            )
            .clicked()
            {
                state.refresh(ui.ctx(), true);
            }
        });
    });
    if let Some(error) = &state.error {
        ui.colored_label(theme::danger(), tr(language, "Request history unavailable"))
            .on_hover_text(error);
    }
    ui.add_space(8.0);
    if state.records.is_empty() {
        if state.error.is_none() && (state.pending.is_none() || state.checked.is_some()) {
            ui.weak(tr(language, "No requests yet"));
        }
        return;
    }
    let height = ui.available_height().max(320.0);
    if ui.available_width() < 560.0 {
        request_list(ui, state, 135.0, language);
        ui.separator();
        request_detail(ui, state, (height - 150.0).max(200.0), language);
    } else {
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(195.0, height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    request_list(ui, state, height, language);
                },
            );
            ui.separator();
            ui.vertical(|ui| {
                request_detail(ui, state, height, language);
            });
        });
    }
}

fn request_list(
    ui: &mut egui::Ui,
    state: &mut State,
    height: f32,
    language: crate::i18n::UiLanguage,
) {
    egui::ScrollArea::vertical()
        .id_salt("model_request_list")
        .max_height(height)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for record in state.records.iter().rev() {
                let time = record.started_ms / 1000 % 86400;
                let label = format!(
                    "#{} · {:02}:{:02}:{:02} UTC\n{}\n{}",
                    record.id,
                    time / 3600,
                    time / 60 % 60,
                    time % 60,
                    record.model,
                    crate::i18n::tr_dynamic(language, &record.operation)
                );
                let color = if record.error.is_some() {
                    theme::danger()
                } else {
                    theme::text_normal()
                };
                let mut button = egui::Button::selectable(
                    state.selected == Some(record.id),
                    egui::RichText::new(label).size(12.0).color(color),
                );
                if record.error.is_some() {
                    button = button.fill(Color32::from_rgba_unmultiplied(
                        200,
                        55,
                        55,
                        if ui.visuals().dark_mode { 65 } else { 30 },
                    ));
                    if state.selected == Some(record.id) {
                        button = button.stroke(egui::Stroke::new(1.5, theme::danger()));
                    }
                }
                if ui.add_sized([ui.available_width(), 65.0], button).clicked() {
                    state.selected = Some(record.id);
                    state.preview = None;
                }
            }
        });
}

fn request_detail(
    ui: &mut egui::Ui,
    state: &mut State,
    height: f32,
    language: crate::i18n::UiLanguage,
) {
    let bottom = ui.cursor().top() + height;
    let Some(record) = state
        .records
        .iter()
        .find(|record| Some(record.id) == state.selected)
        .cloned()
    else {
        return;
    };
    ui.horizontal_wrapped(|ui| {
        ui.strong(&record.model);
        if let Some(ms) = record.elapsed_ms {
            ui.weak(format!("{ms} ms"));
        } else {
            ui.spinner();
        }
        if components::copy_button(ui, tr(language, "Copy prompt")).clicked() {
            ui.ctx().copy_text(record.prompt_text());
        }
        if components::secondary_button(ui, tr(language, "Copy details")).clicked() {
            ui.ctx().copy_text(record.report());
        }
    });
    if let Some(error) = &record.error {
        ui.colored_label(theme::danger(), error);
    }
    ui.horizontal(|ui| {
        components::selectable_value(ui, &mut state.details, false, "Prompt");
        components::selectable_value(ui, &mut state.details, true, tr(language, "Details"));
    });
    if state.details {
        egui::ScrollArea::vertical()
            .id_salt(("model_request_details", record.id))
            .max_height((bottom - ui.cursor().top()).max(100.0))
            .show(ui, |ui| {
                ui.add(
                    egui::Label::new(egui::RichText::new(record.report()).monospace().size(12.0))
                        .wrap()
                        .selectable(true),
                );
            });
        return;
    }
    ui.horizontal_wrapped(|ui| {
        for (kind, label) in [
            (PromptPartKind::Instruction, "Instructions"),
            (PromptPartKind::Style, "Style"),
            (PromptPartKind::History, "History"),
            (PromptPartKind::Terminology, "Terminology"),
            (PromptPartKind::Context, "Context"),
            (PromptPartKind::Input, "Input"),
        ] {
            ui.label(
                egui::RichText::new(tr(language, label))
                    .small()
                    .color(part_color(kind, ui.visuals().dark_mode)),
            );
        }
    });
    let content_height = (bottom - ui.cursor().top()).max(100.0);
    let dark = ui.visuals().dark_mode;
    if state.preview.as_ref().is_none_or(|preview| {
        preview.id != record.id || preview.started_ms != record.started_ms || preview.dark != dark
    }) {
        state.preview = Some(Preview {
            id: record.id,
            started_ms: record.started_ms,
            dark,
            rows: colored_rows(&record, dark),
            measurements: Default::default(),
        });
    }
    let preview = state.preview.as_mut().unwrap();
    ui.push_id(record.id, |ui| {
        ui.allocate_ui(egui::vec2(ui.available_width(), content_height), |ui| {
            crate::ui::virtual_list::show_variable_virtual_rows(
                ui,
                "model_prompt_rows",
                2.0,
                crate::ui::virtual_list::ScrollTarget::None,
                None,
                |ui| {
                    preview.measurements.prepare(
                        ui,
                        egui::IdSalt::new(record.id),
                        language,
                        || {
                            (0..preview.rows.len())
                                .map(|index| crate::ui::virtual_list::MeasuredRow {
                                    id: egui::Id::unique(index),
                                    signature: egui::IdSalt::new(index),
                                    entry: index,
                                })
                                .collect()
                        },
                        |row| {
                            let mut job = preview.rows[row.entry].clone();
                            job.wrap.max_width = ui.available_width();
                            ui.fonts_mut(|fonts| fonts.layout_job(job))
                                .size()
                                .y
                                .max(16.0)
                        },
                    )
                },
                |ui, _, _, row| {
                    ui.add(
                        egui::Label::new(preview.rows[row.entry].clone())
                            .wrap()
                            .selectable(true),
                    );
                },
            );
        });
    });
}

fn part_color(kind: PromptPartKind, dark: bool) -> Color32 {
    let (r, g, b) = match kind {
        PromptPartKind::Instruction => {
            return if dark {
                Color32::LIGHT_GRAY
            } else {
                theme::text_normal()
            };
        }
        PromptPartKind::Style => {
            if dark {
                (202, 164, 255)
            } else {
                (117, 64, 169)
            }
        }
        PromptPartKind::History => {
            if dark {
                (132, 185, 255)
            } else {
                (37, 95, 170)
            }
        }
        PromptPartKind::Terminology => {
            if dark {
                (246, 194, 117)
            } else {
                (150, 90, 23)
            }
        }
        PromptPartKind::Context => {
            if dark {
                (106, 210, 216)
            } else {
                (29, 117, 126)
            }
        }
        PromptPartKind::Input => {
            if dark {
                (118, 221, 160)
            } else {
                (34, 129, 72)
            }
        }
    };
    Color32::from_rgb(r, g, b)
}

fn colored_rows(record: &ModelRequestRecord, dark: bool) -> Vec<egui::text::LayoutJob> {
    let text = record.prompt_text();
    let mut spans: Vec<(Range<usize>, PromptPartKind)> = Vec::new();
    let mut parts = record.parts.iter().collect::<Vec<_>>();
    parts.sort_by_key(|part| std::cmp::Reverse(part.text.len()));
    for part in parts {
        if part.text.is_empty() {
            continue;
        }
        for (start, _) in text.match_indices(&part.text) {
            let range = start..start + part.text.len();
            if !spans
                .iter()
                .any(|(other, _)| range.start < other.end && other.start < range.end)
            {
                spans.push((range, part.kind));
            }
        }
    }
    spans.sort_by_key(|(range, _)| range.start);
    let mut offset = 0;
    text.split('\n')
        .map(|line| {
            let mut job = egui::text::LayoutJob::default();
            let end = offset + line.len();
            let mut cursor = offset;
            let append = |job: &mut egui::text::LayoutJob, start, end, kind| {
                if end > start {
                    job.append(
                        &text[start..end],
                        0.0,
                        egui::TextFormat {
                            font_id: egui::FontId::proportional(13.0),
                            color: part_color(kind, dark),
                            ..Default::default()
                        },
                    );
                }
            };
            for (range, kind) in spans
                .iter()
                .filter(|(range, _)| range.start < end && range.end > offset)
            {
                append(
                    &mut job,
                    cursor,
                    range.start.max(offset),
                    PromptPartKind::Instruction,
                );
                append(&mut job, range.start.max(offset), range.end.min(end), *kind);
                cursor = range.end.min(end);
            }
            append(&mut job, cursor, end, PromptPartKind::Instruction);
            if line.is_empty() {
                job.append(" ", 0.0, egui::TextFormat::default());
            }
            offset = end + 1;
            job
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use xrtranslate_prompt::diagnostics::PromptPart;

    fn fixture() -> State {
        let style = "保持原意，语气自然，不解释、不扩写。";
        let history = "English: Can we finish it tomorrow?\nChinese: 我们明天能完成吗？";
        let terms = "context window,上下文窗口";
        let input = "Let's keep the style and shorten the conversation history.";
        let request = format!(
            "Translate the current input into Chinese.\n\n{style}\n\n--- REFERENCE CONTEXT ---\n{history}\n\n{terms}\n--- END REFERENCE CONTEXT ---\n\nCurrent input:\n{input}"
        );
        let parts = [
            (PromptPartKind::Style, style),
            (PromptPartKind::History, history),
            (PromptPartKind::Terminology, terms),
            (PromptPartKind::Input, input),
        ]
        .into_iter()
        .map(|(kind, text)| PromptPart {
            kind,
            text: text.into(),
        })
        .collect();
        let record = ModelRequestRecord {
            id: 12,
            started_ms: 1_763_000_000_000,
            operation: "Translation".into(),
            model: "Hy-MT 7B".into(),
            request: serde_json::json!({"model":"Hy-MT 7B","messages":[{"role":"user","content":request}],"max_tokens":512,"temperature":0.3}),
            parts,
            elapsed_ms: Some(823),
            error: Some("Translation output was rejected: unexpected expansion".into()),
            output: "".into(),
        };
        let mut success = record.clone();
        success.id = 13;
        success.error = None;
        success.elapsed_ms = Some(671);
        success.output = "我们保留风格，缩短对话历史。".into();
        State {
            records: vec![Arc::new(record), Arc::new(success)],
            selected: Some(12),
            ..Default::default()
        }
    }

    #[test]
    fn diagnostics_idle_service_keeps_previous_records_and_waits_before_polling_again() {
        let ctx = egui::Context::default();
        let mut state = fixture();
        state.connected = true;
        for result in [Ok(None), Err("HTTP 503".into())] {
            state.checked = Some(Instant::now() - Duration::from_secs(10));
            state.pending = Some(Arc::new(Mutex::new(Some(result))));
            state.poll(&ctx);
            assert!(state.pending.is_none());
            assert!(state.checked.unwrap().elapsed() < Duration::from_secs(1));
            assert_eq!(state.records.len(), 2);
            assert_eq!(state.selected, Some(12));
        }
        assert_eq!(state.error.as_deref(), Some("HTTP 503"));
        assert!(!state.connected);
    }

    #[test]
    fn diagnostics_idle_page_shows_an_empty_state_without_an_error() {
        let ctx = egui::Context::default();
        let mut state = State::default();
        state.pending = Some(Arc::new(Mutex::new(Some(Ok(None)))));
        state.poll(&ctx);
        assert!(state.error.is_none());
        assert!(state.pending.is_none());
        let output = ctx.run_ui(Default::default(), |ui| {
            render_history(ui, &mut state, crate::i18n::UiLanguage::English);
        });
        assert!(output.shapes.iter().any(|shape| {
            matches!(&shape.shape, egui::epaint::Shape::Text(text) if text.galley.job.text == "No requests yet")
        }));
    }

    #[test]
    fn diagnostics_colors_preserve_the_complete_prompt() {
        let state = fixture();
        let record = &state.records[0];
        let rows = colored_rows(record, false);
        let reconstructed = rows
            .iter()
            .map(|row| {
                if row.text == " " {
                    ""
                } else {
                    row.text.as_str()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(reconstructed, record.prompt_text());
        for kind in [
            PromptPartKind::Style,
            PromptPartKind::History,
            PromptPartKind::Terminology,
            PromptPartKind::Input,
        ] {
            assert!(
                rows.iter()
                    .flat_map(|row| &row.sections)
                    .any(|section| section.format.color == part_color(kind, false))
            );
        }
    }

    #[test]
    fn diagnostics_request_list_fills_failed_requests_red() {
        let ctx = egui::Context::default();
        let mut state = fixture();
        let mut red = false;
        for width in [420.0, 800.0] {
            let mut dark = false;
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 600.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    dark = ui.visuals().dark_mode;
                    render_history(ui, &mut state, crate::i18n::UiLanguage::English);
                },
            );
            red |= output.shapes.iter().any(|shape| matches!(&shape.shape, egui::epaint::Shape::Rect(rect) if rect.fill == Color32::from_rgba_unmultiplied(200, 55, 55, if dark { 65 } else { 30 })));
        }
        assert!(red);
    }
}
