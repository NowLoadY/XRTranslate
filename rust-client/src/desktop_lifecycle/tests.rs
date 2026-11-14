use super::*;

struct Harness {
    ctx: egui::Context,
    close: WindowClose,
    time: f64,
}

impl Harness {
    fn new() -> Self {
        Self {
            ctx: egui::Context::default(),
            close: WindowClose::Running,
            time: 0.0,
        }
    }

    fn frame(&mut self, request_close: bool, events: Vec<egui::Event>) -> egui::FullOutput {
        self.time += 0.05;
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(560.0, 300.0),
            )),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        if request_close {
            input
                .viewports
                .get_mut(&egui::ViewportId::ROOT)
                .unwrap()
                .events
                .push(egui::ViewportEvent::Close);
        }
        self.ctx.run_ui(input, |ui| {
            self.close.update(ui.ctx());
            self.close.render(ui.ctx(), UiLanguage::English);
        })
    }

    fn click(&mut self, label: &str) -> egui::FullOutput {
        let output = self.frame(false, vec![]);
        let position = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == label => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("Missing button: {label}"));
        let events = |pressed| {
            vec![
                egui::Event::PointerMoved(position),
                egui::Event::PointerButton {
                    pos: position,
                    pressed,
                    button: egui::PointerButton::Primary,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        };
        self.frame(false, events(true));
        self.frame(false, events(false))
    }
}

fn commands(output: &egui::FullOutput) -> &[egui::ViewportCommand] {
    &output.viewport_output[&egui::ViewportId::ROOT].commands
}

#[test]
fn closing_waits_for_a_choice_and_dismissal_keeps_the_window_open() {
    for escape in [false, true] {
        let mut harness = Harness::new();
        for _ in 0..2 {
            let output = harness.frame(true, vec![]);
            assert_eq!(harness.close, WindowClose::Confirming);
            assert!(commands(&output).contains(&egui::ViewportCommand::CancelClose));
            assert!(!commands(&output).contains(&egui::ViewportCommand::Close));
            assert!(!commands(&output).contains(&egui::ViewportCommand::Visible(false)));
        }
        let output = if escape {
            harness.frame(
                false,
                vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            )
        } else {
            harness.click("×")
        };
        assert_eq!(harness.close, WindowClose::Running);
        assert!(!commands(&output).contains(&egui::ViewportCommand::Close));
        assert!(!commands(&output).contains(&egui::ViewportCommand::Visible(false)));
    }
}

#[test]
fn background_choice_only_hides_the_window_and_the_next_close_asks_again() {
    let mut harness = Harness::new();
    harness.frame(true, vec![]);
    let output = harness.click("Keep running in background");
    assert_eq!(harness.close, WindowClose::Running);
    assert!(commands(&output).contains(&egui::ViewportCommand::Visible(false)));
    assert!(!commands(&output).contains(&egui::ViewportCommand::Close));
    harness.frame(true, vec![]);
    assert_eq!(harness.close, WindowClose::Confirming);
}

#[test]
fn complete_exit_closes_without_reopening_the_prompt() {
    let mut harness = Harness::new();
    harness.frame(true, vec![]);
    let output = harness.click("Exit completely");
    assert_eq!(harness.close, WindowClose::Exiting);
    assert!(commands(&output).contains(&egui::ViewportCommand::Close));
    let output = harness.frame(true, vec![]);
    assert_eq!(harness.close, WindowClose::Exiting);
    assert!(!commands(&output).contains(&egui::ViewportCommand::CancelClose));
}

#[test]
fn explicit_exit_bypasses_confirmation_even_when_it_is_already_open() {
    for close in [WindowClose::Running, WindowClose::Confirming] {
        let mut harness = Harness::new();
        harness.close = close;
        harness.close.exit(&harness.ctx);
        let output = harness.frame(true, vec![]);
        assert_eq!(harness.close, WindowClose::Exiting);
        assert!(!commands(&output).contains(&egui::ViewportCommand::CancelClose));
        assert!(output.shapes.iter().all(|shape| !matches!(
            &shape.shape, egui::Shape::Text(text) if text.galley.text() == "Also stop running in the background?"
        )));
    }
}
