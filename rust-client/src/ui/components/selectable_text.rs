//! Selection for already laid out, read-only history text.
//!
//! Keep the selected galley intact while live translations are revised. The
//! cursor mechanics and selection painting come from egui, just as in TextEdit.
use std::{ops::Range, sync::Arc};

use eframe::egui::{self, Galley, Id, Rect, Response, text_selection::TextCursorState};

#[derive(Clone, Debug)]
pub struct SelectionText {
    pub text: String,
    pub context: String,
    pub context_offset: usize,
    pub anchor: Rect,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Harness {
        ctx: egui::Context,
        state: SelectionState,
        time: f64,
        galley: Option<Arc<Galley>>,
        origin: egui::Pos2,
        copied: Vec<String>,
        align: egui::Align,
    }

    impl Harness {
        fn new() -> Self {
            Self {
                ctx: egui::Context::default(),
                state: SelectionState::default(),
                time: 0.0,
                galley: None,
                origin: egui::Pos2::ZERO,
                copied: Vec::new(),
                align: egui::Align::Min,
            }
        }

        fn frame(
            &mut self,
            text: &str,
            ranges: &[Range<usize>],
            events: Vec<egui::Event>,
        ) -> Option<SelectionText> {
            self.time += 0.04;
            let mut lookup = None;
            let mut output = self.ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600.0, 300.0),
                    )),
                    time: Some(self.time),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let galley = ui.fonts_mut(|fonts| {
                        let mut job = egui::text::LayoutJob::simple(
                            text.to_owned(),
                            egui::FontId::proportional(24.0),
                            egui::Color32::WHITE,
                            560.0,
                        );
                        job.halign = self.align;
                        fonts.layout_job(job)
                    });
                    let id = Id::new("test-history-line");
                    let galley = self.state.galley(id, galley);
                    let response = self.state.show(ui, id, Arc::clone(&galley), ranges);
                    self.origin = response.rect.min - galley.rect.min.to_vec2();
                    self.galley = Some(galley);
                    lookup = self
                        .state
                        .show_action(ui.ctx(), crate::i18n::UiLanguage::English);
                },
            );
            for command in output.platform_output.commands {
                if let egui::OutputCommand::CopyText(text) = command {
                    self.copied.push(text);
                }
            }
            output.textures_delta.clear();
            lookup
        }

        fn point(&self, character: usize) -> egui::Pos2 {
            self.galley
                .as_ref()
                .unwrap()
                .pos_from_cursor(egui::text::CCursor::new(character))
                .center()
                + self.origin.to_vec2()
        }

        fn selected_text(&self) -> &str {
            let selected = self.state.selected.as_ref().unwrap();
            selected
                .cursor
                .range(&selected.galley)
                .unwrap()
                .slice_str(selected.galley.text())
        }
    }

    fn button(position: egui::Pos2, pressed: bool) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(position),
            egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    #[test]
    fn pointer_drag_freezes_revisions_and_lookup_survives_button_focus() {
        let mut harness = Harness::new();
        harness.align = egui::Align::Center;
        let text = "hello brave world";
        let range = [0..text.len()];
        harness.frame(text, &range, vec![]);
        let start = harness.point(6);
        let end = harness.point(11);
        harness.frame(text, &range, button(start, true));
        let revision = "a completely different revision";
        harness.frame(
            revision,
            &[0..revision.len()],
            vec![egui::Event::PointerMoved(end)],
        );
        harness.frame(revision, &[0..revision.len()], button(end, false));
        assert_eq!(harness.selected_text(), "brave");
        assert_eq!(harness.galley.as_ref().unwrap().text(), text);
        harness.frame(revision, &[0..revision.len()], vec![]);
        let action = harness.state.action_rect.unwrap().center();
        assert!(
            harness
                .frame(revision, &[0..revision.len()], button(action, true))
                .is_none()
        );
        let lookup = harness
            .frame(revision, &[0..revision.len()], button(action, false))
            .unwrap();
        assert_eq!(lookup.text, "brave");
        assert_eq!(lookup.context, text);
        assert!(!harness.state.is_active());
    }

    #[test]
    fn native_double_click_selects_unicode_word_and_copy_keeps_utf8() {
        let mut harness = Harness::new();
        let text = "Speaker: café 世界";
        let ranges = ["Speaker: ".len()..text.len()];
        harness.frame(text, &ranges, vec![]);
        let point = harness.point(11);
        for _ in 0..2 {
            harness.frame(text, &ranges, button(point, true));
            harness.frame(text, &ranges, button(point, false));
        }
        assert_eq!(harness.selected_text(), "café");
        harness.frame(text, &ranges, vec![egui::Event::Copy]);
        assert_eq!(harness.copied, ["café"]);
        harness.frame(text, &ranges, button(egui::pos2(550.0, 250.0), true));
        assert!(!harness.state.is_active());
    }

    #[test]
    fn drag_stays_in_sentence_and_cannot_select_speaker_prefix() {
        let mut harness = Harness::new();
        let text = "Alice: hello Bob: goodbye";
        let ranges = [7..12, 18..25];
        harness.frame(text, &ranges, vec![]);
        let prefix = harness.point(2);
        harness.frame(text, &ranges, button(prefix, true));
        assert!(!harness.state.is_active());
        harness.frame(text, &ranges, button(prefix, false));
        let start = harness.point(8);
        let end = harness.point(24);
        harness.frame(text, &ranges, button(start, true));
        harness.frame(text, &ranges, vec![egui::Event::PointerMoved(end)]);
        harness.frame(text, &ranges, button(end, false));
        assert_eq!(harness.selected_text(), "ello");
    }
}

#[derive(Clone)]
struct Selected {
    id: Id,
    galley: Arc<Galley>,
    ranges: Vec<Range<usize>>,
    context_range: Range<usize>,
    cursor: TextCursorState,
    rect: Rect,
    anchor: Rect,
    seen_pass: u64,
    visuals: egui::Visuals,
}

/// A single selection shared by all the text blocks in one history view.
#[derive(Clone, Default)]
pub struct SelectionState {
    selected: Option<Selected>,
    action_rect: Option<Rect>,
}

impl SelectionState {
    pub fn is_active(&self) -> bool {
        self.selected.is_some()
    }

    pub fn clear(&mut self) {
        self.selected = None;
        self.action_rect = None;
    }

    /// Use this during measurement as well as painting so a streamed revision
    /// cannot move the text out from underneath the user's pointer.
    pub fn galley(&self, id: Id, fresh: Arc<Galley>) -> Arc<Galley> {
        self.selected
            .as_ref()
            .filter(|selected| selected.id == id)
            .map_or(fresh, |selected| Arc::clone(&selected.galley))
    }

    /// Render a full-width row without changing the galley's formatting.
    /// `selectable_ranges` are UTF-8 byte ranges of actual sentences, excluding
    /// headers and speaker labels. A selection stays within its starting range.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        id: Id,
        fresh: Arc<Galley>,
        selectable_ranges: &[Range<usize>],
    ) -> Response {
        let mut galley = self.galley(id, fresh);
        let (_, row) = ui.allocate_space(egui::vec2(ui.available_width(), galley.size().y));
        let origin = match galley.job.halign {
            egui::Align::Min => row.left_top(),
            egui::Align::Center => row.center_top(),
            egui::Align::Max => row.right_top(),
        };
        let text_rect = galley.rect.translate(origin.to_vec2());
        let response = ui.interact(text_rect, id, egui::Sense::click_and_drag());
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Label, true, galley.text())
        });
        if response.hovered() {
            ui.set_cursor_icon(egui::CursorIcon::Text);
        }

        if let Some(pointer) = response.interact_pointer_pos() {
            let at_pointer = galley.cursor_from_pos(pointer - origin);
            if response.hovered() && ui.input(|input| input.pointer.primary_pressed()) {
                let ranges = self
                    .selected
                    .as_ref()
                    .filter(|selected| selected.id == id)
                    .map_or(selectable_ranges, |selected| &selected.ranges);
                let byte = egui::text_selection::text_cursor_state::byte_index_from_char_index(
                    galley.text(),
                    at_pointer.index,
                )
                .0;
                let context_range = ranges
                    .iter()
                    .find(|range| range.contains(&byte))
                    .or_else(|| ranges.iter().find(|range| range.end == byte))
                    .filter(|range| galley.text().get((*range).clone()).is_some())
                    .cloned();
                if let Some(context_range) = context_range {
                    let keep_cursor = self.selected.as_ref().is_some_and(|selected| {
                        selected.id == id && selected.context_range == context_range
                    });
                    if !keep_cursor {
                        self.selected = Some(Selected {
                            id,
                            galley: Arc::clone(&galley),
                            ranges: ranges.to_vec(),
                            context_range,
                            cursor: TextCursorState::default(),
                            rect: text_rect,
                            anchor: text_rect,
                            seen_pass: ui.ctx().cumulative_pass_nr(),
                            visuals: ui.visuals().clone(),
                        });
                    }
                } else {
                    self.clear();
                }
            }
            if let Some(selected) = self.selected.as_mut().filter(|selected| selected.id == id) {
                if selected.cursor.pointer_interaction(
                    ui,
                    &response,
                    at_pointer,
                    &galley,
                    ui.is_being_dragged(id),
                ) {
                    response.request_focus();
                }
            }
        }

        if let Some(selected) = self.selected.as_mut().filter(|selected| selected.id == id) {
            selected.rect = text_rect.intersect(ui.clip_rect());
            selected.seen_pass = ui.ctx().cumulative_pass_nr();
            selected.visuals = ui.visuals().clone();
            if let Some(mut range) = selected.cursor.range(&galley) {
                if response.has_focus() {
                    for event in ui.input(|input| input.events.clone()) {
                        range.on_event(ui.ctx().os(), &event, &galley, id);
                    }
                }
                // Clamp after egui's word/line selection too: a double click must
                // never include the next speaker or a different translation.
                let text = galley.text();
                let first = text[..selected.context_range.start].chars().count();
                let last = text[..selected.context_range.end].chars().count();
                range.primary.index.0 = range.primary.index.0.clamp(first, last);
                range.secondary.index.0 = range.secondary.index.0.clamp(first, last);
                selected.cursor.set_char_range(Some(range));
                let [start, end] = range.sorted_cursors();
                selected.anchor = galley
                    .pos_from_cursor(start)
                    .union(galley.pos_from_cursor(end))
                    .translate(origin.to_vec2());
                if response.has_focus()
                    && ui.input(|input| {
                        input
                            .events
                            .iter()
                            .any(|event| matches!(event, egui::Event::Copy))
                    })
                    && !range.is_empty()
                {
                    ui.ctx().copy_text(range.slice_str(text).to_owned());
                }
                egui::text_selection::visuals::paint_text_selection(
                    &mut galley,
                    ui.visuals(),
                    &range,
                    None,
                );
            }
        }
        if ui.is_rect_visible(text_rect) {
            ui.painter()
                .galley(origin, galley, ui.visuals().text_color());
        }
        response
    }

    /// Call after drawing all text blocks. The button works from an owned
    /// snapshot, so pressing it cannot discard the lookup by taking focus.
    pub fn show_action(
        &mut self,
        ctx: &egui::Context,
        language: crate::i18n::UiLanguage,
    ) -> Option<SelectionText> {
        let selected = self.selected.as_ref()?;
        if selected.seen_pass != ctx.cumulative_pass_nr() || !selected.rect.is_positive() {
            self.clear();
            return None;
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.clear();
            return None;
        }

        let selection = selected.cursor.range(&selected.galley).and_then(|range| {
            let raw = range.slice_str(selected.galley.text());
            let text = raw.trim();
            let start = egui::text_selection::text_cursor_state::byte_index_from_char_index(
                selected.galley.text(),
                range.sorted_cursors()[0].index,
            )
            .0;
            // This is a lookup affordance, not a second sentence translator.
            // Longer selections remain usable for copying.
            crate::history::archive::vocabulary::can_save(text).then(|| SelectionText {
                text: text.to_owned(),
                context: selected.galley.text()[selected.context_range.clone()].to_owned(),
                context_offset: start - selected.context_range.start + raw.len()
                    - raw.trim_start().len(),
                anchor: selected.anchor,
            })
        });
        let text_rect = selected.rect;
        let mut clicked = false;
        let selecting = ctx.input(|input| {
            input.pointer.primary_down()
                && !self.action_rect.is_some_and(|rect| {
                    input
                        .pointer
                        .press_origin()
                        .is_some_and(|position| rect.contains(position))
                })
        });
        self.action_rect = None;
        if let Some(selection) = &selection {
            // Do not cover the word or intercept the selection's release.
            if !selecting {
                let screen = ctx.content_rect().shrink(6.0);
                let position = egui::pos2(
                    selection
                        .anchor
                        .right()
                        .clamp(screen.left(), (screen.right() - 36.0).max(screen.left())),
                    (selection.anchor.bottom() + 5.0)
                        .clamp(screen.top(), (screen.bottom() - 36.0).max(screen.top())),
                );
                let action = egui::Area::new(selected.id.with("lookup_action"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(position)
                    .movable(false)
                    .show(ctx, |ui| {
                        *ui.visuals_mut() = selected.visuals.clone();
                        ui.visuals_mut().panel_fill = ui.visuals().widgets.inactive.bg_fill;
                        super::icon_button(
                            ui,
                            crate::i18n::tr(language, "Look up"),
                            egui::include_image!("../../../resources/icons/search.svg"),
                        )
                    });
                clicked = action.inner.clicked();
                self.action_rect = Some(action.inner.rect);
            }
        }
        if clicked {
            self.clear();
            return selection;
        }
        if ctx.input(|input| {
            input.pointer.primary_pressed()
                && input.pointer.interact_pos().is_some_and(|position| {
                    !text_rect.contains(position)
                        && !self.action_rect.is_some_and(|rect| rect.contains(position))
                })
        }) {
            self.clear();
        }
        None
    }
}
