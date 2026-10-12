//! One chronological feed for source text, translation previews and completed results.
use crate::ui::components;
use crate::ui::components::annotated_text::{AnnotatedText, TextLayout};
use eframe::egui;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayMode {
    #[default]
    Bubbles,
    Text,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HistoryDisplay {
    #[serde(rename = "translation")]
    pub mode: DisplayMode,
}

#[derive(Clone, Copy, Debug)]
struct HistoryFeedScale {
    text_size: f32,
    source_size: f32,
    speaker_size: f32,
    header_size: f32,
    min_row_height: f32,
    card_margin_x: i8,
    card_margin_y: i8,
    card_radius: u8,
    row_gap: f32,
}

impl HistoryFeedScale {
    fn compute(col_width: f32, available_height: f32) -> Self {
        let width_factor = ((col_width - 130.0) / 150.0).clamp(0.0, 1.0);
        let height_factor = ((available_height - 160.0) / 200.0).clamp(0.0, 1.0);
        let factor = width_factor.min(height_factor);

        let text_size = 10.5 + 2.5 * factor;
        let source_size = 9.0 + 2.5 * factor;
        let speaker_size = 9.5 + 2.0 * factor;
        let header_size = 12.0 + 3.0 * factor;
        let min_row_height = 32.0 + 16.0 * factor;
        let card_margin_x = (5.0 + 5.0 * factor).round() as i8;
        let card_margin_y = (2.0 + 3.0 * factor).round() as i8;
        let card_radius = (5.0 + 4.0 * factor).round() as u8;
        let row_gap = 3.0 + 3.0 * factor;

        Self {
            text_size,
            source_size,
            speaker_size,
            header_size,
            min_row_height,
            card_margin_x,
            card_margin_y,
            card_radius,
            row_gap,
        }
    }
}

fn history_card_frame(ctx: &egui::Context, scale: &HistoryFeedScale) -> egui::Frame {
    egui::Frame::new()
        .fill(crate::ui::theme::panel_fill(
            ctx,
            crate::ui::theme::history_surface(),
        ))
        .corner_radius(egui::CornerRadius::same(scale.card_radius))
        .inner_margin(egui::Margin::symmetric(
            scale.card_margin_x,
            scale.card_margin_y,
        ))
        .stroke(egui::Stroke::new(
            1.0,
            crate::ui::theme::border().gamma_multiply(0.35),
        ))
}

fn history_card_with_activity(
    ui: &mut egui::Ui,
    id: egui::Id,
    activity: f32,
    row_height: f32,
    scale: &HistoryFeedScale,
    add_contents: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    let frame = history_card_frame(ui.ctx(), scale);
    let res = frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height((row_height - frame.total_margin().sum().y).max(0.0));
        ui.spacing_mut().item_spacing.y = 2.0;
        crate::ui::animation::AnimationSystem::render_data_text(ui, id, activity, add_contents);
    });
    res.response.interact(egui::Sense::click())
}

struct HistoryRow<'a> {
    text: TextLayout<'a>,
    translation: Option<TextLayout<'a>>,
}

impl HistoryRow<'_> {
    fn show(&self, ui: &mut egui::Ui) {
        self.text.show(ui);
        if let Some(translation) = &self.translation {
            translation.show(ui);
        }
    }
}

fn history_text<'a>(ui: &egui::Ui, speaker_id: &str, speaker_size: f32) -> AnnotatedText<'a> {
    let mut text = AnnotatedText::default();
    if let Some(speaker) = crate::compact_speaker_label(speaker_id) {
        text.append(
            ui,
            egui::RichText::new(format!("{speaker}  "))
                .color(crate::ui::theme::primary_dark())
                .size(speaker_size),
        );
    }
    text
}

/// A turn keeps its place while recognition and translation update independently.
struct HistoryTurn<'a> {
    stream_id: Option<u64>,
    turn_id: &'a str,
    recognition: Vec<&'a crate::history::RecognitionHistoryEntry>,
    translations: Vec<&'a crate::history::TranslationHistoryEntry>,
}

impl HistoryTurn<'_> {
    fn live(&self) -> bool {
        self.recognition.iter().any(|entry| entry.live)
            || self.translations.iter().any(|entry| entry.live)
    }

    fn speaker(&self) -> &str {
        self.recognition
            .iter()
            .map(|entry| entry.speaker_id.as_str())
            .chain(
                self.translations
                    .iter()
                    .map(|entry| entry.speaker_id.as_str()),
            )
            .find(|speaker| !speaker.is_empty())
            .unwrap_or_default()
    }
}

fn history_turns(app: &crate::XRTranslateApp) -> Vec<HistoryTurn<'_>> {
    let mut turns: Vec<HistoryTurn<'_>> = Vec::new();
    for entry in &app.recognition_history {
        if entry.text.trim().is_empty() {
            continue;
        }
        if let Some(turn) = turns.iter_mut().find(|turn| {
            !entry.turn_id.is_empty()
                && turn.stream_id == entry.stream_id
                && turn.turn_id == entry.turn_id
        }) {
            turn.recognition.push(entry);
        } else {
            turns.push(HistoryTurn {
                stream_id: entry.stream_id,
                turn_id: &entry.turn_id,
                recognition: vec![entry],
                translations: Vec::new(),
            });
        }
    }
    for entry in &app.translations {
        if entry.source.trim().is_empty()
            && entry.translated.trim().is_empty()
            && entry
                .additional_translations
                .iter()
                .all(|extra| extra.translated_text.trim().is_empty())
        {
            continue;
        }
        let index = turns
            .iter()
            .position(|turn| {
                turn.stream_id == entry.stream_id
                    && if entry.turn_id.is_empty() {
                        turn.turn_id.is_empty()
                            && turn
                                .recognition
                                .iter()
                                .any(|source| source.text == entry.source)
                    } else {
                        turn.turn_id == entry.turn_id
                    }
            })
            .unwrap_or_else(|| {
                turns.push(HistoryTurn {
                    stream_id: entry.stream_id,
                    turn_id: &entry.turn_id,
                    recognition: Vec::new(),
                    translations: Vec::new(),
                });
                turns.len() - 1
            });
        let translations = &mut turns[index].translations;
        if let Some(existing) = translations.iter_mut().find(|existing| {
            !entry.turn_id.is_empty() && existing.segment_index == entry.segment_index
        }) {
            if entry.revision_id >= existing.revision_id {
                *existing = entry;
            }
        } else {
            translations.push(entry);
        }
    }
    for turn in &mut turns {
        turn.translations.sort_by_key(|entry| entry.segment_index);
    }
    turns.sort_by_key(|turn| app.history_timeline.position(turn.stream_id, turn.turn_id));
    turns
}

fn prepare_turn<'a>(
    ui: &egui::Ui,
    turn: &HistoryTurn<'a>,
    scale: &HistoryFeedScale,
    plain: bool,
) -> (f32, HistoryRow<'a>) {
    let mut has_output = turn.translations.iter().any(|entry| {
        !entry.translated.trim().is_empty()
            || entry
                .additional_translations
                .iter()
                .any(|extra| !extra.translated_text.trim().is_empty())
    });
    let output_is_source = turn.translations.iter().all(|entry| entry.asr_only);
    if output_is_source && !turn.recognition.is_empty() {
        // ASR-only output replaces raw recognition once the whole turn is
        // covered; an arriving prefix must not hide the still-pending tail.
        has_output &= turn
            .recognition
            .iter()
            .flat_map(|entry| entry.text.chars())
            .filter(|character| !character.is_whitespace())
            .eq(turn
                .translations
                .iter()
                .flat_map(|entry| entry.source.chars())
                .filter(|character| !character.is_whitespace()));
    }
    let has_source = !turn.recognition.is_empty()
        || turn
            .translations
            .iter()
            .any(|entry| !entry.source.trim().is_empty());
    let show_source = has_source && !(has_output && output_is_source);
    let mut source = history_text(ui, turn.speaker(), scale.speaker_size);
    if show_source {
        let color = if has_output {
            crate::ui::theme::text_weak()
        } else {
            crate::ui::theme::text_strong()
        };
        let size = if has_output {
            scale.source_size
        } else {
            scale.text_size
        };
        if turn.recognition.is_empty() {
            for entry in &turn.translations {
                source.append_segment(ui, &entry.source, &[], &[], color, size);
            }
        } else {
            for entry in &turn.recognition {
                source.append_segment(
                    ui,
                    &entry.text,
                    &entry.activation_matches,
                    &entry.context_matches,
                    color,
                    size,
                );
            }
        }
    }
    let mut output = history_text(
        ui,
        if show_source { "" } else { turn.speaker() },
        scale.speaker_size,
    );
    let mut output_started = false;
    for entry in &turn.translations {
        if !entry.translated.trim().is_empty() {
            output.append_segment(
                ui,
                &entry.translated,
                &entry.term_matches,
                &[],
                crate::ui::theme::text_strong(),
                scale.text_size,
            );
            output_started = true;
        }
    }
    let mut languages = Vec::new();
    for extra in turn
        .translations
        .iter()
        .flat_map(|entry| &entry.additional_translations)
    {
        if !extra.translated_text.trim().is_empty()
            && !languages.contains(&extra.target_lang.as_str())
        {
            languages.push(extra.target_lang.as_str());
        }
    }
    for language in languages {
        if output_started {
            output.append(ui, egui::RichText::new("\n").size(scale.text_size));
        }
        for extra in turn
            .translations
            .iter()
            .flat_map(|entry| &entry.additional_translations)
            .filter(|extra| {
                extra.target_lang == language && !extra.translated_text.trim().is_empty()
            })
        {
            output.append_segment(
                ui,
                &extra.translated_text,
                &extra.term_matches,
                &[],
                crate::ui::theme::text_strong(),
                scale.text_size,
            );
            output_started = true;
        }
    }
    if show_source {
        prepare_history_row(ui, scale, source, has_output.then_some(output), plain)
    } else {
        prepare_history_row(ui, scale, output, None, plain)
    }
}

fn prepare_history_row<'a>(
    ui: &egui::Ui,
    scale: &HistoryFeedScale,
    text: AnnotatedText<'a>,
    translation: Option<AnnotatedText<'a>>,
    plain: bool,
) -> (f32, HistoryRow<'a>) {
    let margin = if plain {
        egui::Vec2::ZERO
    } else {
        history_card_frame(ui.ctx(), scale).total_margin().sum()
    };
    let width = (ui.available_width() - margin.x).max(1.0);
    let row = HistoryRow {
        text: text.layout(ui, width),
        translation: translation.map(|text| text.layout(ui, width)),
    };
    let height = row.text.height()
        + row
            .translation
            .as_ref()
            .map_or(0.0, |text| text.height() + 2.0)
        + if plain {
            0.0
        } else {
            crate::ui::theme::data_text_motion(ui.ctx()).max_offset
        }
        + margin.y;
    (
        if plain {
            height
        } else {
            height.max(scale.min_row_height)
        },
        row,
    )
}

fn history_activity(index: usize, row_count: usize) -> f32 {
    if row_count <= 1 {
        return 1.0;
    }
    index as f32 / (row_count - 1) as f32
}

fn render_feed(
    app: &crate::XRTranslateApp,
    ui: &mut egui::Ui,
    scale: &HistoryFeedScale,
    display: DisplayMode,
) -> bool {
    let plain = display == DisplayMode::Text;
    let turns = history_turns(app);
    let stamp = (app.history_timeline.revision, plain);
    let id = ui.make_persistent_id("history_scroll_state");
    let changed = ui.data(|data| data.get_temp::<(u64, bool)>(id)) != Some(stamp);
    let count = turns.len() + usize::from(!app.partial_text.is_empty());
    let mut clicked = false;
    crate::ui::layout::show_variable_virtual_rows(
        ui,
        "history_scroll",
        if plain { 12.0 } else { scale.row_gap },
        changed,
        |ui| {
            let mut rows = turns
                .iter()
                .map(|turn| prepare_turn(ui, turn, scale, plain))
                .collect::<Vec<_>>();
            if !app.partial_text.is_empty() {
                let mut text = AnnotatedText::default();
                text.append(
                    ui,
                    egui::RichText::new(&app.partial_text)
                        .color(crate::ui::theme::primary_dark())
                        .size(scale.text_size),
                );
                rows.push(prepare_history_row(ui, scale, text, None, plain));
            }
            if rows.is_empty() {
                let mut text = AnnotatedText::default();
                text.append(
                    ui,
                    egui::RichText::new(crate::i18n::tr(app.ui_language, "No results yet"))
                        .color(crate::ui::theme::text_weak())
                        .size(scale.text_size),
                );
                rows.push(prepare_history_row(ui, scale, text, None, true));
            }
            rows
        },
        |ui, index, height, row| {
            ui.spacing_mut().item_spacing.y = 2.0;
            if plain || count == 0 {
                row.show(ui);
            } else {
                let turn = turns.get(index);
                let activity = if turn.is_none_or(HistoryTurn::live) {
                    1.0
                } else {
                    history_activity(index, count)
                };
                let id = turn.map_or_else(
                    || ui.make_persistent_id("history_partial"),
                    |turn| {
                        ui.make_persistent_id((
                            "history_data",
                            turn.stream_id,
                            turn.turn_id,
                            if turn.turn_id.is_empty() { index } else { 0 },
                        ))
                    },
                );
                clicked |=
                    history_card_with_activity(ui, id, activity, height, scale, |ui| row.show(ui))
                        .clicked();
            }
        },
    );
    ui.data_mut(|data| data.insert_temp(id, stamp));
    clicked
}

fn render_header(
    app: &mut crate::XRTranslateApp,
    ui: &mut egui::Ui,
    header_size: f32,
    openable: bool,
) -> (bool, DisplayMode) {
    let count = history_turns(app).len() + usize::from(!app.partial_text.is_empty());
    let response = ui.add(
        egui::Label::new(
            egui::RichText::new(format!(
                "{} ({count})",
                crate::i18n::tr(app.ui_language, "History")
            ))
            .size(header_size)
            .color(crate::ui::theme::text_strong())
            .strong(),
        )
        .truncate()
        .sense(if openable {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        }),
    );
    let clicked = openable && response.clicked();
    if openable {
        response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(crate::i18n::tr(
                app.ui_language,
                "Click to view full screen",
            ));
    }
    let display = &mut app.history_display.mode;
    let mut plain = *display == DisplayMode::Text;
    if components::segmented_switch(
        ui,
        &mut plain,
        [
            crate::i18n::tr(app.ui_language, "Bubbles"),
            crate::i18n::tr(app.ui_language, "Plain text"),
        ],
    )
    .changed()
    {
        *display = if plain {
            DisplayMode::Text
        } else {
            DisplayMode::Bubbles
        };
        app.save_settings();
    }
    (clicked, app.history_display.mode)
}

pub(super) fn render_history_feed(
    app: &mut crate::XRTranslateApp,
    ui: &mut egui::Ui,
    available_width: f32,
    available_height: f32,
) {
    let scale = HistoryFeedScale::compute(available_width, available_height);
    let height = (available_height * 0.85 - 56.0).clamp(160.0, 520.0);
    let mut fullscreen = false;
    egui::Frame::new()
        .fill(crate::ui::theme::panel_fill(
            ui.ctx(),
            crate::ui::theme::surface_subtle(),
        ))
        .corner_radius(scale.card_radius + 2)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .stroke(egui::Stroke::new(1.0, crate::ui::theme::border()))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let (header_clicked, display) = render_header(app, ui, scale.header_size, true);
            ui.add_space(6.0);
            let body_clicked = egui::Frame::new()
                .fill(crate::ui::theme::history_viewport())
                .corner_radius(scale.card_radius)
                .inner_margin(egui::Margin::symmetric(
                    scale.card_margin_x,
                    scale.card_margin_y,
                ))
                .show(ui, |ui| {
                    ui.set_height((height - 8.0).max(0.0));
                    render_feed(app, ui, &scale, display)
                })
                .inner;
            fullscreen = header_clicked || body_clicked;
        });
    if fullscreen {
        app.fullscreen_history = true;
        ui.ctx().request_repaint();
    }
}

pub(super) fn render_fullscreen_history(app: &mut crate::XRTranslateApp, ui: &mut egui::Ui) {
    if components::back_button(ui, crate::i18n::tr(app.ui_language, "Back")).clicked() {
        app.fullscreen_history = false;
    }
    ui.push_id("fullscreen_history", |ui| {
        let (_, display) = render_header(app, ui, 18.0, false);
        ui.add_space(10.0);
        let scale = HistoryFeedScale {
            text_size: 16.0,
            source_size: 13.5,
            speaker_size: 12.0,
            ..HistoryFeedScale::compute(ui.available_width(), ui.available_height())
        };
        render_feed(app, ui, &scale, display);
    });
}
