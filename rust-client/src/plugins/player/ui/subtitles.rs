use super::format_time_ms;
use crate::plugins::player::{controller::VideoPlayerController, i18n::tr, subtitles::SubtitleCue};
use crate::ui::components;
use eframe::egui::{self, Color32, CornerRadius, Frame, Margin, Stroke};

const CUE_ROW_HEIGHT: f32 = 88.0;
const CUE_ROW_GAP: f32 = 8.0;

fn timeline_padding(viewport_height: f32) -> f32 {
    ((viewport_height - CUE_ROW_HEIGHT) * 0.5).max(0.0)
}

fn timeline_content_height(cue_count: usize, viewport_height: f32) -> f32 {
    if cue_count == 0 {
        return viewport_height;
    }
    let rows = cue_count as f32 * CUE_ROW_HEIGHT;
    let gaps = cue_count.saturating_sub(1) as f32 * CUE_ROW_GAP;
    rows + gaps + timeline_padding(viewport_height) * 2.0
}

fn centered_cue_offset(index: usize) -> f32 {
    index as f32 * (CUE_ROW_HEIGHT + CUE_ROW_GAP)
}

fn visible_cue_range(
    viewport: egui::Rect,
    cue_count: usize,
    viewport_height: f32,
) -> std::ops::Range<usize> {
    let padding = timeline_padding(viewport_height);
    let extent = CUE_ROW_HEIGHT + CUE_ROW_GAP;
    let first = (((viewport.min.y - padding) / extent).floor() as isize - 1).max(0) as usize;
    let end = (((viewport.max.y - padding) / extent).ceil() as isize + 1).max(0) as usize;
    first.min(cue_count)..end.min(cue_count)
}

pub(super) fn render_subtitles_card(
    controller: &mut VideoPlayerController,
    language: crate::i18n::UiLanguage,
    ui: &mut egui::Ui,
) {
    if controller.fullscreen_mode {
        return;
    }

    ui.add_space(10.0);

    components::card(ui, |ui| {
        ui.vertical(|ui| {
            let current_time_ms = controller.get_time_ms();
            let now = std::time::Instant::now();

            let cues = controller.subtitles.cues();
            let cues_count = cues.len();

            let active_idx = if cues_count > 0 {
                let query_time = current_time_ms + 250;
                let idx = cues.partition_point(|cue| cue.start_ms <= query_time);
                if idx > 0 {
                    let candidate_idx = idx - 1;
                    let cue = &cues[candidate_idx];
                    let effective_end = if cue.end_ms <= cue.start_ms {
                        cue.start_ms + 3000
                    } else {
                        cue.end_ms.max(cue.start_ms + 2000)
                    };
                    if current_time_ms <= effective_end {
                        Some(candidate_idx)
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };

            let is_manually_scrolling = controller.last_manual_scroll.map_or(false, |instant| {
                instant.elapsed() < std::time::Duration::from_secs(5)
            });
            let auto_scroll_active = !is_manually_scrolling;

            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(tr(language, "Live Subtitles & Translation"))
                        .size(16.0)
                        .strong()
                        .color(crate::ui::theme::text_strong()),
                );
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(format!("{} {}", cues_count, tr(language, "Subtitles Count")))
                        .size(12.0)
                        .color(crate::ui::theme::text_weak()),
                );

                if is_manually_scrolling {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button(
                                egui::RichText::new("⤓ Auto-scroll")
                                    .size(12.0)
                                    .color(Color32::from_rgb(37, 99, 235)),
                            )
                            .clicked()
                        {
                            controller.last_manual_scroll = None;
                            controller.last_auto_scrolled_cue_id = None;
                        }
                    });
                }
            });

            ui.add_space(10.0);

            if cues.is_empty() {
                Frame::new()
                    .fill(Color32::from_rgb(248, 250, 252))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(241, 245, 249)))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(Margin::same(20))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.centered_and_justified(|ui| {
                            ui.label(
                                egui::RichText::new(tr(
                                    language,
                                    "No subtitle at current timestamp",
                                ))
                                .size(13.5)
                                .color(crate::ui::theme::text_weak()),
                            );
                        });
                    });
            } else {
                let mut seek_to_ms = None;
                let total_rows = cues.len();

                let mut scroll_area = egui::ScrollArea::vertical()
                    .id_salt("player_subtitles_timeline_scroll")
                    .min_scrolled_height(360.0)
                    .max_height(500.0)
                    .auto_shrink([false, false]);

                let viewport_height = controller
                    .timeline_viewport_height
                    .unwrap_or(450.0)
                    .clamp(360.0, 500.0);

                let active_cue_id = active_idx.map(|idx| cues[idx].id.as_str());
                if auto_scroll_active
                    && active_cue_id != controller.last_auto_scrolled_cue_id.as_deref()
                    && let Some(idx) = active_idx
                {
                    scroll_area = scroll_area.vertical_scroll_offset(centered_cue_offset(idx));
                    controller.last_auto_scrolled_cue_id = Some(cues[idx].id.clone());
                }

                let scroll_output = scroll_area.show_viewport(ui, |ui, viewport| {
                    let padding = timeline_padding(viewport_height);
                    let row_extent = CUE_ROW_HEIGHT + CUE_ROW_GAP;
                    let content_top = ui.max_rect().top();
                    let content_left = ui.max_rect().left();
                    let content_width = ui.available_width();
                    ui.set_height(timeline_content_height(total_rows, viewport_height));

                    for idx in visible_cue_range(viewport, total_rows, viewport_height) {
                        let cue = &cues[idx];
                        let row_top = content_top + padding + idx as f32 * row_extent;
                        let row_rect = egui::Rect::from_min_size(
                            egui::pos2(content_left, row_top),
                            egui::vec2(content_width, CUE_ROW_HEIGHT),
                        );
                        let response = ui
                            .scope_builder(
                                egui::UiBuilder::new()
                                    .id_salt(("player-subtitle-cue", &cue.id))
                                    .max_rect(row_rect),
                                |ui| render_cue_row(ui, cue, Some(idx) == active_idx),
                            )
                            .inner;
                        if response.clicked() {
                            seek_to_ms = Some(cue.start_ms);
                            controller.last_manual_scroll = None;
                            controller.last_auto_scrolled_cue_id = None;
                        }
                    }
                });

                controller.timeline_viewport_height = Some(scroll_output.inner_rect.height());

                let is_hovered = ui.rect_contains_pointer(scroll_output.inner_rect)
                    || ui.rect_contains_pointer(scroll_output.inner_rect.expand(16.0));
                let wheel_scrolled = is_hovered
                    && ui.input(|i| {
                        i.smooth_scroll_delta.y.abs() > 0.05
                            || i.smooth_scroll_delta.x.abs() > 0.05
                            || i.raw
                                .events
                                .iter()
                                .any(|e| matches!(e, egui::Event::MouseWheel { .. }))
                    });
                let is_dragged = is_hovered && ui.input(|i| i.pointer.is_decidedly_dragging());

                if wheel_scrolled || is_dragged {
                    controller.last_manual_scroll = Some(now);
                    // Recenter the same active cue after the manual-scroll
                    // grace period expires.
                    controller.last_auto_scrolled_cue_id = None;
                }

                if let Some(ms) = seek_to_ms {
                    if let Some(backend) = &mut controller.backend {
                        backend.seek(ms);
                    }
                }
            }
        });
    });
}

fn render_cue_row(ui: &mut egui::Ui, cue: &SubtitleCue, is_current: bool) -> egui::Response {
    let bg_color = if is_current {
        Color32::from_rgb(239, 246, 255)
    } else {
        Color32::from_rgb(248, 250, 252)
    };
    let stroke = if is_current {
        Stroke::new(1.5, Color32::from_rgb(96, 165, 250))
    } else {
        Stroke::new(1.0, Color32::from_rgb(241, 245, 249))
    };

    Frame::new()
        .fill(bg_color)
        .stroke(stroke)
        .corner_radius(CornerRadius::same(if is_current { 10 } else { 8 }))
        .inner_margin(Margin::symmetric(14, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_height(CUE_ROW_HEIGHT - 16.0);
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "[{} - {}]",
                            format_time_ms(cue.start_ms),
                            format_time_ms(cue.end_ms.max(cue.start_ms + 2000))
                        ))
                        .size(11.5)
                        .monospace()
                        .color(if is_current {
                            Color32::from_rgb(37, 99, 235)
                        } else {
                            Color32::from_rgb(59, 130, 246)
                        })
                        .strong(),
                    );

                    if let Some(speaker) = &cue.speaker_name {
                        ui.add_space(6.0);
                        components::speaker_badge(ui, speaker);
                    }
                });

                ui.add_space(2.0);
                ui.add(
                    egui::Label::new(egui::RichText::new(&cue.original_text).size(12.5).color(
                        if is_current {
                            crate::ui::theme::text_strong()
                        } else {
                            crate::ui::theme::text_weak()
                        },
                    ))
                    .truncate(),
                );

                if let Some(translated) = &cue.translated_text {
                    ui.add_space(1.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(translated).size(14.0).strong().color(
                                if is_current {
                                    Color32::from_rgb(30, 58, 138)
                                } else {
                                    Color32::from_rgb(30, 64, 175)
                                },
                            ),
                        )
                        .truncate(),
                    );
                }
            });
        })
        .response
        .interact(egui::Sense::click())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_and_last_cues_can_both_be_centered() {
        let viewport = 450.0;
        let count = 125;
        let content_height = timeline_content_height(count, viewport);
        let max_offset = content_height - viewport;

        assert_eq!(centered_cue_offset(0), 0.0);
        assert!((centered_cue_offset(count - 1) - max_offset).abs() < f32::EPSILON);
    }

    #[test]
    fn appending_cues_does_not_move_the_current_cue_center() {
        let active = 42;
        let viewport = 450.0;
        let target = centered_cue_offset(active);

        for count in [43, 100] {
            let max_offset = timeline_content_height(count, viewport) - viewport;
            assert!(target <= max_offset);
            let cue_center = timeline_padding(viewport)
                + active as f32 * (CUE_ROW_HEIGHT + CUE_ROW_GAP)
                + CUE_ROW_HEIGHT * 0.5;
            assert!((cue_center - target - viewport * 0.5).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn visible_range_is_bounded_for_dynamic_content() {
        let viewport =
            egui::Rect::from_min_max(egui::pos2(0.0, 1_000.0), egui::pos2(900.0, 1_450.0));
        let range = visible_cue_range(viewport, 20, 450.0);

        assert!(range.start < range.end);
        assert!(range.end <= 20);
    }
}
