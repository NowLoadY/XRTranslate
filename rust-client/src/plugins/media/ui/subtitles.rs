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
    ui.allocate_ui(
        egui::vec2(
            ui.available_width(),
            420.0_f32.min(ui.ctx().content_rect().height() * 0.6),
        ),
        |ui| {
            crate::ui::layout::show_variable_virtual_rows(
                ui,
                "media_subtitles",
                8.0,
                target,
                |ui| {
                    let key = egui::Id::new((
                        controller.active_task_id.as_deref(),
                        controller.subtitles.revision(),
                        ui.available_width().to_bits(),
                        ui.ctx().pixels_per_point().to_bits(),
                        language,
                        crate::ui::theme::text_strong(),
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
                                (ui.available_width() - 32.0).max(20.0),
                            );
                            (galley.size().y + 74.0, galley)
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
                    components::card(ui, |ui| {
                        ui.set_width(ui.available_width());
                        crate::ui::layout::flow_row(ui, |ui| {
                            let time = super::format_time_ms(cue.start_ms);
                            if can_play {
                                if ui
                                    .selectable_label(
                                        current.as_deref() == Some(cue.id.as_str()),
                                        time,
                                    )
                                    .clicked()
                                {
                                    seek = Some(cue.start_ms);
                                }
                            } else {
                                ui.weak(time);
                            }
                            if components::secondary_button(ui, crate::i18n::tr(language, "Copy"))
                                .clicked()
                            {
                                ui.ctx().copy_text(
                                    cue.translated_text
                                        .as_ref()
                                        .unwrap_or(&cue.original_text)
                                        .clone(),
                                );
                            }
                        });
                        ui.add(egui::Label::new(galley.clone()).selectable(true));
                    });
                },
            );
        },
    );
    if let Some(time) = seek
        && let Some(backend) = &mut controller.backend
    {
        backend.seek(time);
        controller.last_manual_scroll = None;
    }
}

#[derive(Clone)]
struct TextRows {
    key: egui::Id,
    rows: std::sync::Arc<[(f32, std::sync::Arc<egui::Galley>)]>,
}
