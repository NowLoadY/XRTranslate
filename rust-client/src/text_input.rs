//! Shared text-input state and actions for the app, plugins, and floating window.
use eframe::egui;
use serde_json::Value;

impl crate::XRTranslateApp {
    pub(crate) fn text_input_budget(&self) -> Option<usize> {
        self.service_config.translation_input_budget(
            &self.prompt_library.active_graph(),
            &self.source_lang,
            &self.target_lang,
        )
    }

    pub(crate) fn copy_text_translation(&mut self, ctx: &egui::Context) {
        if !self.text_translation.busy_host()
            && let Some(text) = self.text_result.take()
        {
            ctx.copy_text(text);
        }
    }

    pub(crate) fn dismiss_text_result(&mut self) {
        self.text_result = None;
        self.text_result_dismissed = true;
    }

    pub(crate) fn text_input_state(&self) -> xrtranslate_client_core::services::TextState {
        let input_budget = self.text_input_budget();
        let recognition = self
            .shared_session_state
            .lock()
            .map(|mut state| {
                state.text_collection.set_budget(input_budget);
                state.text_collection.input.clone()
            })
            .unwrap_or_default();
        let preparing = self.text_translation.busy_host();
        xrtranslate_client_core::services::TextState {
            preparing,
            input_budget,
            can_copy: self.text_result.is_some() && !preparing,
            recognition: Some(recognition),
            asr_only: self.asr_only,
            additional_target: self.additional_target_lang.clone(),
            microphone_sync: self.microphone_sync_enabled,
        }
    }

    pub(crate) fn copy_recognized_text(&self, ctx: &egui::Context) {
        if let Ok(state) = self.shared_session_state.lock() {
            let text = state.text_collection.original();
            if !text.is_empty() {
                ctx.copy_text(text);
            }
        }
    }

    pub(crate) fn collect_recognition(&mut self, enabled: bool, text: String) {
        self.dismiss_text_result();
        let budget = self.text_input_budget();
        let completed = self.shared_session_state.lock().ok().and_then(|mut state| {
            if enabled {
                state.text_collection.start(text);
                state.text_collection.set_budget(budget);
                None
            } else {
                state.text_collection.stop()
            }
        });
        if let Some(text) = completed {
            self.submit_text_translation(&text, None, None, None);
        }
    }

    pub(crate) fn handle_text_action(
        &mut self,
        action: xrtranslate_client_core::services::TextAction,
        ctx: &egui::Context,
    ) -> xrtranslate_plugin::Result {
        use xrtranslate_client_core::services::TextAction;
        match action {
            TextAction::Clear => self.clear_history(),
            TextAction::Copy => self.copy_text_translation(ctx),
            TextAction::DismissCopy => self.dismiss_text_result(),
            TextAction::CopyRecognition => self.copy_recognized_text(ctx),
            TextAction::CollectRecognition { enabled, text } => {
                self.collect_recognition(enabled, text);
            }
            TextAction::Submit { text } => {
                if !self.submit_text_translation(&text, None, None, None) {
                    return Err("Text translation is unavailable".into());
                }
            }
            TextAction::Language {
                source,
                target,
                asr_only,
            } => {
                self.select_language_options(
                    &source,
                    &target,
                    self.additional_target_lang.clone().as_deref(),
                    asr_only,
                )
                .ok_or("Language route is unavailable")?;
                self.source_lang = source;
                self.target_lang = target;
                self.asr_only = asr_only;
                self.apply_language_route();
            }
            TextAction::MicrophoneSync { enabled } => self.set_microphone_sync_enabled(enabled),
        }
        self.refresh_plugin_context();
        Ok(Value::Null)
    }
}
