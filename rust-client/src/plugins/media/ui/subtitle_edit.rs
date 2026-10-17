use crate::{
    i18n::UiLanguage,
    plugins::media::{
        controller::{SubtitleEdit, SubtitleField},
        i18n::tr,
    },
    ui::components,
};
use eframe::egui;

pub(super) enum EditAction {
    None,
    Save,
    Cancel,
}

pub(super) fn render(
    draft: &mut SubtitleEdit,
    bounds: std::ops::RangeInclusive<i64>,
    language: UiLanguage,
    height: f32,
    ui: &mut egui::Ui,
) -> EditAction {
    let id = egui::Id::new(("media_subtitle_edit", &draft.task_id, &draft.cue.id));
    let focus = draft.focus.take();
    let body_height = (height
        - components::card_margins(ui.ctx()).y
        - ui.spacing().interact_size.y.max(32.0)
        - ui.spacing().item_spacing.y)
        .max(60.0);
    egui::ScrollArea::vertical()
        .id_salt(id)
        .max_height(body_height)
        .min_scrolled_height(body_height)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            crate::ui::layout::flow_row(ui, |ui| {
                ui.label(tr(language, "Start time"));
                if time_input(
                    ui,
                    "subtitle_start",
                    &mut draft.cue.start_ms,
                    bounds.clone(),
                    focus == Some(SubtitleField::Start),
                ) {
                    draft.cue.end_ms = draft.cue.end_ms.clamp(draft.cue.start_ms, *bounds.end());
                }
                ui.label(tr(language, "End time"));
                time_input(
                    ui,
                    "subtitle_end",
                    &mut draft.cue.end_ms,
                    draft.cue.start_ms..=(*bounds.end()).max(draft.cue.start_ms),
                    focus == Some(SubtitleField::End),
                );
            });
            for (field, name, label, text) in [
                (
                    SubtitleField::Original,
                    "subtitle_original",
                    "Original",
                    &mut draft.cue.original_text,
                ),
                (
                    SubtitleField::Translation,
                    "subtitle_translation",
                    "Translation",
                    draft.cue.translated_text.get_or_insert_default(),
                ),
            ] {
                let label = ui.label(tr(language, label));
                let response = components::text_edit_ui(
                    ui,
                    name,
                    egui::TextEdit::multiline(text)
                        .id(id.with(name))
                        .desired_width(f32::INFINITY)
                        .desired_rows(3),
                )
                .labelled_by(label.id);
                if focus == Some(field) {
                    response.request_focus();
                    response.scroll_to_me(Some(egui::Align::Center));
                }
                if let Some(value) = crate::ui::automation::record_text_input(
                    ui,
                    response.id,
                    name,
                    text,
                    ui.is_enabled(),
                    response.rect,
                ) {
                    *text = value;
                }
            }
        });
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let save = ui
            .add_enabled_ui(!draft.cue.original_text.trim().is_empty(), |ui| {
                components::icon_button(
                    ui,
                    tr(language, "Save"),
                    egui::include_image!("../../../../resources/icons/check.svg"),
                )
            })
            .inner
            .clicked();
        let cancel = components::icon_button(
            ui,
            tr(language, "Cancel"),
            egui::include_image!("../../../../resources/icons/x.svg"),
        )
        .clicked();
        if save {
            EditAction::Save
        } else if cancel {
            EditAction::Cancel
        } else {
            EditAction::None
        }
    })
    .inner
}

fn time_input(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut i64,
    bounds: std::ops::RangeInclusive<i64>,
    focus: bool,
) -> bool {
    ui.push_id(label, |ui| {
        let response = ui.add(
            egui::DragValue::new(value)
                .speed(10.0)
                .range(bounds.clone())
                .clamp_existing_to_range(false)
                .custom_formatter(|value, _| super::format_cue_time(value.round() as i64))
                .custom_parser(parse_time),
        );
        if focus {
            response.request_focus();
            let mut state = egui::TextEdit::load_state(ui.ctx(), response.id).unwrap_or_default();
            state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::default(),
                egui::text::CCursor::new(super::format_cue_time(*value).chars().count()),
            )));
            state.store(ui.ctx(), response.id);
            response.scroll_to_me(Some(egui::Align::Center));
        }
        let automated = crate::ui::automation::record_slider(
            ui,
            response.id,
            label,
            *value as f64,
            ui.is_enabled(),
            response.rect,
        );
        if let Some(time) = automated {
            *value = (time.round() as i64).clamp(*bounds.start(), *bounds.end());
        }
        response.changed() || automated.is_some()
    })
    .inner
}

fn parse_time(text: &str) -> Option<f64> {
    let text = text.trim().replace(',', ".");
    let parts = text.split(':').collect::<Vec<_>>();
    if parts.len() > 3 {
        return None;
    }
    let mut seconds = 0.0;
    for (index, part) in parts.iter().enumerate() {
        let value = part.parse::<f64>().ok()?;
        if !value.is_finite()
            || value < 0.0
            || (index > 0 && value >= 60.0)
            || (index + 1 < parts.len() && value.fract() != 0.0)
        {
            return None;
        }
        seconds = seconds * 60.0 + value;
    }
    (seconds.is_finite()).then_some(seconds * 1000.0)
}
