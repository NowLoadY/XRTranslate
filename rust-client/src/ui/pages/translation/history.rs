//! Recognition and translation feeds share layouts, scrolling, and display preferences.
#[cfg(test)]
use crate::CaptureSource;
use crate::ui::components;
use crate::ui::components::annotated_text::{AnnotatedText, TextLayout};
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};

fn recognition_history_fingerprint(
    entries: &[crate::history::RecognitionHistoryEntry],
    partial_text: &str,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    entries.len().hash(&mut hasher);
    if let Some(first) = entries.first() {
        first.stream_id.hash(&mut hasher);
        first.turn_id.hash(&mut hasher);
        first.text.hash(&mut hasher);
    }
    if let Some(last) = entries.last() {
        last.stream_id.hash(&mut hasher);
        last.turn_id.hash(&mut hasher);
        last.speaker_id.hash(&mut hasher);
        last.text.hash(&mut hasher);
    }
    partial_text.hash(&mut hasher);
    hasher.finish()
}

fn translation_history_fingerprint(entries: &[crate::history::TranslationHistoryEntry]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    entries.len().hash(&mut hasher);
    if let Some(first) = entries.first() {
        first.stream_id.hash(&mut hasher);
        first.turn_id.hash(&mut hasher);
        first.segment_index.hash(&mut hasher);
        first.source.hash(&mut hasher);
        first.translated.hash(&mut hasher);
        hash_translation_outputs(first, &mut hasher);
    }
    if let Some(last) = entries.last() {
        last.stream_id.hash(&mut hasher);
        last.turn_id.hash(&mut hasher);
        last.segment_index.hash(&mut hasher);
        last.speaker_id.hash(&mut hasher);
        last.source.hash(&mut hasher);
        last.translated.hash(&mut hasher);
        hash_translation_outputs(last, &mut hasher);
    }
    hasher.finish()
}

fn hash_translation_outputs(
    entry: &crate::history::TranslationHistoryEntry,
    hasher: &mut impl Hasher,
) {
    entry.asr_only.hash(hasher);
    entry.additional_translations.len().hash(hasher);
    for output in &entry.additional_translations {
        output.target_lang.hash(hasher);
        output.translated_text.hash(hasher);
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum FullscreenHistory {
    Recognition,
    Translation,
}

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
    pub recognition: DisplayMode,
    pub translation: DisplayMode,
}

impl FullscreenHistory {
    fn title(self) -> &'static str {
        match self {
            Self::Recognition => "Recognition History",
            Self::Translation => "Translation History",
        }
    }

    fn display(self, preferences: &mut HistoryDisplay) -> &mut DisplayMode {
        match self {
            Self::Recognition => &mut preferences.recognition,
            Self::Translation => &mut preferences.translation,
        }
    }
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

fn translation_output_text<'a>(
    ui: &egui::Ui,
    entry: &'a crate::history::TranslationHistoryEntry,
    language: crate::i18n::UiLanguage,
    text_size: f32,
    label_size: f32,
    show_speaker: bool,
) -> AnnotatedText<'a> {
    let mut text = history_text(
        ui,
        if show_speaker { &entry.speaker_id } else { "" },
        label_size,
    );
    if entry.asr_only {
        text.append(
            ui,
            egui::RichText::new(format!("{}  ", crate::i18n::tr(language, "ASR only")))
                .color(crate::ui::theme::text_weak())
                .size(label_size),
        );
    }
    text.append_terms(
        ui,
        &entry.translated,
        &entry.term_matches,
        &[],
        crate::ui::theme::text_strong(),
        text_size,
    );
    for output in &entry.additional_translations {
        text.append(ui, egui::RichText::new("\n").size(text_size));
        text.append_terms(
            ui,
            &output.translated_text,
            &output.term_matches,
            &[],
            crate::ui::theme::text_strong(),
            text_size,
        );
    }
    text
}

fn prepare_translation_history_row<'a>(
    ui: &egui::Ui,
    entry: &'a crate::history::TranslationHistoryEntry,
    scale: &HistoryFeedScale,
    language: crate::i18n::UiLanguage,
) -> (f32, HistoryRow<'a>) {
    let show_source = !entry.source.is_empty() && !entry.asr_only;
    let translated = translation_output_text(
        ui,
        entry,
        language,
        scale.text_size,
        scale.speaker_size,
        !show_source,
    );
    if show_source {
        let mut source = history_text(ui, &entry.speaker_id, scale.speaker_size);
        source.append(
            ui,
            egui::RichText::new(&entry.source)
                .color(crate::ui::theme::text_weak())
                .size(scale.source_size),
        );
        prepare_history_row(ui, scale, source, Some(translated))
    } else {
        prepare_history_row(ui, scale, translated, None)
    }
}

fn prepare_history_row<'a>(
    ui: &egui::Ui,
    scale: &HistoryFeedScale,
    text: AnnotatedText<'a>,
    translation: Option<AnnotatedText<'a>>,
) -> (f32, HistoryRow<'a>) {
    let margin = history_card_frame(ui.ctx(), scale).total_margin().sum();
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
        + crate::ui::theme::data_text_motion(ui.ctx()).max_offset
        + margin.y;
    (height.max(scale.min_row_height), row)
}

fn history_activity(index: usize, row_count: usize) -> f32 {
    if row_count <= 1 {
        return 1.0;
    }
    index as f32 / (row_count - 1) as f32
}

fn prepare_recognition_row<'a>(
    ui: &egui::Ui,
    entry: &'a crate::history::RecognitionHistoryEntry,
    scale: &HistoryFeedScale,
) -> (f32, HistoryRow<'a>) {
    let mut text = history_text(ui, &entry.speaker_id, scale.speaker_size);
    text.append_terms(
        ui,
        &entry.text,
        &entry.activation_matches,
        &entry.context_matches,
        crate::ui::theme::text_normal(),
        scale.text_size,
    );
    prepare_history_row(ui, scale, text, None)
}

fn text_row<'a>(ui: &egui::Ui, text: AnnotatedText<'a>) -> (f32, HistoryRow<'a>) {
    let text = text.layout(ui, ui.available_width());
    (
        text.height(),
        HistoryRow {
            text,
            translation: None,
        },
    )
}

fn plain_rows<'a>(
    app: &'a crate::XRTranslateApp,
    ui: &egui::Ui,
    kind: FullscreenHistory,
    scale: &HistoryFeedScale,
) -> Vec<(f32, HistoryRow<'a>)> {
    match kind {
        FullscreenHistory::Recognition => app
            .recognition_history
            .chunk_by(|a, b| {
                !a.turn_id.is_empty()
                    && a.turn_id == b.turn_id
                    && a.stream_id == b.stream_id
                    && a.speaker_id == b.speaker_id
            })
            .map(|group| {
                let mut text = history_text(ui, &group[0].speaker_id, scale.speaker_size);
                for entry in group {
                    text.append_segment(
                        ui,
                        &entry.text,
                        &entry.activation_matches,
                        &entry.context_matches,
                        crate::ui::theme::text_normal(),
                        scale.text_size,
                    );
                }
                text_row(ui, text)
            })
            .collect(),
        FullscreenHistory::Translation => app
            .translations
            .chunk_by(|a, b| {
                !a.turn_id.is_empty()
                    && a.turn_id == b.turn_id
                    && a.stream_id == b.stream_id
                    && a.speaker_id == b.speaker_id
                    && a.audio_source == b.audio_source
                    && a.asr_only == b.asr_only
            })
            .map(|group| {
                let mut text = history_text(ui, &group[0].speaker_id, scale.speaker_size);
                if group[0].asr_only {
                    text.append(
                        ui,
                        egui::RichText::new(format!(
                            "{}  ",
                            crate::i18n::tr(app.ui_language, "ASR only")
                        ))
                        .color(crate::ui::theme::text_weak())
                        .size(scale.speaker_size),
                    );
                }
                for entry in group {
                    text.append_segment(
                        ui,
                        &entry.translated,
                        &entry.term_matches,
                        &[],
                        crate::ui::theme::text_strong(),
                        scale.text_size,
                    );
                }
                // Each extra language stays a separate paragraph, including when it
                // arrives after the main translation. Sentence order stays unchanged.
                let mut languages = Vec::new();
                for extra in group
                    .iter()
                    .flat_map(|entry| &entry.additional_translations)
                {
                    if !languages.contains(&extra.target_lang.as_str()) {
                        languages.push(extra.target_lang.as_str());
                    }
                }
                for language in languages {
                    text.append(ui, egui::RichText::new("\n").size(scale.text_size));
                    for extra in group
                        .iter()
                        .flat_map(|entry| &entry.additional_translations)
                        .filter(|extra| extra.target_lang == language)
                    {
                        text.append_segment(
                            ui,
                            &extra.translated_text,
                            &extra.term_matches,
                            &[],
                            crate::ui::theme::text_strong(),
                            scale.text_size,
                        );
                    }
                }
                text_row(ui, text)
            })
            .collect(),
    }
}

fn prepare_rows<'a>(
    app: &'a crate::XRTranslateApp,
    ui: &egui::Ui,
    kind: FullscreenHistory,
    scale: &HistoryFeedScale,
    plain: bool,
) -> Vec<(f32, HistoryRow<'a>)> {
    let mut rows = if plain {
        plain_rows(app, ui, kind, scale)
    } else {
        match kind {
            FullscreenHistory::Recognition => app
                .recognition_history
                .iter()
                .map(|entry| prepare_recognition_row(ui, entry, scale))
                .collect(),
            FullscreenHistory::Translation => app
                .translations
                .iter()
                .map(|entry| prepare_translation_history_row(ui, entry, scale, app.ui_language))
                .collect(),
        }
    };
    if kind == FullscreenHistory::Recognition && !app.partial_text.is_empty() {
        let mut text = AnnotatedText::default();
        text.append(
            ui,
            egui::RichText::new("• • •  ")
                .color(crate::ui::theme::primary())
                .size(scale.speaker_size),
        );
        text.append(
            ui,
            egui::RichText::new(&app.partial_text)
                .color(crate::ui::theme::primary_dark())
                .size(scale.text_size)
                .italics(),
        );
        rows.push(if plain {
            text_row(ui, text)
        } else {
            prepare_history_row(ui, scale, text, None)
        });
    }
    rows
}

fn render_feed(
    app: &crate::XRTranslateApp,
    ui: &mut egui::Ui,
    kind: FullscreenHistory,
    scale: &HistoryFeedScale,
    display: DisplayMode,
) -> bool {
    let plain = display == DisplayMode::Text;
    let fingerprint = match kind {
        FullscreenHistory::Recognition => {
            recognition_history_fingerprint(&app.recognition_history, &app.partial_text)
        }
        FullscreenHistory::Translation => translation_history_fingerprint(&app.translations),
    };
    let stamp = (fingerprint, plain);
    let id = ui.make_persistent_id("history_scroll_state");
    let changed = ui.data(|data| data.get_temp::<(u64, bool)>(id)) != Some(stamp);
    let mut clicked = false;
    crate::ui::layout::show_variable_virtual_rows(
        ui,
        "history_scroll",
        if plain { 12.0 } else { scale.row_gap },
        changed,
        |ui| {
            let mut rows = prepare_rows(app, ui, kind, scale, plain);
            if rows.is_empty() {
                let mut text = AnnotatedText::default();
                text.append(
                    ui,
                    egui::RichText::new(crate::i18n::tr(
                        app.ui_language,
                        match kind {
                            FullscreenHistory::Recognition => "No speech",
                            FullscreenHistory::Translation => "No translations",
                        },
                    ))
                    .color(crate::ui::theme::text_weak())
                    .size(scale.text_size)
                    .italics(),
                );
                rows.push(text_row(ui, text));
            }
            rows
        },
        |ui, index, height, row| {
            if plain {
                // Keep normal text selection; only the heading opens full screen.
                row.show(ui);
            } else {
                let (count, live) = match kind {
                    FullscreenHistory::Recognition => (
                        app.recognition_history.len() + usize::from(!app.partial_text.is_empty()),
                        app.recognition_history
                            .get(index)
                            .is_none_or(|entry| entry.live),
                    ),
                    FullscreenHistory::Translation => (
                        app.translations.len(),
                        app.translations.get(index).is_some_and(|entry| entry.live),
                    ),
                };
                let activity = if live {
                    1.0
                } else {
                    history_activity(index, count)
                };
                let id = ui.make_persistent_id(("history_data", index));
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
    kind: FullscreenHistory,
    header_size: f32,
    openable: bool,
) -> (bool, DisplayMode) {
    let count = match kind {
        FullscreenHistory::Recognition => app.recognition_history.len(),
        FullscreenHistory::Translation => app.translations.len(),
    };
    let response = ui.add(
        egui::Label::new(
            egui::RichText::new(format!(
                "{} ({count})",
                crate::i18n::tr(app.ui_language, kind.title())
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
    let display = kind.display(&mut app.history_display);
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
    (clicked, *kind.display(&mut app.history_display))
}

pub(super) fn render_history_feeds(
    app: &mut crate::XRTranslateApp,
    ui: &mut egui::Ui,
    available_width: f32,
    available_height: f32,
) {
    let stacked = crate::ui::layout::should_stack(available_width, 2, 140.0);
    let width = if stacked {
        available_width
    } else {
        (available_width - ui.spacing().item_spacing.x) * 0.5
    };
    let scale = HistoryFeedScale::compute(width, available_height);
    let height = if stacked {
        ((available_height - 20.0) / 2.0).clamp(100.0, 320.0)
    } else {
        (available_height * 0.85 - 56.0).clamp(160.0, 520.0)
    };
    let mut fullscreen = None;
    ui.columns(if stacked { 1 } else { 2 }, |columns| {
        for (index, kind) in [
            FullscreenHistory::Recognition,
            FullscreenHistory::Translation,
        ]
        .into_iter()
        .enumerate()
        {
            let ui = &mut columns[if stacked { 0 } else { index }];
            if stacked && index > 0 {
                ui.add_space(10.0);
            }
            ui.push_id(kind, |ui| {
                egui::Frame::new()
                    .fill(crate::ui::theme::panel_fill(
                        ui.ctx(),
                        crate::ui::theme::surface_subtle(),
                    ))
                    .corner_radius(scale.card_radius + 2)
                    .inner_margin(egui::Margin::symmetric(
                        if scale.header_size < 14.0 { 8 } else { 12 },
                        if scale.header_size < 14.0 { 6 } else { 10 },
                    ))
                    .stroke(egui::Stroke::new(1.0, crate::ui::theme::border()))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let (header_clicked, display) =
                            render_header(app, ui, kind, scale.header_size, true);
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
                                render_feed(app, ui, kind, &scale, display)
                            })
                            .inner;
                        if header_clicked || body_clicked {
                            fullscreen = Some(kind);
                        }
                    });
            });
        }
    });
    if let Some(kind) = fullscreen {
        app.fullscreen_history = Some(kind);
        ui.ctx().request_repaint();
    }
}

pub(super) fn render_fullscreen_history(
    app: &mut crate::XRTranslateApp,
    ui: &mut egui::Ui,
    kind: FullscreenHistory,
) {
    if components::animated_button(
        ui,
        &format!("← {}", crate::i18n::tr(app.ui_language, "Back")),
    )
    .clicked()
    {
        app.fullscreen_history = None;
    }
    ui.push_id(("fullscreen_history", kind), |ui| {
        let (_, display) = render_header(app, ui, kind, 18.0, false);
        ui.add_space(10.0);
        let scale = HistoryFeedScale {
            text_size: 16.0,
            source_size: 13.5,
            speaker_size: 12.0,
            ..HistoryFeedScale::compute(ui.available_width(), ui.available_height())
        };
        render_feed(app, ui, kind, &scale, display);
    });
}

#[cfg(test)]
mod virtual_history_tests {
    use super::*;

    fn translation_entry() -> crate::history::TranslationHistoryEntry {
        crate::history::TranslationHistoryEntry::preview(
            1,
            CaptureSource::Microphone,
            xrtranslate_protocol::TranslationPreview {
                source_text: "raw recognition".into(),
                translated_text: "corrected result".into(),
                turn_id: "turn-1".into(),
                segment_index: 0,
                revision: 1,
                speaker_id: String::new(),
            },
        )
    }

    #[test]
    fn history_activity_increases_towards_the_newest_row() {
        assert_eq!(history_activity(0, 0), 1.0);
        assert_eq!(history_activity(0, 1), 1.0);
        assert_eq!(history_activity(0, 5), 0.0);
        assert_eq!(history_activity(4, 5), 1.0);
    }

    #[test]
    fn asr_only_history_renders_the_final_pipeline_output_once() {
        let ctx = egui::Context::default();
        let mut entry = translation_entry();
        entry.asr_only = true;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let (_, row) = prepare_translation_history_row(
                    ui,
                    &entry,
                    &HistoryFeedScale::compute(300.0, 400.0),
                    crate::i18n::UiLanguage::English,
                );
                assert!(row.translation.is_none());
                row.show(ui);
            });
        });
        output.textures_delta.clear();
        let rendered = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.text()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(rendered, vec!["ASR only  corrected result"]);
    }

    #[test]
    fn extra_outputs_invalidate_history_and_contribute_to_measured_height() {
        let ctx = egui::Context::default();
        let mut entry = translation_entry();
        let original_fingerprint = translation_history_fingerprint(std::slice::from_ref(&entry));
        let mut original_height = 0.0;
        let scale = HistoryFeedScale::compute(300.0, 400.0);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                original_height = prepare_translation_history_row(
                    ui,
                    &entry,
                    &scale,
                    crate::i18n::UiLanguage::English,
                )
                .0;
            });
        });
        output.textures_delta.clear();
        entry
            .additional_translations
            .push(xrtranslate_protocol::AdditionalTranslation {
                target_lang: "fr".into(),
                translated_text: "bonjour".into(),
                term_matches: Vec::new(),
            });
        assert_ne!(
            original_fingerprint,
            translation_history_fingerprint(std::slice::from_ref(&entry))
        );
        let mut output = ctx.run_ui(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let (height, row) = prepare_translation_history_row(
                    ui,
                    &entry,
                    &scale,
                    crate::i18n::UiLanguage::English,
                );
                assert!(height > original_height);
                row.show(ui);
            });
        });
        output.textures_delta.clear();
        assert!(output.shapes.iter().any(|shape| matches!(
            &shape.shape,
            egui::Shape::Text(text) if text.galley.text() == "corrected result\nbonjour"
        )));
    }
}
