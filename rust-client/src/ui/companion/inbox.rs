//! Transient reminders wait for the companion to be ready to read them.
use crate::i18n::{self, UiLanguage};
use std::{borrow::Cow, collections::VecDeque};

const MAX_ERROR_SOURCES: usize = 4;
const MAX_PENDING_ERRORS: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Message {
    Localized(&'static str),
    Error(String),
}

impl Message {
    pub(super) fn text(&self, language: UiLanguage) -> Cow<'static, str> {
        match self {
            Self::Localized(key) => Cow::Borrowed(i18n::tr(language, key)),
            Self::Error(detail) => Cow::Owned(format!(
                "{}: {detail}",
                i18n::tr(language, "Something went wrong")
            )),
        }
    }
}

impl From<&'static str> for Message {
    fn from(key: &'static str) -> Self {
        Self::Localized(key)
    }
}

#[derive(Default)]
pub(crate) struct Inbox {
    letters: VecDeque<Message>,
    reading: Option<Message>,
    observed_errors: VecDeque<(&'static str, String)>,
}

impl Inbox {
    pub(crate) fn post(&mut self, message: &'static str) {
        self.enqueue(message.into());
    }

    /// Polling a persistent failure must not repeat it after the bubble finishes.
    /// A cleared error allows a later failure to deliver a new letter.
    pub(crate) fn observe_error(&mut self, source: &'static str, error: Option<&str>) {
        let error = error.map(str::trim).filter(|error| !error.is_empty());
        if let Some(index) = self
            .observed_errors
            .iter()
            .position(|(observed_source, _)| *observed_source == source)
        {
            if error == Some(self.observed_errors[index].1.as_str()) {
                return;
            }
            self.observed_errors.remove(index);
        }
        if let Some(error) = error {
            // Session, text, application and OCR failures have independent
            // lifetimes. Bound the cache even if another source is added.
            if self.observed_errors.len() == MAX_ERROR_SOURCES {
                self.observed_errors.pop_front();
            }
            self.observed_errors.push_back((source, error.to_owned()));
            self.enqueue(Message::Error(error.to_owned()));
        }
    }

    fn enqueue(&mut self, message: Message) {
        if self.reading.as_ref() != Some(&message) && !self.letters.contains(&message) {
            if matches!(message, Message::Error(_))
                && self
                    .letters
                    .iter()
                    .filter(|letter| matches!(letter, Message::Error(_)))
                    .count()
                    >= MAX_PENDING_ERRORS
                && let Some(index) = self
                    .letters
                    .iter()
                    .position(|letter| matches!(letter, Message::Error(_)))
            {
                self.letters.remove(index);
            }
            self.letters.push_back(message);
        }
    }

    pub(super) fn read(&mut self) -> Option<Message> {
        // The companion calls this only after finishing its sentence and cooldown.
        self.reading = self.letters.pop_front();
        self.reading.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeats_are_merged_while_the_companion_is_reading_the_letter() {
        let mut inbox = Inbox::default();
        inbox.post("reminder");
        assert_eq!(inbox.read(), Some("reminder".into()));
        inbox.post("reminder");
        assert_eq!(inbox.read(), None);
        inbox.post("reminder");
        assert_eq!(inbox.read(), Some("reminder".into()));
    }

    #[test]
    fn mail_is_ordered_and_repeated_pending_reminders_are_merged() {
        let mut inbox = Inbox::default();
        inbox.post("start translation");
        inbox.post("start microphone");
        inbox.post("start translation");
        assert_eq!(inbox.read(), Some("start translation".into()));
        assert_eq!(inbox.read(), Some("start microphone".into()));
        assert_eq!(inbox.read(), None);
        inbox.post("start translation");
        assert_eq!(inbox.read(), Some("start translation".into()));
    }

    #[test]
    fn errors_are_reported_once_per_source_until_that_source_recovers() {
        let mut inbox = Inbox::default();
        inbox.observe_error("session", Some("Connection lost"));
        inbox.observe_error("application", Some("Microphone unavailable"));
        inbox.observe_error("ocr", Some("Text recognition failed"));
        inbox.observe_error("text", None);
        assert_eq!(inbox.read(), Some(Message::Error("Connection lost".into())));
        assert_eq!(
            inbox.read(),
            Some(Message::Error("Microphone unavailable".into()))
        );
        assert_eq!(
            inbox.read(),
            Some(Message::Error("Text recognition failed".into()))
        );
        assert_eq!(inbox.read(), None);
        inbox.observe_error("session", None);
        inbox.observe_error("application", Some("Microphone unavailable"));
        inbox.observe_error("ocr", Some("Text recognition failed"));
        assert_eq!(inbox.read(), None);
        inbox.observe_error("session", Some("Connection lost"));
        assert_eq!(inbox.read(), Some(Message::Error("Connection lost".into())));
    }
}
