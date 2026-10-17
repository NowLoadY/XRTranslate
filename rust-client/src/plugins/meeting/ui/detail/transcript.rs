use super::super::actions::UiAction;
use crate::plugins::meeting::controller::MeetingController;
use eframe::egui;

pub(super) fn render_transcript(
    controller: &mut MeetingController,
    language: crate::i18n::UiLanguage,
    action: &mut UiAction,
    ui: &mut egui::Ui,
) {
    let can_seek = controller.active_meeting_id().is_none();
    if let Some(bundle) = controller.bundle.as_ref() {
        let active_id = controller
            .playback
            .current_segment(&bundle.segments)
            .map(|segment| segment.id.clone());
        let mut reached = false;
        egui::ScrollArea::vertical()
            .id_salt("meeting_full_transcript")
            .show(ui, |ui| {
                for segment in &bundle.segments {
                    let active = active_id.as_deref() == Some(&segment.id);
                    let can_seek = can_seek && controller.playback.can_seek_segment(segment);
                    let row = egui::Frame::new()
                        .inner_margin(egui::Margin {
                            left: 8,
                            ..egui::Margin::ZERO
                        })
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                super::timestamp(segment, can_seek, active, language, action, ui);
                                super::sentence_text(
                                    egui::RichText::new(&segment.original_text)
                                        .size(12.5)
                                        .color(crate::ui::theme::text_normal()),
                                    &segment.id,
                                    can_seek,
                                    language,
                                    action,
                                    ui,
                                );
                                if let Some(text) =
                                    super::segment_preview(&controller.previews, segment)
                                        .and_then(|preview| preview.translated.as_ref())
                                        .or(segment.translated_text.as_ref())
                                {
                                    super::sentence_text(
                                        egui::RichText::new(text)
                                            .strong()
                                            .size(13.0)
                                            .color(crate::ui::theme::text_strong()),
                                        &segment.id,
                                        can_seek,
                                        language,
                                        action,
                                        ui,
                                    );
                                }
                            });
                        });
                    super::current_marker(ui, row.response.rect, active);
                    if controller.evidence_target.as_deref() == Some(&segment.id) {
                        row.response.scroll_to_me(Some(egui::Align::Center));
                        reached = true;
                    }
                    ui.separator();
                }
                super::render_unstored_previews(ui, &controller.previews, &bundle.segments);
            });
        if reached {
            controller.evidence_target = None;
        }
    }
}
