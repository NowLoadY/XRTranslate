use crate::{
    plugins::media::{controller::MediaController, i18n::tr},
    ui::{components, layout::ScrollTarget},
};
use eframe::egui;

pub(super) fn render_subtitles_card(
    controller: &mut MediaController,
    language: crate::i18n::UiLanguage,
    ui: &mut egui::Ui,
) {
    ui.add_space(12.0);
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
        let id = ui.id().with("subtitle_viewport");
        let previous = data.get_temp::<(u32, u32, u32)>(id);
        data.insert_temp(id, viewport);
        previous != Some(viewport)
    });
    if resized {
        controller.last_auto_scrolled_cue_id = None;
    }
    let follow = can_play
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
    if controller.subtitles.count() == 0 {
        ui.weak(tr(language, "No subtitles yet"));
        return;
    }
    let mut seek = None;
    crate::ui::layout::show_variable_virtual_rows(
        ui,
        "media_subtitles",
        8.0,
        target,
        |ui| {
            let margins = components::card_margins(ui.ctx());
            let key = egui::Id::new((
                controller.active_task_id.as_deref(),
                controller.subtitles.revision(),
                ui.available_width().to_bits(),
                ui.ctx().pixels_per_point().to_bits(),
                language,
                crate::ui::theme::text_strong(),
                margins.x.to_bits(),
                margins.y.to_bits(),
            ));
            let cache_id = egui::Id::new("media_subtitle_layout");
            let cached = ui.ctx().data(|data| data.get_temp::<TextRows>(cache_id));
            if let Some(cached) = cached.filter(|cache| cache.key == key) {
                return cached.rows;
            }
            let rows: std::sync::Arc<[(f32, std::sync::Arc<egui::Galley>)]> = controller
                .subtitles
                .cues()
                .iter()
                .map(|cue| {
                    let mut text = cue.original_text.clone();
                    if let Some(translated) = &cue.translated_text {
                        text.push('\n');
                        text.push_str(translated);
                    }
                    let galley = ui.painter().layout(
                        text,
                        egui::FontId::proportional(14.0),
                        crate::ui::theme::text_strong(),
                        (ui.available_width() - margins.x).max(20.0),
                    );
                    (
                        galley.size().y
                            + margins.y
                            + ui.spacing().interact_size.y.max(32.0)
                            + ui.spacing().item_spacing.y,
                        galley,
                    )
                })
                .collect();
            ui.ctx().data_mut(|data| {
                data.insert_temp(
                    cache_id,
                    TextRows {
                        key,
                        rows: rows.clone(),
                    },
                )
            });
            rows
        },
        |ui, index, _, galley| {
            let cue = &controller.subtitles.cues()[index];
            let row_start = ui.cursor().min;
            let mut copied = false;
            let mut copy_rect = egui::Rect::NOTHING;
            components::card(ui, |ui| {
                ui.set_width(ui.available_width());
                crate::ui::layout::flow_row(ui, |ui| {
                    let time = super::format_time_ms(cue.start_ms);
                    if can_play {
                        if ui
                            .selectable_label(current.as_deref() == Some(cue.id.as_str()), time)
                            .clicked()
                        {
                            seek = Some(cue.start_ms);
                        }
                    } else {
                        ui.weak(time);
                    }
                    let copy = components::secondary_button(ui, crate::i18n::tr(language, "Copy"));
                    copy_rect = copy.rect;
                    if copy.clicked() {
                        copied = true;
                        ui.ctx().copy_text(
                            cue.translated_text
                                .as_ref()
                                .unwrap_or(&cue.original_text)
                                .clone(),
                        );
                    }
                });
                let text = ui.add(egui::Label::new(galley.clone()).selectable(true));
                if can_play && text.clicked() {
                    seek = Some(cue.start_ms);
                }
            });
            if can_play {
                let rect = egui::Rect::from_min_max(row_start, ui.min_rect().max);
                // Keep child controls and text selection in charge of their input.
                let response = ui.interact(rect, ui.id().with("play_cue"), egui::Sense::hover());
                let label = format!(
                    "{} · {}",
                    tr(language, "Play from here"),
                    super::format_time_ms(cue.start_ms)
                );
                let response =
                    crate::ui::automation::button_response(ui, response.id, &label, true, response)
                        .on_hover_text(tr(language, "Play from here"));
                let clicked = ui.rect_contains_pointer(rect)
                    && ui.input(|input| {
                        input.pointer.button_clicked(egui::PointerButton::Primary)
                            && input
                                .pointer
                                .interact_pos()
                                .is_some_and(|pos| !copy_rect.contains(pos))
                    });
                if (response.clicked() || clicked) && !copied {
                    seek = Some(cue.start_ms);
                }
            }
        },
    );
    if let Some(time) = seek {
        controller.seek_to(time, true);
    }
}

#[derive(Clone)]
struct TextRows {
    key: egui::Id,
    rows: std::sync::Arc<[(f32, std::sync::Arc<egui::Galley>)]>,
}
