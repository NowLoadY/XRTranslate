//! Bounded host-output composition. Content expiry is independent of connection control.
use super::{
    connectors::Settings, display::TextDisplay, osc_caption::Layout, websocket::SenderHandle,
};
use crate::{
    client_settings::CaptureSource,
    plugins::osc::transport::OscTransportHandle,
    plugins::text_output::format_message,
    session_coordinator::{CaptionUpdate, HostOutputEvent, HostOutputSubscriber},
};
use parking_lot::Mutex;
use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, Instant},
};
use xrt_connector_core::{DisplayMessage, DisplaySnapshot};

struct Entry {
    stream: u64,
    id: String,
    source: String,
    target: String,
    additional: Vec<String>,
    speaker: String,
    audio_source: CaptureSource,
    is_typing: bool,
    live: bool,
    preview: bool,
    expires: Instant,
}
#[derive(Default)]
struct State {
    display: TextDisplay,
    settings: Settings,
    enabled: bool,
    entries: VecDeque<Entry>,
    sequence: u64,
    dirty: bool,
    sent: Option<Instant>,
}
impl State {
    fn snapshot(&self, now: Instant) -> DisplaySnapshot {
        let messages = if self.settings.messages {
            self.entries
                .iter()
                .filter(|entry| {
                    entry.expires > now
                        && (!self.display.prefers_completed_text() || !entry.live)
                        && self
                            .settings
                            .audio_source_filter
                            .allows(entry.audio_source, entry.is_typing)
                })
                .filter_map(|entry| {
                    let text = format_message(
                        &entry.source,
                        &entry.target,
                        entry.additional.iter().map(String::as_str),
                        &entry.speaker,
                        self.settings.format,
                    );
                    (!text.trim().is_empty()).then(|| DisplayMessage {
                        id: entry.id.clone(),
                        text,
                        ttl_ms: entry.expires.saturating_duration_since(now).as_millis() as u64,
                    })
                })
                .collect()
        } else {
            Vec::new()
        };
        DisplaySnapshot {
            header: Some(self.settings.header.clone()),
            messages,
            footer: Some(self.settings.footer.clone()),
        }
        .normalized()
    }
}
#[derive(Clone, Default)]
pub(crate) struct DisplayOutput(Arc<Mutex<State>>);
impl DisplayOutput {
    pub(super) fn configure(&self, settings: &Settings, enabled: bool) {
        let mut state = self.0.lock();
        if state.enabled && !enabled {
            state.display.clear();
            state.entries.clear();
        }
        if state.settings != *settings || state.enabled != enabled {
            state.dirty = true;
        }
        state.settings = settings.clone();
        state.enabled = enabled;
    }
    pub(super) fn bind(&self, sender: SenderHandle, session: String, structured: bool) {
        let mut state = self.0.lock();
        state.display.bind(sender, session, structured);
        state.dirty = true;
    }
    pub(super) fn bind_osc(
        &self,
        transport: OscTransportHandle,
        epoch: u64,
        layout: Option<Layout>,
    ) {
        let mut state = self.0.lock();
        state.dirty |= state.display.bind_osc(transport, epoch, layout);
    }
    pub(super) fn available(&self) -> bool {
        self.0.lock().display.available()
    }
    pub(super) fn disconnect(&self) {
        let mut state = self.0.lock();
        state.display.disconnect();
        state.entries.clear();
        state.dirty = true;
    }
    pub(super) fn clear_messages(&self) {
        let mut state = self.0.lock();
        state.entries.clear();
        state.dirty = true;
    }
    pub(super) fn preview(&self) -> DisplaySnapshot {
        let state = self.0.lock();
        state.display.project(&state.snapshot(Instant::now()))
    }
    pub(super) fn poll(&self) -> Result<(), String> {
        let mut state = self.0.lock();
        let now = Instant::now();
        let count = state.entries.len();
        state.entries.retain(|entry| entry.expires > now);
        state.dirty |= count != state.entries.len();
        if state.enabled
            && state.display.available()
            && (state.dirty
                || state
                    .sent
                    .is_none_or(|sent| now.duration_since(sent) >= Duration::from_secs(4)))
        {
            let snapshot = state.snapshot(now);
            state.display.replace(&snapshot);
            state.dirty = false;
            state.sent = Some(now);
        }
        state.display.poll()
    }
}
impl HostOutputSubscriber for DisplayOutput {
    fn on_host_output(&self, event: HostOutputEvent<'_>) {
        let mut state = self.0.lock();
        if !state.enabled {
            return;
        }
        let expires =
            Instant::now() + Duration::from_secs(state.settings.message_seconds.clamp(1, 15));
        match event {
            HostOutputEvent::Caption {
                stream_id,
                audio_source,
                is_typing,
                source,
                translated,
                additional_translations,
                asr_only,
                speaker,
                update,
            } => {
                if !state
                    .settings
                    .audio_source_filter
                    .allows(audio_source, is_typing)
                {
                    return;
                }
                // A recognition-only update must not evict a visible translation.
                if translated.is_empty() && !asr_only {
                    return;
                }
                let translated = if asr_only && translated.is_empty() {
                    source
                } else {
                    translated
                };
                let source = if asr_only { "" } else { source };
                state
                    .entries
                    .retain(|entry| entry.stream != stream_id || !entry.preview);
                let live = matches!(update, CaptionUpdate::Replace);
                let previous = state
                    .entries
                    .iter()
                    .rposition(|entry| entry.stream == stream_id && entry.live);
                if let Some(index) = previous {
                    state.entries.remove(index);
                }
                state.sequence += 1;
                let entry = Entry {
                    stream: stream_id,
                    id: format!("{stream_id}:{}", state.sequence),
                    source: source.chars().take(1024).collect(),
                    target: translated.chars().take(1024).collect(),
                    additional: additional_translations
                        .iter()
                        .take(4)
                        .map(|translation| translation.translated_text.chars().take(1024).collect())
                        .collect(),
                    speaker: speaker.chars().take(64).collect(),
                    audio_source,
                    is_typing,
                    live,
                    preview: false,
                    expires,
                };
                state.entries.push_back(entry);
            }
            HostOutputEvent::Previews {
                stream_id,
                is_typing,
                entries,
            } => {
                if state.display.prefers_completed_text() {
                    return;
                }
                state
                    .entries
                    .retain(|entry| entry.stream != stream_id || !entry.preview);
                for preview in entries.iter().filter(|preview| {
                    preview
                        .translated
                        .as_ref()
                        .is_some_and(|text| !text.is_empty())
                }) {
                    if !state
                        .settings
                        .audio_source_filter
                        .allows(preview.audio_source, is_typing)
                    {
                        continue;
                    }
                    let entry = Entry {
                        stream: stream_id,
                        id: format!("{stream_id}:{}:{}", preview.turn_id, preview.segment_index),
                        source: preview.source.chars().take(1024).collect(),
                        target: preview
                            .translated
                            .as_deref()
                            .unwrap_or_default()
                            .chars()
                            .take(1024)
                            .collect(),
                        additional: Vec::new(),
                        speaker: String::new(),
                        audio_source: preview.audio_source,
                        is_typing,
                        live: true,
                        preview: true,
                        expires,
                    };
                    state.entries.push_back(entry);
                }
            }
            HostOutputEvent::CommittedTranslation {
                stream_id,
                translated,
                ..
            } => {
                let translated: String = translated.chars().take(1024).collect();
                for entry in &mut state.entries {
                    if entry.stream == stream_id
                        && entry.live
                        && !entry.preview
                        && entry.target == translated
                    {
                        entry.live = false;
                        entry.expires = expires;
                    }
                }
            }
            HostOutputEvent::StreamCancelled(stream) => {
                state.entries.retain(|entry| entry.stream != stream);
            }
            HostOutputEvent::StreamEnded(stream) => {
                state
                    .entries
                    .retain(|entry| entry.stream != stream || !entry.preview);
                for entry in &mut state.entries {
                    if entry.stream == stream {
                        entry.live = false;
                    }
                }
            }
            HostOutputEvent::Clear => {
                state.entries.clear();
            }
        }
        while state.entries.len() > 8 {
            state.entries.pop_front();
        }
        state.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_coordinator::TranslationPreview;

    #[test]
    fn normalized_output_preserves_finality_previews_and_source_filtering() {
        let output = DisplayOutput::default();
        let mut settings = Settings::default();
        settings.audio_source_filter.system_audio = false;
        output.configure(&settings, true);
        let caption = |stream, source, target, asr_only, audio_source| HostOutputEvent::Caption {
            stream_id: stream,
            source,
            translated: target,
            asr_only,
            audio_source,
            additional_translations: &[],
            speaker: "",
            is_typing: false,
            update: CaptionUpdate::Replace,
        };
        output.on_host_output(caption(
            1,
            "Hello",
            "Bonjour",
            false,
            CaptureSource::Microphone,
        ));
        assert!(output.0.lock().entries[0].live);
        output.on_host_output(HostOutputEvent::CommittedTranslation {
            stream_id: 1,
            turn_id: "first",
            segment_index: 1,
            audio_source: CaptureSource::Microphone,
            is_typing: false,
            source: "Hello",
            translated: "Bonjour",
            additional_translations: &[],
            speaker: "",
            asr_only: false,
        });
        assert!(!output.0.lock().entries[0].live);
        for stream in 2..12 {
            output.on_host_output(caption(
                stream,
                "Noise",
                "Bruit",
                false,
                CaptureSource::SystemAudio,
            ));
        }
        assert_eq!(output.preview().messages.len(), 1);
        let previews = [TranslationPreview {
            stream_id: 12,
            audio_source: CaptureSource::Microphone,
            turn_id: "preview".into(),
            segment_index: 1,
            source: "Wait".into(),
            translated: Some("Attends".into()),
            revision: 1,
        }];
        output.on_host_output(HostOutputEvent::Previews {
            stream_id: 12,
            is_typing: false,
            entries: &previews,
        });
        assert_eq!(output.preview().messages.len(), 2);
        output.on_host_output(HostOutputEvent::Previews {
            stream_id: 12,
            is_typing: false,
            entries: &[],
        });
        assert_eq!(output.preview().messages[0].text, "Bonjour");
        output.on_host_output(HostOutputEvent::Previews {
            stream_id: 12,
            is_typing: false,
            entries: &previews,
        });
        output.on_host_output(HostOutputEvent::StreamEnded(12));
        assert_eq!(output.preview().messages.len(), 1);
        output.on_host_output(caption(
            13,
            "Recognized",
            "",
            true,
            CaptureSource::Microphone,
        ));
        assert_eq!(output.preview().messages.last().unwrap().text, "Recognized");
        output.on_host_output(HostOutputEvent::Clear);
        assert!(output.preview().messages.is_empty());

        settings = serde_json::from_value(serde_json::json!({
            "header": {"content_type": "CustomText", "custom_text": "Hello", "show_device_name": false},
            "footer": {"content_type": "GpuStatus", "custom_text": "old", "show_device_name": true}
        })).unwrap();
        assert_eq!(settings.header, "Hello");
        assert!(settings.footer.is_empty());
        settings.footer = "Welcome".into();
        settings.messages = false;
        settings.audio_source_filter.microphone = false;
        settings.audio_source_filter.system_audio = false;
        output.configure(&settings, true);
        let preview = output.preview();
        assert_eq!(preview.header.as_deref(), Some("Hello"));
        assert_eq!(preview.footer.as_deref(), Some("Welcome"));
        assert!(preview.messages.is_empty());
        assert_eq!(
            serde_json::from_value::<Settings>(serde_json::to_value(&settings).unwrap()).unwrap(),
            settings
        );
    }
}
