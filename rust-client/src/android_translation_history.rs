//! Completed output adapter; widget storage and rendering belong to Android.
use jni::objects::JValue;
use std::{collections::VecDeque, sync::Mutex};

use crate::session_coordinator::{CaptionUpdate, HostOutputEvent, HostOutputSubscriber};

#[derive(Default)]
pub(crate) struct ResultSubscriber {
    pending: Mutex<VecDeque<(u64, String, String)>>,
}

impl HostOutputSubscriber for ResultSubscriber {
    fn on_host_output(&self, event: HostOutputEvent<'_>) {
        match event {
            HostOutputEvent::Caption {
                stream_id,
                source,
                translated,
                update: CaptionUpdate::Replace,
                ..
            } => {
                let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
                pending.retain(|(stream, _, _)| *stream != stream_id);
                if !translated.trim().is_empty() {
                    if pending.len() == 32 {
                        pending.pop_front();
                    }
                    pending.push_back((stream_id, source.to_owned(), translated.to_owned()));
                }
            }
            HostOutputEvent::Caption {
                stream_id,
                source,
                translated,
                ..
            } => {
                self.pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .retain(|(stream, _, _)| *stream != stream_id);
                publish(source, translated);
            }
            HostOutputEvent::StreamEnded(stream_id) => {
                let completed = {
                    let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
                    pending
                        .iter()
                        .position(|(stream, _, _)| *stream == stream_id)
                        .and_then(|index| pending.remove(index))
                };
                if let Some((_, source, translated)) = completed {
                    publish(&source, &translated);
                }
            }
            HostOutputEvent::Clear => self
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clear(),
        }
    }
}

pub(crate) fn publish(source: &str, translated: &str) {
    if translated.trim().is_empty() {
        return;
    }
    let _ = crate::android::with_activity(|env, activity| {
        env.with_local_frame(2, |env| {
            let source = env.new_string(source)?;
            let translated = env.new_string(translated)?;
            env.call_method(
                activity,
                "publishTranslationResult",
                "(Ljava/lang/String;Ljava/lang/String;)V",
                &[JValue::Object(&source), JValue::Object(&translated)],
            )?;
            Ok::<_, jni::errors::Error>(())
        })
    });
}
