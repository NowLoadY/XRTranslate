use super::*;
use crate::i18n::UiLanguage;

const LIST: &str = "subtitle_merge_interaction_test";

#[derive(Clone, Copy)]
struct Row {
    id: usize,
    ordinal: Option<usize>,
}

fn key(row: &Row) -> egui::Id {
    egui::Id::new((LIST, row.id))
}

#[derive(Default)]
struct Frame {
    rows: Vec<(usize, egui::Rect)>,
    buttons: Vec<(usize, egui::Rect)>,
    clicked: Vec<usize>,
    confirmed: Option<(usize, usize)>,
    gesture: Option<MergeGesture>,
    offset: f32,
    dialog_buttons: Vec<(String, egui::Pos2)>,
    dragged_width: Option<f32>,
    dragged_alpha: Option<u8>,
}

impl Frame {
    fn row(&self, index: usize) -> egui::Rect {
        self.rows.iter().find(|(id, _)| *id == index).unwrap().1
    }

    fn dialog_button(&self, text: &str) -> egui::Pos2 {
        self.dialog_buttons
            .iter()
            .find(|(label, _)| label == text)
            .unwrap_or_else(|| panic!("Missing dialog button {text}"))
            .1
    }
}

struct Harness {
    ctx: egui::Context,
    rows: Vec<(f32, Row)>,
    enabled: bool,
    time: f64,
}

impl Harness {
    fn new(ordinals: &[Option<usize>]) -> Self {
        let mut harness = Self {
            ctx: egui::Context::default(),
            rows: ordinals
                .iter()
                .enumerate()
                .map(|(id, ordinal)| {
                    (
                        88.0,
                        Row {
                            id,
                            ordinal: *ordinal,
                        },
                    )
                })
                .collect(),
            enabled: true,
            time: 0.0,
        };
        for _ in 0..3 {
            harness.idle();
        }
        harness
    }

    fn frame(&mut self, events: Vec<egui::Event>, target: ScrollTarget) -> Frame {
        self.time += 1.0 / 120.0;
        let mut frame = Frame::default();
        let mut gesture_id = egui::Id::NULL;
        let mut scroll_id = egui::Id::NULL;
        let mut output = self.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(640.0, 440.0),
                )),
                time: Some(self.time),
                events,
                ..Default::default()
            },
            |ui| {
                gesture_id = ui.make_persistent_id((LIST, "merge"));
                scroll_id = ui.make_persistent_id(egui::IdSalt::new(LIST));
                frame.confirmed = show_subtitle_rows(
                    ui,
                    LIST,
                    8.0,
                    target,
                    key,
                    SubtitleMerge {
                        enabled: self.enabled,
                        language: UiLanguage::English,
                        index: &|row| row.ordinal,
                        text: &|_| "A subtitle for merging.",
                    },
                    |_| &self.rows,
                    |ui, _, height, row| {
                        let rect = egui::Rect::from_min_size(
                            ui.next_widget_position(),
                            egui::vec2(ui.available_width(), height),
                        );
                        frame.rows.push((row.id, rect));
                        // A unique solid marker also lets the test inspect the actual
                        // painted size and opacity of the floating source row.
                        if row.id == 0 {
                            ui.painter().rect_filled(rect, 0.0, egui::Color32::RED);
                        }
                        let button = ui.button("Copy");
                        frame.buttons.push((row.id, button.rect));
                        if button.clicked() {
                            frame.clicked.push(row.id);
                        }
                        ui.add(
                            egui::Label::new("Original and translated subtitle")
                                .selectable(false)
                                .sense(egui::Sense::click()),
                        );
                    },
                );
            },
        );
        frame.gesture = self.ctx.data(|data| data.get_temp(gesture_id));
        frame.offset = egui::scroll_area::State::load(&self.ctx, scroll_id)
            .unwrap()
            .offset
            .y;
        fn inspect(shape: &egui::Shape, frame: &mut Frame) {
            match shape {
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        inspect(shape, frame);
                    }
                }
                egui::Shape::Text(text)
                    if matches!(text.galley.job.text.as_str(), "Merge" | "Cancel") =>
                {
                    frame.dialog_buttons.push((
                        text.galley.job.text.clone(),
                        text.pos + text.galley.rect.center().to_vec2(),
                    ));
                }
                egui::Shape::Rect(rect)
                    if rect.fill.r() > 0 && rect.fill.g() == 0 && rect.fill.b() == 0 =>
                {
                    frame.dragged_width = Some(rect.rect.width());
                    frame.dragged_alpha = Some(rect.fill.a());
                }
                _ => {}
            }
        }
        for shape in &output.shapes {
            inspect(&shape.shape, &mut frame);
        }
        output.textures_delta.clear();
        frame
    }

    fn idle(&mut self) -> Frame {
        self.frame(Vec::new(), ScrollTarget::None)
    }

    fn pointer(&mut self, point: egui::Pos2, pressed: Option<bool>) -> Frame {
        let mut events = vec![egui::Event::PointerMoved(point)];
        if let Some(pressed) = pressed {
            events.push(egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        self.frame(events, ScrollTarget::None)
    }

    fn drag(&mut self, from: usize, to: usize) -> (Frame, egui::Pos2) {
        let initial = self.idle();
        let origin = initial.row(from).center();
        let destination = initial.row(to).center();
        assert!(self.pointer(origin, Some(true)).gesture.is_none());
        self.pointer(destination, None);
        (self.pointer(destination, None), destination)
    }

    fn escape(&mut self) -> Frame {
        self.frame(
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ScrollTarget::None,
        )
    }
}

#[test]
fn adjacent_drag_previews_then_requires_confirmation_in_both_directions() {
    for (source, target) in [(0, 1), (1, 0)] {
        let mut harness = Harness::new(&[Some(0), Some(1), Some(2)]);
        let (preview, destination) = harness.drag(source, target);
        let gesture = preview.gesture.as_ref().unwrap();
        assert_eq!(gesture.source, key(&harness.rows[source].1));
        assert_eq!(gesture.target, Some(key(&harness.rows[target].1)));
        assert!(!gesture.pending);
        assert!(preview.confirmed.is_none());
        if source == 0 {
            assert!((preview.dragged_width.unwrap() - preview.row(0).width() * 0.9).abs() < 0.1);
            assert!(preview.dragged_alpha.unwrap() < 180);
        }
        let released = harness.pointer(destination, Some(false));
        assert!(released.confirmed.is_none());
        assert!(released.gesture.unwrap().pending);
        let dialog = harness.idle();
        let confirm = dialog.dialog_button("Merge");
        harness.pointer(confirm, Some(true));
        let accepted = harness.pointer(confirm, Some(false));
        assert_eq!(accepted.confirmed, Some((source, target)));
        assert!(accepted.gesture.is_none());
    }
}

#[test]
fn filtered_gaps_non_subtitle_rows_and_outside_drops_never_merge() {
    for ordinals in [vec![Some(0), Some(2)], vec![Some(0), None, Some(1)]] {
        let mut harness = Harness::new(&ordinals);
        let (preview, destination) = harness.drag(0, 1);
        assert!(preview.gesture.unwrap().target.is_none());
        let released = harness.pointer(destination, Some(false));
        assert!(released.gesture.is_none());
        assert!(released.confirmed.is_none());
    }
    // A heading between adjacent subtitles does not make them non-neighbors.
    let mut harness = Harness::new(&[Some(0), None, Some(1)]);
    let (preview, _) = harness.drag(0, 2);
    assert_eq!(
        preview.gesture.unwrap().target,
        Some(key(&harness.rows[2].1))
    );
    let outside = egui::pos2(700.0, 500.0);
    harness.pointer(outside, None);
    let released = harness.pointer(outside, Some(false));
    assert!(released.gesture.is_none());
    assert!(released.confirmed.is_none());
}

#[test]
fn child_buttons_still_click_and_escape_cancels_the_pending_dialog() {
    let mut harness = Harness::new(&[Some(0), Some(1)]);
    let initial = harness.idle();
    let button = initial.buttons[0].1.center();
    assert!(harness.pointer(button, Some(true)).gesture.is_none());
    let clicked = harness.pointer(button, Some(false));
    assert_eq!(clicked.clicked, vec![0]);
    assert!(clicked.gesture.is_none());

    let (_, destination) = harness.drag(0, 1);
    assert!(
        harness
            .pointer(destination, Some(false))
            .gesture
            .unwrap()
            .pending
    );
    let canceled = harness.escape();
    assert!(canceled.confirmed.is_none());
    assert!(canceled.gesture.is_none());
    assert!(harness.idle().gesture.is_none());
}

#[test]
fn drag_holds_position_during_wheel_input_following_and_an_existing_scroll_animation() {
    let mut harness = Harness::new(&(0..24).map(Some).collect::<Vec<_>>());
    let before = harness.frame(Vec::new(), ScrollTarget::Row(8));
    let origin = before.row(1).center();
    harness.pointer(origin, Some(true));
    let dragging = harness.pointer(origin + egui::vec2(0.0, 20.0), None);
    assert!(dragging.gesture.is_some());
    let frozen = harness.idle();
    let offset = frozen.offset;
    let source_top = frozen.row(1).top();
    for _ in 0..40 {
        let frame = harness.frame(
            vec![egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -100.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::NONE,
            }],
            ScrollTarget::End,
        );
        assert!(frame.gesture.is_some());
        assert!((frame.offset - offset).abs() < 0.1);
        assert!((frame.row(1).top() - source_top).abs() < 0.1);
    }
    assert!(harness.escape().gesture.is_none());
}

#[test]
fn changed_rows_or_disabled_editing_cancel_a_pending_merge() {
    for disable in [false, true] {
        let mut harness = Harness::new(&[Some(0), Some(1), Some(2)]);
        let (_, destination) = harness.drag(0, 1);
        assert!(
            harness
                .pointer(destination, Some(false))
                .gesture
                .unwrap()
                .pending
        );
        if disable {
            harness.enabled = false;
        } else {
            harness.rows.remove(1);
        }
        let frame = harness.idle();
        assert!(frame.confirmed.is_none());
        assert!(frame.gesture.is_none());
    }
}
