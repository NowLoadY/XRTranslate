use crossbeam_channel::{Receiver, bounded};
#[path = "windows.rs"]
mod windows;
use super::{Settings, clipboard_text};
#[derive(Clone, Copy)]
struct Shortcut {
    control: bool,
    alt: bool,
    shift: bool,
    key: u8,
}
impl Shortcut {
    fn parse(text: &str) -> Result<Self, String> {
        let mut result = Self {
            control: false,
            alt: false,
            shift: false,
            key: 0,
        };
        for part in text.split('+').map(str::trim) {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => result.control = true,
                "alt" => result.alt = true,
                "shift" => result.shift = true,
                key if key.len() == 1
                    && key.as_bytes()[0].is_ascii_alphanumeric()
                    && result.key == 0 =>
                {
                    result.key = key.as_bytes()[0].to_ascii_uppercase()
                }
                _ => return Err("Use Ctrl, Alt or Shift with one letter or digit.".into()),
            }
        }
        if result.key == 0 || !(result.control || result.alt) {
            return Err("Use Ctrl or Alt with one letter or digit.".into());
        }
        Ok(result)
    }
}

pub(crate) struct Listener {
    pub events: Receiver<Event>,
    cancel: Option<Box<dyn FnOnce() + Send>>,
    worker: Option<std::thread::JoinHandle<()>>,
}
pub(crate) enum Event {
    Captured(Result<String, String>),
    Unavailable(String),
}
type Emit = std::sync::Arc<dyn Fn(Event) + Send + Sync>;
impl Listener {
    pub fn start(settings: &Settings, ctx: &eframe::egui::Context) -> Result<Self, String> {
        let key = Shortcut::parse(&settings.shortcut)?;
        let (tx, events) = bounded(4);
        let ctx = ctx.clone();
        let emit: Emit = std::sync::Arc::new(move |result| {
            let _ = tx.try_send(result);
            ctx.request_repaint();
        });
        let (cancel, worker) = windows::start(key, emit)?;
        Ok(Self {
            events,
            cancel: Some(cancel),
            worker: Some(worker),
        })
    }
}
impl Drop for Listener {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn selected_text() -> Result<String, String> {
    let selection = windows::selection();
    let text = match selection.filter(|text| !text.trim().is_empty()) {
        Some(text) => text,
        None => clipboard_text()?,
    };
    if text.trim().is_empty() {
        return Err("Select or copy text, then use the shortcut again.".into());
    }
    if text.chars().count() > 64_000 {
        return Err("Select a shorter passage to translate.".into());
    }
    Ok(text.trim().to_owned())
}
