//! Continuous, copyable documents projected from the same timeline as the conversation.
use super::{DisplayMode, HistorySegment};
use crate::{
    i18n::{UiLanguage, tr},
    ui::theme,
};
use eframe::egui;
use std::sync::Arc;

#[derive(Default)]
struct Transcript {
    original: String,
    translation: String,
    bilingual: String,
}

impl Transcript {
    fn from_segments(segments: &[HistorySegment<'_>], partial: &str) -> Self {
        let mut document = Self::default();
        for segment in segments {
            let original = if segment.asr_output_replaces_source() {
                join_segments(
                    segment
                        .translations
                        .iter()
                        .map(|entry| entry.translated.as_str()),
                )
            } else if segment.recognition.is_empty() {
                join_segments(
                    segment
                        .translations
                        .iter()
                        .map(|entry| entry.source.as_str()),
                )
            } else {
                join_segments(segment.recognition.iter().map(|entry| entry.text.as_str()))
            };
            let translations = segment.translations.iter().filter(|entry| !entry.asr_only);
            let mut translation =
                join_segments(translations.clone().map(|entry| entry.translated.as_str()));
            let mut languages = Vec::new();
            for extra in translations
                .clone()
                .flat_map(|entry| &entry.additional_translations)
            {
                if !extra.translated_text.trim().is_empty()
                    && !languages.contains(&extra.target_lang.as_str())
                {
                    languages.push(extra.target_lang.as_str());
                }
            }
            for language in languages {
                let extra = join_segments(
                    translations
                        .clone()
                        .flat_map(|entry| &entry.additional_translations)
                        .filter(|extra| extra.target_lang == language)
                        .map(|extra| extra.translated_text.as_str()),
                );
                append_block(&mut translation, &extra, "\n");
            }
            append_block(&mut document.original, &original, "\n");
            append_block(&mut document.translation, &translation, "\n");
            let mut pair = original;
            append_block(&mut pair, &translation, "\n");
            append_block(&mut document.bilingual, &pair, "\n\n");
        }
        append_block(&mut document.original, partial, "\n");
        append_block(&mut document.bilingual, partial, "\n\n");
        document
    }
}

pub(super) fn segment_text(segment: &HistorySegment<'_>) -> String {
    Transcript::from_segments(std::slice::from_ref(segment), "").bilingual
}

fn join_segments<'a>(parts: impl Iterator<Item = &'a str>) -> String {
    let mut text = String::new();
    for part in parts {
        crate::streaming::append_segment(&mut text, part);
    }
    text
}

fn append_block(text: &mut String, block: &str, separator: &str) {
    let block = block.trim();
    if !block.is_empty() {
        if !text.is_empty() {
            text.push_str(separator);
        }
        text.push_str(block);
    }
}

pub(super) fn render(app: &crate::XRTranslateApp, ui: &mut egui::Ui, size: f32) {
    let document = Transcript::from_segments(&super::history_segments(app), &app.partial_text);
    render_document(
        ui,
        &document,
        app.history_display.mode == DisplayMode::Split,
        app.ui_language,
        size,
    );
}

fn render_document(
    ui: &mut egui::Ui,
    document: &Transcript,
    split: bool,
    language: UiLanguage,
    size: f32,
) {
    let (rect, _) = ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
    if !split {
        render_pane(
            ui,
            rect,
            &document.bilingual,
            Pane::Bilingual,
            language,
            size,
        );
        return;
    }

    let id = ui.make_persistent_id("text_split");
    let mut fraction = ui.data(|data| data.get_temp::<f32>(id)).unwrap_or(0.5);
    let height = (rect.height() - 10.0).max(0.0);
    let divider_y = rect.top() + height * fraction;
    let divider = egui::Rect::from_min_size(
        egui::pos2(rect.left(), divider_y),
        egui::vec2(rect.width(), 10.0),
    );
    let response = ui
        .interact(divider, id, egui::Sense::drag())
        .on_hover_cursor(egui::CursorIcon::ResizeVertical);
    if response.dragged()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        fraction = ((pointer.y - rect.top() - 5.0) / height.max(1.0)).clamp(0.25, 0.75);
        ui.data_mut(|data| data.insert_temp(id, fraction));
    }
    let divider_y = rect.top() + height * fraction;
    let color = if response.hovered() || response.dragged() {
        theme::primary()
    } else {
        theme::border()
    };
    ui.painter().hline(
        rect.x_range(),
        divider_y + 5.0,
        egui::Stroke::new(1.0, theme::border()),
    );
    ui.painter().line_segment(
        [
            egui::pos2(rect.center().x - 12.0, divider_y + 5.0),
            egui::pos2(rect.center().x + 12.0, divider_y + 5.0),
        ],
        egui::Stroke::new(3.0, color),
    );
    render_pane(
        ui,
        egui::Rect::from_min_max(rect.min, egui::pos2(rect.right(), divider_y)),
        &document.original,
        Pane::Original,
        language,
        size,
    );
    render_pane(
        ui,
        egui::Rect::from_min_max(egui::pos2(rect.left(), divider_y + 10.0), rect.max),
        &document.translation,
        Pane::Translation,
        language,
        size,
    );
}

#[derive(Clone, Copy, Debug, Hash)]
enum Pane {
    Bilingual,
    Original,
    Translation,
}

impl Pane {
    fn title(self) -> &'static str {
        match self {
            Self::Bilingual => "",
            Self::Original => "Original",
            Self::Translation => "Translation",
        }
    }
}

fn render_pane(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    text: &str,
    pane: Pane,
    language: UiLanguage,
    size: f32,
) {
    ui.scope_builder(egui::UiBuilder::new().id_salt(egui::IdSalt::new(pane)).max_rect(rect), |ui| {
        ui.set_clip_rect(ui.clip_rect().intersect(rect));
        let id = ui.make_persistent_id("text");
        let snapshot_id = id.with("snapshot");
        let focused = ui.memory(|memory| memory.has_focus(id));
        let mut paused = focused && (ui.input(|input| input.pointer.primary_down())
            || egui::TextEdit::load_state(ui.ctx(), id)
                .and_then(|state| state.cursor.char_range())
                .is_some_and(|range| !range.is_empty()));
        let previous = ui.data(|data| data.get_temp::<Arc<str>>(snapshot_id));
        let mut snapshot = previous.filter(|previous| paused || previous.as_ref() == text)
            .unwrap_or_else(|| Arc::from(text));
        let away = ui.data(|data| data.get_temp::<bool>(id.with("away"))).unwrap_or(false);
        let mut follow = false;
        ui.horizontal(|ui| {
            // Keep the text stationary when the follow control appears during selection.
            ui.set_height(24.0);
            if !pane.title().is_empty() {
                ui.weak(tr(language, pane.title()));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if paused || away {
                    follow = super::toolbar_button(ui, tr(language, "Follow latest"),
                        egui::include_image!("../../../../../../crates/xrtranslate-client-core/resources/icons/chevron-down.svg"), false)
                        .clicked();
                }
            });
        });
        if follow {
            paused = false;
            snapshot = Arc::from(text);
            ui.memory_mut(|memory| memory.surrender_focus(id));
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) {
                state.cursor.set_char_range(None);
                state.store(ui.ctx(), id);
            }
        }
        let output = egui::ScrollArea::vertical()
            .id_salt("scroll")
            .max_height(ui.available_height().max(0.0))
            .min_scrolled_height(0.0)
            .auto_shrink([false, false])
            .scroll_source(egui::scroll_area::ScrollSource::SCROLL_BAR | egui::scroll_area::ScrollSource::MOUSE_WHEEL)
            .stick_to_bottom(!paused)
            .show(ui, |ui| {
            let mut content = snapshot.as_ref();
            let response = egui::TextEdit::multiline(&mut content)
                .id(id)
                .font(egui::FontId::proportional(size))
                .text_color(theme::text_strong())
                .frame(egui::Frame::NONE)
                .margin(0)
                .desired_width(ui.available_width())
                .desired_rows(1)
                .hint_text(tr(language, "No results yet"))
                .show(ui).response;
            ui_director::egui::record_text_output(ui, &snapshot, &response);
            if follow {
                ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
            }
        });
        ui.data_mut(|data| {
            data.insert_temp(snapshot_id, snapshot);
            data.insert_temp(id.with("away"), output.state.clamped_offset().y + output.inner_rect.height() + 2.0 < output.content_size.y);
        });
    });
}

#[cfg(test)]
mod tests;
