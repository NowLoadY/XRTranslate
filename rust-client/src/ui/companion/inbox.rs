//! Transient reminders wait for the companion to be ready to read them.
use crate::i18n::{self, UiLanguage};
use std::{borrow::Cow, collections::VecDeque};

const MAX_ERROR_SOURCES: usize = 4;
const MAX_PENDING_ERRORS: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Message {
    Localized(&'static str),
    Error(String),
    Text { source: String, text: String },
    Command(super::Command),
    Tutorial { revision: u64, text: String },
}

impl Message {
    pub(super) fn text(&self, language: UiLanguage) -> Cow<'static, str> {
        match self {
            Self::Localized(key) => Cow::Borrowed(i18n::tr(language, key)),
            Self::Tutorial { text, .. } => Cow::Owned(text.clone()),
            Self::Command(command) => Cow::Borrowed(command.reply(language)),
            Self::Text { text, .. } => Cow::Owned(text.clone()),
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
    acknowledgement: Option<super::Command>,
    tutorial: Option<Message>,
}

impl Inbox {
    /// One replaceable priority slot: skipping never leaves old tutorial mail.
    pub(crate) fn guide(&mut self, revision: u64, text: impl Into<String>) {
        self.tutorial = Some(Message::Tutorial {
            revision,
            text: text.into(),
        });
    }

    pub(crate) fn end_guide(&mut self) {
        self.tutorial = None;
    }

    pub(super) fn tutorial(&self) -> Option<&Message> {
        self.tutorial.as_ref()
    }

    pub(crate) fn acknowledge(&mut self, command: super::Command) {
        // Immediate feedback for the latest action, never a backlog of stale replies.
        self.acknowledgement = Some(command);
    }

    pub(super) fn read_acknowledgement(&mut self) -> Option<Message> {
        self.acknowledgement.take().map(Message::Command)
    }

    pub(crate) fn post(&mut self, message: &'static str) {
        self.enqueue(message.into());
    }

    /// Keep the latest update from a producer, rather than a backlog of stale progress.
    pub(crate) fn post_text(&mut self, source: &str, text: String) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        self.letters.retain(|message| !matches!(message, Message::Text { source: previous, .. } if previous == source));
        self.enqueue(Message::Text {
            source: source.to_owned(),
            text: text.to_owned(),
        });
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
    fn dynamic_progress_keeps_latest_update_and_existing_mail() {
        let mut inbox = Inbox::default();
        inbox.post("Welcome");
        inbox.post_text("agent", "正在检查。".into());
        inbox.post_text("agent", "检查完成。".into());
        assert_eq!(inbox.read(), Some(Message::Localized("Welcome")));
        assert_eq!(
            inbox.read().unwrap().text(UiLanguage::Chinese),
            "检查完成。"
        );
        inbox.post_text("agent", "检查完成。".into());
        assert!(
            inbox.read().is_none(),
            "do not repeat a message currently being read"
        );
    }
}
