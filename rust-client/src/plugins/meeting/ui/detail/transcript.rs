use super::super::presentation::format_duration;
use crate::plugins::meeting::controller::MeetingController;
use eframe::egui;

pub(super) fn render_transcript(
    controller: &mut MeetingController,
    _language: crate::i18n::UiLanguage,
    ui: &mut egui::Ui,
) {
    if let Some(bundle) = controller.bundle.as_ref() {
        egui::ScrollArea::vertical()
            .id_salt("meeting_full_transcript")
            .show(ui, |ui| {
                for segment in &bundle.segments {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            egui::RichText::new(format_duration(segment.start_ms))
                                .monospace()
                                .size(11.0)
                                .color(crate::ui::theme::text_weak()),
                        );
                        ui.label(
                            egui::RichText::new(&segment.original_text)
                                .size(12.5)
                                .color(crate::ui::theme::text_normal()),
                        );
                        if let Some(text) = super::segment_preview(&controller.previews, segment)
                            .and_then(|preview| preview.translated.as_ref())
                            .or(segment.translated_text.as_ref())
                        {
                            ui.label(
                                egui::RichText::new(text)
                                    .strong()
                                    .size(13.0)
                                    .color(crate::ui::theme::text_strong()),
                            );
                        }
                    });
                    ui.separator();
                }
                super::render_unstored_previews(ui, &controller.previews, &bundle.segments);
            });
    }
}
