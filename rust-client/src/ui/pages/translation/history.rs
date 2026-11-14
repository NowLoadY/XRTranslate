//! One chronological feed for source text, translation previews and completed results.
use crate::ui::components::annotated_text::{AnnotatedText, TextLayout};
use eframe::egui;
use serde::{Deserialize, Serialize};

mod text;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayMode {
    #[default]
    Bubbles,
    Text,
    Split,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HistoryDisplay {
    #[serde(rename = "translation")]
    pub mode: DisplayMode,
    pub presentation_dark: bool,
    pub presentation_bilingual: bool,
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

fn history_bubble(
    ui: &mut egui::Ui,
    row: &HistoryRow<'_>,
    activity: f32,
    row_height: f32,
    scale: &HistoryFeedScale,
    language: crate::i18n::UiLanguage,
    copy_text: impl FnOnce() -> String,
) -> egui::Response {
    let frame = history_card_frame(ui.ctx(), scale);
    let res = frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height((row_height - frame.total_margin().sum().y).max(0.0));
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.style_mut().interaction.selectable_labels = false;
        crate::ui::animation::AnimationSystem::render_data_text(ui, row.id, activity, |ui| {
            row.show(ui)
        });
    });
    let response = ui.interact(res.response.rect, row.id, egui::Sense::click());
    let now = ui.input(|input| input.time);
    let copied_id = row.id.with("copied");
    if response.clicked() {
        ui.ctx().copy_text(copy_text());
        ui.data_mut(|data| data.insert_temp(copied_id, now));
    }
    let copied = ui
        .data(|data| data.get_temp::<f64>(copied_id))
        .is_some_and(|at| now - at < 1.2);
    if copied {
        ui.painter().rect_stroke(
            response.rect,
            scale.card_radius,
            egui::Stroke::new(1.0, crate::ui::theme::primary().gamma_multiply(0.65)),
            egui::StrokeKind::Inside,
        );
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(1200));
    }
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(crate::i18n::tr(
            language,
            if copied { "Copied" } else { "Copy" },
        ))
}

struct HistoryRow<'a> {
    id: egui::Id,
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

/// Each sentence keeps its place while recognition and translation update independently.
pub(super) struct HistorySegment<'a> {
    stream_id: Option<u64>,
    turn_id: &'a str,
    segment_index: u32,
    recognition: Vec<&'a crate::history::RecognitionHistoryEntry>,
    pub(super) translations: Vec<&'a crate::history::TranslationHistoryEntry>,
}

impl HistorySegment<'_> {
    fn has_output(&self) -> bool {
        self.translations.iter().any(|entry| {
            !entry.translated.trim().is_empty()
                || entry
                    .additional_translations
                    .iter()
                    .any(|extra| !extra.translated_text.trim().is_empty())
        })
    }

    fn asr_output_replaces_source(&self) -> bool {
        !self.translations.is_empty()
            && self.translations.iter().all(|entry| {
                entry.asr_only && !entry.translated.trim().is_empty()
            })
            // An arriving prefix must not hide the still-pending recognition tail.
            && (self.recognition.is_empty()
                || self.recognition.iter().flat_map(|entry| entry.text.chars())
                    .filter(|character| !character.is_whitespace())
                    .eq(self.translations.iter().flat_map(|entry| entry.source.chars())
                        .filter(|character| !character.is_whitespace())))
    }

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

pub(super) fn history_segments(app: &crate::XRTranslateApp) -> Vec<HistorySegment<'_>> {
    collect_history_segments(
        &app.recognition_history,
        &app.translations,
        &app.history_timeline,
    )
}

fn collect_history_segments<'a>(
    recognition: &'a [crate::history::RecognitionHistoryEntry],
    translations: &'a [crate::history::TranslationHistoryEntry],
    timeline: &crate::history::FeedTimeline,
) -> Vec<HistorySegment<'a>> {
    let mut turns: Vec<HistorySegment<'a>> = Vec::new();
    for entry in recognition {
        if entry.text.trim().is_empty() {
            continue;
        }
        if let Some(turn) = turns.iter_mut().find(|turn| {
            !entry.turn_id.is_empty()
                && turn.stream_id == entry.stream_id
                && turn.turn_id == entry.turn_id
                && turn.segment_index == entry.segment_index
        }) {
            turn.recognition.push(entry);
        } else {
            turns.push(HistorySegment {
                stream_id: entry.stream_id,
                turn_id: &entry.turn_id,
                segment_index: entry.segment_index,
                recognition: vec![entry],
                translations: Vec::new(),
            });
        }
    }
    for entry in translations {
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
                            && (turn.segment_index == entry.segment_index
                                || (turn.segment_index == 0 && !turn.recognition.is_empty()))
                    }
            })
            .unwrap_or_else(|| {
                turns.push(HistorySegment {
                    stream_id: entry.stream_id,
                    turn_id: &entry.turn_id,
                    segment_index: entry.segment_index,
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
    turns.sort_by_key(|turn| {
        (
            timeline.position(turn.stream_id, turn.turn_id),
            turn.segment_index,
        )
    });
    turns
}

fn prepare_turn<'a>(
    ui: &egui::Ui,
    turn: &HistorySegment<'a>,
    scale: &HistoryFeedScale,
    id: egui::Id,
) -> (f32, HistoryRow<'a>) {
    let has_output = turn.has_output()
        && (!turn.translations.iter().all(|entry| entry.asr_only)
            || turn.asr_output_replaces_source());
    let has_source = !turn.recognition.is_empty()
        || turn
            .translations
            .iter()
            .any(|entry| !entry.source.trim().is_empty());
    let show_source = has_source && !turn.asr_output_replaces_source();
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
        prepare_history_row(ui, scale, source, has_output.then_some(output), id)
    } else {
        prepare_history_row(ui, scale, output, None, id)
    }
}

fn prepare_history_row<'a>(
    ui: &egui::Ui,
    scale: &HistoryFeedScale,
    text: AnnotatedText<'a>,
    translation: Option<AnnotatedText<'a>>,
    id: egui::Id,
) -> (f32, HistoryRow<'a>) {
    let margin = history_card_frame(ui.ctx(), scale).total_margin().sum();
    let width = (ui.available_width() - margin.x).max(1.0);
    let row = HistoryRow {
        id,
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

fn render_feed(app: &crate::XRTranslateApp, ui: &mut egui::Ui, scale: &HistoryFeedScale) {
    let turns = history_segments(app);
    let stamp = app.history_timeline.revision;
    let id = ui.make_persistent_id("history_scroll_state");
    let changed = ui.data(|data| data.get_temp::<u64>(id)) != Some(stamp);
    let count = turns.len() + usize::from(!app.partial_text.is_empty());
    if count == 0 {
        ui.weak(crate::i18n::tr(app.ui_language, "No results yet"));
        return;
    }
    crate::ui::virtual_list::show_variable_virtual_rows(
        ui,
        "history_scroll",
        scale.row_gap,
        changed,
        Some(|row: &HistoryRow<'_>| row.id),
        |ui| {
            let mut rows = turns
                .iter()
                .enumerate()
                .map(|(index, turn)| {
                    let id = ui.make_persistent_id((
                        "history_data",
                        turn.stream_id,
                        turn.turn_id,
                        turn.segment_index,
                        if turn.turn_id.is_empty() { index } else { 0 },
                    ));
                    prepare_turn(ui, turn, scale, id)
                })
                .collect::<Vec<_>>();
            if !app.partial_text.is_empty() {
                let mut text = AnnotatedText::default();
                text.append(
                    ui,
                    egui::RichText::new(&app.partial_text)
                        .color(crate::ui::theme::primary_dark())
                        .size(scale.text_size),
                );
                rows.push(prepare_history_row(
                    ui,
                    scale,
                    text,
                    None,
                    ui.make_persistent_id("history_partial"),
                ));
            }
            rows
        },
        |ui, index, height, row| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let turn = turns.get(index);
            let activity = if turn.is_none_or(HistorySegment::live) {
                1.0
            } else {
                history_activity(index, count)
            };
            history_bubble(ui, row, activity, height, scale, app.ui_language, || {
                turn.map_or_else(|| app.partial_text.trim().to_owned(), text::segment_text)
            });
        },
    );
    ui.data_mut(|data| data.insert_temp(id, stamp));
}

fn render_header(
    app: &mut crate::XRTranslateApp,
    ui: &mut egui::Ui,
    header_size: f32,
    openable: bool,
) -> bool {
    let count = history_segments(app).len() + usize::from(!app.partial_text.is_empty());
    let previous = app.history_display.mode;
    let mut toggle_fullscreen = false;
    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new(format!(
                "{} ({count})",
                crate::i18n::tr(app.ui_language, "History")
            ))
            .size(header_size)
            .color(crate::ui::theme::text_strong())
            .strong(),
        );
        ui.add_space((ui.available_rect_before_wrap().width() - 120.0).max(0.0));
        ui.horizontal(|ui| {
            toolbar_group(ui, |ui| {
                for (mode, label, icon) in [
                    (
                        DisplayMode::Bubbles,
                        if openable { "Conversation" } else { "Presentation mode" },
                        egui::include_image!("../../../../../crates/xrtranslate-client-core/resources/icons/messages.svg"),
                    ),
                    (
                        DisplayMode::Text,
                        "Bilingual text",
                        egui::include_image!("../../../../../crates/xrtranslate-client-core/resources/icons/text-lines.svg"),
                    ),
                    (
                        DisplayMode::Split,
                        "Split view",
                        egui::include_image!("../../../../../crates/xrtranslate-client-core/resources/icons/panels-top-bottom.svg"),
                    ),
                ] {
                    if toolbar_button(
                        ui,
                        crate::i18n::tr(app.ui_language, label),
                        icon,
                        app.history_display.mode == mode,
                    ).clicked() {
                        app.history_display.mode = mode;
                    }
                }
            });
            toggle_fullscreen = toolbar_button(
                ui,
                crate::i18n::tr(
                    app.ui_language,
                    if openable { "Click to view full screen" } else { "Back" },
                ),
                if openable {
                    egui::include_image!("../../../../../crates/xrtranslate-client-core/resources/icons/media-expand.svg")
                } else {
                    egui::include_image!("../../../../../crates/xrtranslate-client-core/resources/icons/media-collapse.svg")
                },
                false,
            ).clicked();
        });
    });
    if app.history_display.mode != previous {
        app.history_selection.clear();
        if !openable && app.history_display.mode == DisplayMode::Bubbles {
            app.history_presentation.enter();
        }
        app.save_settings();
    }
    toggle_fullscreen
}

pub(super) fn toolbar_group(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui)) {
    let color = ui.visuals().text_color();
    egui::Frame::new()
        .fill(color.gamma_multiply(0.035))
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.12)))
        .corner_radius(7)
        .inner_margin(2)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(contents);
        });
}

pub(super) fn toolbar_button(
    ui: &mut egui::Ui,
    label: &str,
    icon: egui::ImageSource<'_>,
    selected: bool,
) -> egui::Response {
    ui.push_id(label, |ui| {
        ui.spacing_mut().button_padding = egui::Vec2::ZERO;
        ui.spacing_mut().interact_size = egui::Vec2::splat(24.0);
        let response = ui.add_sized(egui::Vec2::splat(24.0), egui::Button::new("").frame(false));
        let response = ui_director::egui::Control::button(label).response(ui, response);
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), selected, label)
        });
        let color = if selected {
            ui.visuals().hyperlink_color
        } else {
            ui.visuals().text_color()
        };
        let hover = crate::ui::animation::AnimationSystem::hover(
            ui.ctx(),
            response.id.with("hover"),
            response.hovered() || response.has_focus(),
        );
        ui.painter().rect_filled(
            response.rect,
            5,
            color.gamma_multiply(if selected {
                0.12 + hover * 0.05
            } else {
                hover * 0.07
            }),
        );
        if response.has_focus() {
            ui.painter().rect_stroke(
                response.rect,
                5,
                egui::Stroke::new(1.0, color.gamma_multiply(0.6)),
                egui::StrokeKind::Inside,
            );
        }
        egui::Image::new(icon).tint(color).paint_at(
            ui,
            egui::Rect::from_center_size(response.rect.center(), egui::Vec2::splat(16.0)),
        );
        response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(label)
    })
    .inner
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
            fullscreen = render_header(app, ui, scale.header_size, true);
            ui.add_space(6.0);
            egui::Frame::new()
                .fill(crate::ui::theme::history_viewport())
                .corner_radius(scale.card_radius)
                .inner_margin(egui::Margin::symmetric(
                    scale.card_margin_x,
                    scale.card_margin_y,
                ))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.set_height((height - 8.0).max(0.0));
                    if app.history_display.mode == DisplayMode::Bubbles {
                        render_feed(app, ui, &scale)
                    } else {
                        text::render(app, ui, scale.text_size.max(14.0));
                    }
                });
        });
    if fullscreen {
        app.fullscreen_history = true;
        app.history_presentation.enter();
        ui.ctx().request_repaint();
    }
}

pub(super) fn render_fullscreen_history(app: &mut crate::XRTranslateApp, ui: &mut egui::Ui) {
    if exit_on_escape(app, ui.ctx()) {
        app.fullscreen_history = false;
        return;
    }
    ui.push_id("fullscreen_history", |ui| {
        if render_header(app, ui, 18.0, false) {
            app.fullscreen_history = false;
            return;
        }
        if app.history_display.mode == DisplayMode::Bubbles {
            ui.ctx().request_repaint();
            return;
        }
        ui.add_space(10.0);
        text::render(app, ui, 16.0);
    });
}

pub(super) fn exit_on_escape(app: &mut crate::XRTranslateApp, ctx: &egui::Context) -> bool {
    if egui::Popup::is_any_open(ctx) {
        return false;
    }
    if !ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        return false;
    }
    #[cfg(any(windows, target_os = "linux"))]
    if app.is_history_lookup_open() {
        app.close_history_lookup();
        return false;
    }
    if app.history_selection.is_active() {
        app.history_selection.clear();
        return false;
    }
    true
}

pub(super) fn show_lookup(app: &mut crate::XRTranslateApp, ctx: &egui::Context) {
    #[cfg(any(windows, target_os = "linux"))]
    {
        if let Some(selection) = app.history_selection.show_action(ctx, app.ui_language) {
            app.begin_history_lookup(
                selection.text,
                selection.context,
                selection.context_offset,
                selection.anchor,
                ctx,
            );
        }
        app.show_history_lookup(ctx);
    }
}
