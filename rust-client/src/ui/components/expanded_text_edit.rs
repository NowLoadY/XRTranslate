use crate::i18n::{UiLanguage, tr};
use eframe::egui::{self, Context, Id};

/// A single scrolling editor covering the app, sized above the mobile keyboard.
/// Returns whether the user has finished editing; the caller owns the draft.
pub(crate) fn expanded_text_edit(
    ctx: &Context,
    id: Id,
    title: &str,
    text: &mut String,
    focus_pending: &mut bool,
    language: UiLanguage,
) -> bool {
    let viewport = ctx.content_rect();
    let mut closed = false;
    let editor_id = id;
    let modal_id = id.with("expanded");
    let modal = egui::Modal::new(modal_id)
        .area(
            egui::Modal::default_area(modal_id)
                .pivot(egui::Align2::LEFT_TOP)
                .fixed_pos(viewport.min),
        )
        .frame(
            egui::Frame::new()
                .fill(egui::Color32::WHITE)
                .inner_margin(16),
        )
        .show(ctx, |ui| {
            ui.set_min_size((viewport.size() - egui::Vec2::splat(32.0)).max(egui::Vec2::ZERO));
            ui.set_max_size(ui.min_size());
            ui.horizontal(|ui| {
                closed = super::back_button(ui, tr(language, "Back")).clicked();
                ui.strong(title);
            });
            ui.add_space(12.0);
            let height = ui.available_height().max(1.0);
            egui::ScrollArea::vertical()
                .id_salt(id.with("scroll"))
                .auto_shrink([false, false])
                .max_height(height)
                .show(ui, |ui| {
                    let response = super::text_edit_ui(
                        ui,
                        editor_id,
                        egui::TextEdit::multiline(text)
                            .id(editor_id)
                            .desired_width(f32::INFINITY)
                            .min_size(egui::vec2(0.0, height))
                            .frame(egui::Frame::NONE),
                    );
                    if ui.is_enabled() && std::mem::take(focus_pending) {
                        response.request_focus();
                    }
                });
        });
    closed |= modal.should_close();
    if closed {
        ctx.memory_mut(|memory| memory.surrender_focus(editor_id));
        ctx.output_mut(|output| output.ime = None);
        ctx.request_repaint();
    }
    closed
}
