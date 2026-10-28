use crate::{
    plugins::media::{
        controller::{MediaController, SubtitleField},
        i18n::tr,
        subtitles::SubtitleCue,
    },
    ui::{
        components,
        virtual_list::{self, MeasuredRow, Measurements, ScrollTarget, SubtitleMerge},
    },
};
use eframe::egui;

const SINGLE_LINE_HEADER_WIDTH: f32 = 360.0;

pub(super) fn render_subtitles_card(
    controller: &mut MediaController,
    language: crate::i18n::UiLanguage,
    ui: &mut egui::Ui,
) {
    let can_play = controller.can_play();
    let current = can_play
        .then(|| {
            controller
                .subtitles
                .active_cue_at(controller.get_time_ms())
                .map(|cue| cue.id.clone())
        })
        .flatten();
    if ui.rect_contains_pointer(ui.max_rect())
        && ui.input(|input| {
            input.smooth_scroll_delta.y.abs() > 0.1 || input.pointer.is_decidedly_dragging()
        })
    {
        controller.last_manual_scroll = Some(std::time::Instant::now());
    }
    let viewport = (
        ui.available_width().to_bits(),
        ui.available_height().to_bits(),
        ui.ctx().pixels_per_point().to_bits(),
    );
    let resized = ui.ctx().data_mut(|data| {
        let id = ui.make_persistent_id("subtitle_viewport");
        let previous = data.get_temp::<(u32, u32, u32)>(id);
        data.insert_temp(id, viewport);
        previous != Some(viewport)
    });
    if resized {
        controller.last_auto_scrolled_cue_id = None;
    }
    let follow = can_play
        && controller.subtitle_edit.is_none()
        && controller
            .last_manual_scroll
            .is_none_or(|time| time.elapsed().as_secs() >= 5);
    let mut target = ScrollTarget::None;
    if follow && current != controller.last_auto_scrolled_cue_id {
        if let Some(index) = controller
            .subtitles
            .cues()
            .iter()
            .position(|cue| Some(&cue.id) == current.as_ref())
        {
            target = ScrollTarget::Row(index);
        }
        controller.last_auto_scrolled_cue_id = current.clone();
    }
    ui.label(egui::RichText::new(tr(language, "Subtitles")).strong());
    let measurements_id = egui::Id::unique("media_subtitle_measurements");
    if controller.subtitles.count() == 0 {
        ui.data_mut(|data| data.remove::<Measurements<usize>>(measurements_id));
        egui::scroll_area::State::default().store(
            ui.ctx(),
            ui.make_persistent_id(egui::IdSalt::new("media_subtitles")),
        );
        ui.weak(tr(language, "No subtitles yet"));
        return;
    }
    if let Some(index) = super::timeline::overview(controller, ui) {
        let cue = &controller.subtitles.cues()[index];
        let (id, start) = (cue.id.clone(), cue.start_ms);
        if can_play {
            controller.seek_to(start, false);
        }
        controller.last_manual_scroll = Some(std::time::Instant::now());
        controller.last_auto_scrolled_cue_id = Some(id);
        if controller.subtitle_edit.is_none() {
            target = ScrollTarget::Row(index);
        }
    }
    let idle = controller
        .active_task_id
        .as_deref()
        .and_then(|id| controller.store.get(id))
        .is_some_and(|task| !task.is_task_running);
    let editing_index = controller.subtitle_edit.as_ref().and_then(|draft| {
        controller
            .subtitles
            .cues()
            .iter()
            .position(|cue| cue.id == draft.cue.id)
    });
    let edit_bounds = editing_index.map(|index| controller.subtitle_time_bounds(index));
    let edit_height = ui.available_height().clamp(180.0, 300.0);
    if let Some(index) = editing_index
        && controller
            .subtitle_edit
            .as_ref()
            .is_some_and(|draft| draft.focus.is_some())
    {
        target = ScrollTarget::Row(index);
    }
    let can_edit = idle && editing_index.is_none();
    let edit_hint = tr(
        language,
        if !idle {
            "Pause translation to edit."
        } else if !can_edit {
            "Save or cancel this edit first."
        } else {
            "Double-click to edit."
        },
    );
    let mut seek = None;
    let mut edit = None;
    let mut delete = None;
    let mut edit_action = super::subtitle_edit::EditAction::None;
    let subtitles = &controller.subtitles;
    let task_id = controller.active_task_id.as_deref();
    let draft = &mut controller.subtitle_edit;
    let revision = egui::IdSalt::new((
        task_id,
        subtitles.revision(),
        editing_index,
        edit_height.to_bits(),
    ));
    let mut measurements = ui
        .data_mut(|data| data.remove_temp::<Measurements<usize>>(measurements_id))
        .unwrap_or_default();
    let merge = virtual_list::show_subtitle_rows(
        ui,
        "media_subtitles",
        8.0,
        target,
        |row: &MeasuredRow<usize>| row.id,
        SubtitleMerge {
            enabled: can_edit,
            language,
            index: &|row: &MeasuredRow<usize>| Some(row.entry),
            text: &|index| subtitles.cues()[index].original_text.as_str(),
        },
        |ui| {
            measurements.prepare(
                ui,
                revision,
                language,
                || {
                    subtitles
                        .cues()
                        .iter()
                        .enumerate()
                        .map(|(index, cue)| MeasuredRow {
                            id: measurements_id.with((task_id, &cue.id)),
                            signature: egui::IdSalt::new((
                                &cue.original_text,
                                &cue.translated_text,
                                (editing_index == Some(index)).then_some(edit_height.to_bits()),
                            )),
                            entry: index,
                        })
                        .collect()
                },
                |row| {
                    if editing_index == Some(row.entry) {
                        edit_height
                    } else {
                        prepare_cue_text(ui, &subtitles.cues()[row.entry]).0
                    }
                },
            )
        },
        |ui, _, _, row| {
            let index = row.entry;
            let cue = &subtitles.cues()[index];
            let text = (editing_index != Some(index)).then(|| prepare_cue_text(ui, cue).1);
            components::card(ui, |ui| {
                ui.set_width(ui.available_width());
                if editing_index == Some(index) {
                    edit_action = super::subtitle_edit::render(
                        draft.as_mut().unwrap(),
                        edit_bounds.clone().unwrap(),
                        language,
                        edit_height,
                        ui,
                    );
                    return;
                }
                let wrap_actions = ui.available_width() < SINGLE_LINE_HEADER_WIDTH;
                crate::ui::layout::flow_row(ui, |ui| {
                    if can_play
                        && components::icon_button(
                            ui,
                            &format!(
                                "{} · {}",
                                tr(language, "Play from here"),
                                super::format_cue_time(cue.start_ms)
                            ),
                            egui::include_image!("../../../../resources/icons/play.svg"),
                        )
                        .clicked()
                    {
                        seek = Some(cue.start_ms);
                    }
                    for (field, time) in [
                        (SubtitleField::Start, cue.start_ms),
                        (SubtitleField::End, subtitles.effective_end_at(index)),
                    ] {
                        if field == SubtitleField::End {
                            ui.weak("–");
                        }
                        let color = if current.as_deref() == Some(&cue.id) {
                            crate::ui::theme::primary()
                        } else {
                            crate::ui::theme::text_weak()
                        };
                        let response = ui
                            .add(
                                egui::Label::new(
                                    egui::RichText::new(super::format_cue_time(time)).color(color),
                                )
                                .selectable(!can_edit)
                                .sense(egui::Sense::click()),
                            )
                            .on_hover_text(edit_hint);
                        if can_edit && response.double_clicked() {
                            edit = Some((cue.id.clone(), field));
                        }
                    }
                    if wrap_actions {
                        ui.end_row();
                    }
                    if components::copy_button(ui, crate::i18n::tr(language, "Copy")).clicked() {
                        ui.ctx().copy_text(
                            cue.translated_text
                                .as_ref()
                                .filter(|text| !text.trim().is_empty())
                                .unwrap_or(&cue.original_text)
                                .clone(),
                        );
                    }
                    let button = ui
                        .add_enabled_ui(can_edit, |ui| {
                            components::edit_button(ui, tr(language, "Edit subtitle"))
                        })
                        .inner
                        .on_disabled_hover_text(edit_hint);
                    if button.clicked() {
                        edit = Some((cue.id.clone(), SubtitleField::Original));
                    }
                    let button = ui
                        .add_enabled_ui(can_edit, |ui| {
                            components::delete_button(ui, tr(language, "Delete subtitle"))
                        })
                        .inner
                        .on_disabled_hover_text(edit_hint);
                    egui::Popup::menu(&button).show(|ui| {
                        ui.label(tr(language, "Delete this subtitle?"));
                        ui.horizontal(|ui| {
                            if components::secondary_button(ui, tr(language, "Delete")).clicked() {
                                delete = Some(cue.id.clone());
                                ui.close();
                            }
                            if components::secondary_button(ui, tr(language, "Cancel")).clicked() {
                                ui.close();
                            }
                        });
                    });
                });
                let text = text.as_ref().unwrap();
                ui.scope(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for (field, galley) in [
                        (SubtitleField::Original, Some(&text.original)),
                        (SubtitleField::Translation, text.translated.as_ref()),
                    ] {
                        let Some(galley) = galley else {
                            continue;
                        };
                        let response = ui
                            .add(
                                egui::Label::new(galley.clone())
                                    .selectable(!can_edit)
                                    .sense(egui::Sense::click()),
                            )
                            .on_hover_text(edit_hint);
                        if can_edit && response.double_clicked() {
                            edit = Some((cue.id.clone(), field));
                        }
                    }
                });
            });
        },
    );
    ui.data_mut(|data| data.insert_temp(measurements_id, measurements));
    if let Some(id) = delete {
        controller.delete_subtitle(&id);
        return;
    }
    if let Some((source, target)) = merge
        && let (Some(source), Some(target)) = (
            controller.subtitles.cues().get(source),
            controller.subtitles.cues().get(target),
        )
    {
        let (source, target) = (source.id.clone(), target.id.clone());
        controller.merge_subtitles(&source, &target);
    }
    if let Some(time) = seek {
        controller.seek_to(time, true);
    }
    if let Some((id, field)) = edit {
        controller.begin_subtitle_edit(&id, field);
    }
    match edit_action {
        super::subtitle_edit::EditAction::Retranslate => {
            if let Some(draft) = &mut controller.subtitle_edit {
                draft.retranslate_requested = true;
            }
        }
        super::subtitle_edit::EditAction::Save => controller.save_subtitle_edit(),
        super::subtitle_edit::EditAction::Cancel => controller.subtitle_edit = None,
        super::subtitle_edit::EditAction::None => {}
    }
}

fn prepare_cue_text(ui: &egui::Ui, cue: &SubtitleCue) -> (f32, CueText) {
    let margins = components::card_margins(ui.ctx());
    let layout = |text: &str| {
        ui.painter().layout(
            text.to_owned(),
            egui::FontId::proportional(14.0),
            crate::ui::theme::text_strong(),
            (ui.available_width() - margins.x).max(20.0),
        )
    };
    let original = layout(&cue.original_text);
    let translated = cue
        .translated_text
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty() && *text != cue.original_text.trim())
        .map(layout);
    let header_height = ui.spacing().interact_size.y.max(32.0);
    let wrapped_actions = if ui.available_width() - margins.x < SINGLE_LINE_HEADER_WIDTH {
        header_height + ui.spacing().item_spacing.y
    } else {
        0.0
    };
    let height = original.size().y
        + translated.as_ref().map_or(0.0, |text| text.size().y + 2.0)
        + margins.y
        + header_height
        + wrapped_actions
        + ui.spacing().item_spacing.y;
    (
        height,
        CueText {
            original,
            translated,
        },
    )
}

struct CueText {
    original: std::sync::Arc<egui::Galley>,
    translated: Option<std::sync::Arc<egui::Galley>>,
}
