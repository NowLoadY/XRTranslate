//! Transient reminders wait for the companion to be ready to read them.
use std::collections::VecDeque;

#[derive(Default)]
pub(crate) struct Inbox {
    letters: VecDeque<&'static str>,
    reading: Option<&'static str>,
}

impl Inbox {
    pub(crate) fn post(&mut self, message: &'static str) {
        if self.reading != Some(message) && !self.letters.contains(&message) {
            self.letters.push_back(message);
        }
    }

    pub(super) fn read(&mut self) -> Option<&'static str> {
        // The companion calls this only after finishing its sentence and cooldown.
        self.reading = self.letters.pop_front();
        self.reading
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeats_are_merged_while_the_companion_is_reading_the_letter() {
        let mut inbox = Inbox::default();
        inbox.post("reminder");
        assert_eq!(inbox.read(), Some("reminder"));
        inbox.post("reminder");
        assert_eq!(inbox.read(), None);
        inbox.post("reminder");
        assert_eq!(inbox.read(), Some("reminder"));
    }

    #[test]
    fn mail_is_ordered_and_repeated_pending_reminders_are_merged() {
        let mut inbox = Inbox::default();
        inbox.post("start translation");
        inbox.post("start microphone");
        inbox.post("start translation");
        assert_eq!(inbox.read(), Some("start translation"));
        assert_eq!(inbox.read(), Some("start microphone"));
        assert_eq!(inbox.read(), None);
        inbox.post("start translation");
        assert_eq!(inbox.read(), Some("start translation"));
    }
}
