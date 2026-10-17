use crate::{
    i18n::UiLanguage,
    plugins::media::{controller::MediaController, i18n::tr},
    ui::components,
};
use eframe::egui;

pub(super) fn render(controller: &mut MediaController, language: UiLanguage, ctx: &egui::Context) {
    let Some(draft) = controller.subtitle_edit.as_mut() else {
        return;
    };
    let id = egui::Id::new(("media_subtitle_edit", &draft.task_id, &draft.cue_id));
    let mut save = false;
    let mut cancel = false;
    egui::Modal::new(id)
        .frame(egui::Frame::NONE)
        .show(ctx, |ui| {
            crate::ui::organic_border::show(
                ui,
                id.with("border"),
                egui::Frame::new()
                    .fill(crate::ui::theme::modal_backdrop())
                    .corner_radius(crate::ui::theme::container_radius(16))
                    .inner_margin(16),
                crate::ui::theme::container_radius_f32(16.0),
                crate::ui::theme::text_weak(),
                |ui| {
                    ui.set_width((ctx.content_rect().width() - 64.0).clamp(180.0, 500.0));
                    ui.strong(tr(language, "Edit subtitle"));
                    ui.add_space(8.0);
                    egui::ScrollArea::vertical()
                        .max_height((ctx.content_rect().height() - 180.0).max(80.0))
                        .show(ui, |ui| {
                            for (field, label, text) in [
                                ("subtitle_original", "Original", &mut draft.original),
                                ("subtitle_translation", "Translation", &mut draft.translated),
                            ] {
                                let label = ui.label(tr(language, label));
                                let response = components::text_edit_ui(
                                    ui,
                                    field,
                                    egui::TextEdit::multiline(text)
                                        .id(id.with(field))
                                        .desired_width(f32::INFINITY)
                                        .desired_rows(4),
                                )
                                .labelled_by(label.id);
                                if let Some(value) = crate::ui::automation::record_text_input(
                                    ui,
                                    response.id,
                                    field,
                                    text,
                                    ui.is_enabled(),
                                    response.rect,
                                ) {
                                    *text = value;
                                }
                                ui.add_space(8.0);
                            }
                        });
                    ui.add_space(8.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        save = ui
                            .add_enabled_ui(!draft.original.trim().is_empty(), |ui| {
                                components::icon_button(
                                    ui,
                                    tr(language, "Save"),
                                    egui::include_image!("../../../../resources/icons/check.svg"),
                                )
                            })
                            .inner
                            .clicked();
                        cancel = components::icon_button(
                            ui,
                            tr(language, "Cancel"),
                            egui::include_image!("../../../../resources/icons/x.svg"),
                        )
                        .clicked();
                    });
                },
            );
        });
    if save {
        controller.save_subtitle_edit();
    } else if cancel {
        controller.subtitle_edit = None;
    }
}
