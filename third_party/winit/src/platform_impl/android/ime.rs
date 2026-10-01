use android_activity::input::{Keycode, TextInputAction, TextInputState};

use super::{keycodes, DeviceId, KeyEventExtra};
use crate::event::{self, ElementState, Ime, WindowEvent};

#[derive(Default)]
pub(super) struct State {
    allowed: bool,
    committed: String,
    preedit: String,
    pub events: Vec<WindowEvent>,
}

impl State {
    pub fn set_allowed(&mut self, allowed: bool) -> bool {
        if self.allowed == allowed {
            return false;
        }
        self.clear_preedit();
        self.committed.clear();
        self.allowed = allowed;
        self.events.push(WindowEvent::Ime(if allowed {
            Ime::Enabled
        } else {
            Ime::Disabled
        }));
        true
    }

    pub fn text(&mut self, state: &TextInputState) {
        if !self.allowed {
            return;
        }

        // GameTextInput forwards Java UTF-16 offsets; winit preedit uses UTF-8 bytes.
        let compose_start = state
            .compose_region
            .filter(|span| span.start != span.end)
            .map(|span| utf16_offset(&state.text, span.start.min(span.end)));
        let committed_end = compose_start.unwrap_or(state.text.len());
        let committed = &state.text[..committed_end];
        if committed != self.committed {
            self.clear_preedit();
            let common = self
                .committed
                .chars()
                .zip(committed.chars())
                .take_while(|(old, new)| old == new)
                .map(|(c, _)| c.len_utf8())
                .sum::<usize>();
            for _ in 0..self.committed[common..].chars().count() {
                self.key(Keycode::Del);
            }
            if !committed[common..].is_empty() {
                self.events
                    .push(WindowEvent::Ime(Ime::Preedit(String::new(), None)));
                self.events.push(WindowEvent::Ime(Ime::Commit(
                    committed[common..].to_owned(),
                )));
            }
            self.committed = committed.to_owned();
        }

        if let Some(start) = compose_start {
            let cursor = |offset| utf16_offset(&state.text, offset).saturating_sub(start);
            let selection = (cursor(state.selection.start), cursor(state.selection.end));
            self.events.push(WindowEvent::Ime(Ime::Preedit(
                state.text[start..].to_owned(),
                Some((selection.0.min(selection.1), selection.0.max(selection.1))),
            )));
            self.preedit = state.text[start..].to_owned();
        } else {
            self.clear_preedit();
        }
    }

    pub fn action(&mut self, action: TextInputAction) {
        if !self.allowed {
            return;
        }
        if !self.preedit.is_empty() {
            let text = std::mem::take(&mut self.preedit);
            self.events
                .push(WindowEvent::Ime(Ime::Preedit(String::new(), None)));
            self.committed.push_str(&text);
            self.events.push(WindowEvent::Ime(Ime::Commit(text)));
        }
        match action {
            TextInputAction::Next => self.key(Keycode::Tab),
            TextInputAction::Unspecified
            | TextInputAction::None
            | TextInputAction::Go
            | TextInputAction::Search
            | TextInputAction::Send
            | TextInputAction::Done => {
                self.key(Keycode::Enter);
            }
            _ => {}
        }
    }

    fn clear_preedit(&mut self) {
        if !self.preedit.is_empty() {
            self.events
                .push(WindowEvent::Ime(Ime::Preedit(String::new(), None)));
            self.preedit.clear();
        }
    }

    fn key(&mut self, keycode: Keycode) {
        for state in [ElementState::Pressed, ElementState::Released] {
            self.events.push(WindowEvent::KeyboardInput {
                device_id: event::DeviceId(DeviceId(-1)),
                event: event::KeyEvent {
                    physical_key: keycodes::to_physical_key(keycode),
                    logical_key: keycodes::to_logical(None, keycode),
                    text: None,
                    location: keycodes::to_location(keycode),
                    state,
                    repeat: false,
                    platform_specific: KeyEventExtra {},
                },
                is_synthetic: false,
            });
        }
    }
}

fn utf16_offset(text: &str, offset: usize) -> usize {
    let mut units = 0;
    for (index, ch) in text.char_indices() {
        if units + ch.len_utf16() > offset {
            return index;
        }
        units += ch.len_utf16();
    }
    text.len()
}
