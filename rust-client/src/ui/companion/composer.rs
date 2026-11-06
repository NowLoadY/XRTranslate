//! A small goal entry surface; execution stays with the application's Agent.
use crate::{
    i18n::{UiLanguage, tr},
    ui::{components, theme},
};
use components::text_composer::SubmitShortcut;
use eframe::egui::{self, Id, Pos2, Rect};
use ui_director::egui as automation;

#[derive(Clone, Default)]
pub(super) struct Composer {
    pub open: bool,
    draft: String,
    focus: bool,
    submit_key: SubmitShortcut,
    rect: Option<Rect>,
    error: Option<&'static str>,
}

pub(super) enum Action {
    Submit(String),
    Stop,
}

fn editor_id() -> Id {
    super::state_id().with("agent-input")
}

impl Composer {
    pub fn toggle(&mut self, ctx: &egui::Context) {
        if self.open {
            self.close(ctx);
        } else {
            self.open = true;
            self.focus = true;
            self.rect = None;
            self.error = None;
        }
    }

    pub fn close(&mut self, ctx: &egui::Context) {
        if !self.open {
            return;
        }
        self.open = false;
        self.submit_key = SubmitShortcut::default();
        ctx.memory_mut(|memory| memory.surrender_focus(editor_id()));
        ctx.request_repaint();
    }

    pub fn accepted(&mut self, ctx: &egui::Context) {
        self.draft.clear();
        self.close(ctx);
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        anchor: Pos2,
        radius: f32,
        occupied: &[Rect],
        language: UiLanguage,
        running: bool,
    ) -> Option<Action> {
        if !self.open {
            return None;
        }
        let bounds = ctx.content_rect().shrink(12.0);
        let notice = self.error.or(running.then_some("Agent is working…"));
        let width = 340.0_f32.min(bounds.width());
        let row_height = ctx.global_style().spacing.interact_size.y.max(32.0);
        let notice_height = notice.map_or(0.0, |text| {
            ctx.fonts_mut(|fonts| {
                fonts
                    .layout(
                        tr(language, text).into(),
                        egui::FontId::proportional(12.0),
                        theme::text_weak(),
                        (width - 22.0).max(1.0),
                    )
                    .size()
                    .y
            }) + ctx.global_style().spacing.item_spacing.y
        });
        let size = egui::vec2(width, row_height + 22.0 + notice_height).min(bounds.size());
        let rect = self
            .rect
            .filter(|rect| rect.size() == size && bounds.contains_rect(*rect))
            .unwrap_or_else(|| {
                components::avatar::popup_rect(bounds, anchor, radius, size, occupied)
            });
        self.rect = Some(rect);
        let just_opened = self.focus;
        let enter = self.submit_key.pressed(ctx, editor_id());
        let can_send = !self.draft.trim().is_empty();
        let mut action = None;
        let mut send = false;
        let response = egui::Area::new(editor_id().with("panel"))
            .order(egui::Order::Foreground)
            .fixed_pos(rect.min)
            .default_size(size)
            .constrain_to(bounds)
            .movable(false)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(theme::surface_control())
                    .stroke(egui::Stroke::new(1.0, theme::border()))
                    .corner_radius(16)
                    .inner_margin(10)
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 3],
                        blur: 14,
                        spread: 0,
                        color: egui::Color32::from_black_alpha(18),
                    })
                    .show(ui, |ui| {
                        ui.set_width((size.x - 22.0).max(1.0));
                        ui.allocate_ui_with_layout(
                            egui::vec2(ui.available_width(), row_height),
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                let button = ui
                                    .add_enabled_ui(can_send, |ui| {
                                        components::icon_button(
                                            ui,
                                            tr(language, "Send"),
                                            egui::include_image!(
                                                "../../../../crates/xrtranslate-client-core/resources/icons/chevron-right.svg"
                                            ),
                                        )
                                    })
                                    .inner;
                                send = button.clicked() || enter;
                                if running {
                                    if components::icon_button(
                                        ui,
                                        tr(language, "Stop"),
                                        egui::include_image!("../../../../crates/xrtranslate-client-core/resources/icons/stop.svg"),
                                    )
                                    .clicked()
                                    {
                                        action = Some(Action::Stop);
                                    }
                                }
                                let response = ui.add(
                                    egui::TextEdit::singleline(&mut self.draft)
                                        .id(editor_id())
                                        .desired_width(f32::INFINITY)
                                        .font(egui::FontId::proportional(14.0))
                                        .frame(egui::Frame::NONE)
                                        .hint_text(tr(
                                            language,
                                            if running {
                                                "Add a requirement or correction…"
                                            } else {
                                                "What would you like me to do?"
                                            },
                                        )),
                                );
                                let response = automation::text_response(
                                    ui,
                                    tr(language, "Agent instruction"),
                                    &mut self.draft,
                                    response,
                                );
                                if response.changed() {
                                    self.error = None;
                                }
                                if std::mem::take(&mut self.focus) {
                                    response.request_focus();
                                }
                            },
                        );
                        if let Some(notice) = notice {
                            ui.label(
                                egui::RichText::new(tr(language, notice))
                                    .size(12.0)
                                    .color(theme::text_weak()),
                            );
                        }
                    });
            })
            .response;
        if send && !self.draft.trim().is_empty() {
            if self.draft.trim().len() > 4000 {
                self.error = Some("Please shorten the instruction");
            } else {
                action = Some(Action::Submit(self.draft.trim().to_owned()));
            }
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
            || (!just_opened && response.clicked_elsewhere())
        {
            self.close(ctx);
            return None;
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(
        ctx: &egui::Context,
        composer: &mut Composer,
        events: Vec<egui::Event>,
        running: bool,
    ) -> Option<Action> {
        let mut action = None;
        let mut output = ctx.run_pass(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0))),
                events,
                ..Default::default()
            },
            |ctx| {
                action = composer.show(
                    ctx,
                    egui::pos2(400.0, 200.0),
                    30.0,
                    &[],
                    UiLanguage::Chinese,
                    running,
                );
            },
        );
        output.textures_delta.clear();
        if composer.open {
            let actual = ctx
                .memory(|memory| memory.area_rect(editor_id().with("panel")))
                .unwrap();
            let planned = composer.rect.unwrap();
            assert!(
                actual.width() <= planned.width() + 1.0
                    && actual.height() <= planned.height() + 1.0,
                "Composer grew beyond its popup: actual={actual:?}, planned={planned:?}"
            );
        }
        action
    }

    fn key(key: egui::Key, pressed: bool) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn enter_waits_for_ime_and_keeps_the_draft_until_accepted() {
        let ctx = egui::Context::default();
        let mut composer = Composer::default();
        composer.toggle(&ctx);
        for running in [false, true, false] {
            for _ in 0..16 {
                frame(&ctx, &mut composer, vec![], running);
            }
        }
        assert!(ctx.memory(|memory| memory.has_focus(editor_id())));
        assert!(
            frame(
                &ctx,
                &mut composer,
                vec![egui::Event::Ime(egui::ImeEvent::Preedit {
                    text: "整理".into(),
                    active_range_chars: None,
                })],
                false
            )
            .is_none()
        );
        assert!(
            frame(
                &ctx,
                &mut composer,
                vec![
                    egui::Event::Ime(egui::ImeEvent::Commit("整理字幕".into())),
                    key(egui::Key::Enter, true)
                ],
                false
            )
            .is_none()
        );
        frame(
            &ctx,
            &mut composer,
            vec![key(egui::Key::Enter, false)],
            false,
        );
        assert_eq!(composer.draft, "整理字幕");
        for running in [false, true] {
            let action = frame(
                &ctx,
                &mut composer,
                vec![key(egui::Key::Enter, true)],
                running,
            );
            assert!(matches!(action, Some(Action::Submit(ref goal)) if goal == "整理字幕"));
            frame(
                &ctx,
                &mut composer,
                vec![key(egui::Key::Enter, false)],
                running,
            );
        }
        assert!(composer.open);
        assert_eq!(composer.draft, "整理字幕");
        composer.accepted(&ctx);
        assert!(!composer.open);
        assert!(composer.draft.is_empty());
        assert!(!ctx.memory(|memory| memory.has_focus(editor_id())));
        composer.toggle(&ctx);
        frame(&ctx, &mut composer, vec![], false);
        frame(
            &ctx,
            &mut composer,
            vec![egui::Event::Text("保留草稿".into())],
            false,
        );
        assert!(
            frame(
                &ctx,
                &mut composer,
                vec![key(egui::Key::Escape, true)],
                false
            )
            .is_none()
        );
        assert!(!composer.open);
        assert_eq!(composer.draft, "保留草稿");
    }
}
