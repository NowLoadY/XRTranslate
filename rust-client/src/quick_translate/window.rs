use super::{Action, QuickTranslate};
use crate::{
    i18n::{UiLanguage, tr},
    ui::{components, theme},
};
use eframe::egui;
pub(super) fn id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("quick_translation")
}
impl QuickTranslate {
    pub fn show(&self, ctx: &egui::Context, language: UiLanguage, appearance: theme::UiTheme) {
        if !self.view.lock().unwrap().open {
            return;
        }
        let state = self.view.clone();
        let actions = self.action_tx.clone();
        let parent = ctx.clone();
        ctx.show_viewport_deferred(
            id(),
            egui::ViewportBuilder::default()
                .with_title(tr(language, "Quick translation"))
                .with_inner_size([440.0, 330.0])
                .with_min_inner_size([300.0, 200.0])
                .with_always_on_top()
                .with_taskbar(false),
            move |ui, _class| {
                let ctx = ui.ctx().clone();
                theme::install_context(&ctx, appearance);
                let view = state.lock().unwrap().clone();
                let mut close = ctx
                    .input(|i| i.viewport().close_requested() || i.key_pressed(egui::Key::Escape));
                let mut content = |ui: &mut egui::Ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            egui::RichText::new(tr(language, "Quick translation"))
                                .size(16.0)
                                .color(theme::text_strong())
                                .strong(),
                        );
                        if components::secondary_button(ui, tr(language, "Close")).clicked() {
                            close = true;
                        }
                        if !view.translated.is_empty()
                            && components::primary_button(ui, tr(language, "Copy translation"))
                                .clicked()
                        {
                            ui.ctx().copy_text(view.translated.to_string());
                        }
                    });
                    ui.add_space(10.0);
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        if !view.source.is_empty() {
                            components::section(ui, tr(language, "Original text"), |ui| {
                                ui.set_width(ui.available_width());
                                ui.add(
                                    egui::Label::new(view.source.as_ref())
                                        .wrap()
                                        .selectable(true),
                                );
                            });
                        }
                        if view.busy {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(tr(language, "Translating…"));
                            });
                        }
                        if !view.translated.is_empty() {
                            components::section(ui, tr(language, "Translation"), |ui| {
                                ui.set_width(ui.available_width());
                                ui.add(
                                    egui::Label::new(view.translated.as_ref())
                                        .wrap()
                                        .selectable(true),
                                );
                            });
                        }
                        if let Some(error) = &view.error {
                            ui.add(
                                egui::Label::new(crate::i18n::tr_dynamic(language, error)).wrap(),
                            );
                            if !view.source.is_empty()
                                && components::secondary_button(ui, tr(language, "Retry")).clicked()
                            {
                                let _ = actions.send(Action::Retry);
                                parent.request_repaint();
                            }
                        }
                    });
                };
                egui::Frame::new().inner_margin(14).show(ui, &mut content);
                if close {
                    state.lock().unwrap().open = false;
                    let _ = actions.send(Action::Close);
                    parent.request_repaint();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            },
        );
    }
}
