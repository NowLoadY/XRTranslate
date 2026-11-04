//! Application adapter for media tasks; providers and audio execution remain shared.
use crate::{
    CaptureSource, XRTranslateApp,
    plugins::{self, PluginId},
    session_coordinator::{TranslationInput, TranslationSessionPlugin, TranslationTask},
};
use eframe::egui;

impl XRTranslateApp {
    pub(crate) fn apply_media_import_event(
        &mut self,
        owner: &crate::session_coordinator::TranslationSessionOwner,
        event: crate::media_import::AudioImportEvent,
    ) {
        use crate::{
            media_import,
            session_coordinator::{TranslationEvent, TranslationOutcome},
        };
        if let Some(operation) = owner
            .operation_id()
            .filter(|id| self.media_plugin.is_workspace_operation(id))
        {
            if let media_import::AudioImportEvent::Error(error) = event {
                self.media_plugin.accept_workspace_event(
                    operation,
                    &TranslationEvent::Finished {
                        stream_id: 0,
                        outcome: TranslationOutcome::Failed(error),
                    },
                );
                self.stop_task_owner(owner);
            }
            return;
        }
        if owner.operation_id() != self.media_plugin.controller.active_task_id.as_deref() {
            return;
        }
        match event {
            media_import::AudioImportEvent::Started(info) => {
                self.set_connection_status(format!(
                    "Processing media audio ({} Hz)",
                    info.source_sample_rate
                ));
            }
            media_import::AudioImportEvent::Progress(progress) => {
                let percentage = progress.fraction.map(|value| value * 100.0);
                match progress.stage {
                    media_import::AudioImportStage::Extracting => {
                        self.media_plugin.update_import_progress(
                            plugins::media::ImportProgressStage::Extracting,
                            progress.fraction,
                            progress.position,
                            progress.duration,
                        );
                        self.set_connection_status(percentage.map_or_else(
                            || format!("Extracting media audio · {}s", progress.position.as_secs()),
                            |value| format!("Extracting media audio · {value:.0}%"),
                        ));
                    }
                    media_import::AudioImportStage::Recognizing => {
                        self.media_plugin.update_import_progress(
                            plugins::media::ImportProgressStage::Recognizing,
                            progress.fraction,
                            progress.position,
                            progress.duration,
                        );
                        self.set_connection_status(percentage.map_or_else(
                            || {
                                format!(
                                    "Transcribing media audio · {}s",
                                    progress.position.as_secs()
                                )
                            },
                            |value| format!("Transcribing media audio · {value:.0}%"),
                        ));
                    }
                }
            }
            media_import::AudioImportEvent::Completed { .. } => {
                self.media_plugin.complete_import();
                self.set_connection_status("Finishing media translation");
            }
            media_import::AudioImportEvent::Stopped { .. }
            | media_import::AudioImportEvent::OutputClosed => {
                // Session events report downstream failures; do not replace or cancel them.
                self.media_plugin.stop_import();
            }
            media_import::AudioImportEvent::Error(error) => {
                log::error!("Host audio import error: {error}");
                self.media_plugin.stop_import();
                self.media_plugin.set_error(error.clone());
                self.last_error = Some(error);
                self.stop_plugin_task(PluginId::MEDIA);
            }
        }
    }

    pub(crate) fn stop_media_agent(&mut self) {
        let owns_run = self
            .agent
            .scope()
            .and_then(|scope| scope["workspace"].as_str())
            .is_some_and(|id| self.media_plugin.workspace_task(id).is_some());
        if owns_run {
            self.stop_agent("Media task stopped");
        }
    }

    pub(crate) fn refresh_media_director(&mut self) {
        let models = self.audio_preprocessing_models();
        self.media_plugin.available_preprocessing = [
            xrtranslate_protocol::AudioPreprocessing::None,
            xrtranslate_protocol::AudioPreprocessing::Gtcrn,
            xrtranslate_protocol::AudioPreprocessing::VocalSeparation,
        ]
        .into_iter()
        .filter(|choice| models.is_ready(*choice))
        .collect();
        self.media_plugin.default_preprocessing = self.service_config.default_audio_preprocessing();
    }

    pub(crate) fn start_media_agent(&mut self, task: &str) -> Result<(), String> {
        self.prepare_agent()?;
        let (scope, goal) = self.media_plugin.prepare_optimization(task)?;
        self.agent.start(scope, goal)?;
        self.companion_inbox.post("I'm improving the translation.");
        Ok(())
    }

    pub(crate) fn poll_media_translation(&mut self, ctx: &egui::Context) {
        self.media_plugin.poll_translation_events();
        for owner in std::mem::take(&mut self.media_plugin.workspace_cancelled) {
            self.stop_task_owner(&owner);
        }
        if let Some(task) = self.media_plugin.next_workspace_task() {
            self.start_translation_task(task, Some(ctx.clone()));
        }
        self.media_plugin.poll_subtitle_processing(
            self.service_config.supports_ask(),
            self.service_config.agent_available() && self.plugin_enabled(PluginId::AGENT),
            |prompt| self.ask.request(prompt, ctx),
            ctx,
        );
        if let Some(owner) = self.media_plugin.take_cancelled_retranslation() {
            self.cancel_text_tasks(Some(&owner));
            self.cancel_pending_tasks(Some(&owner));
        }
        if let Some(task) = self.media_plugin.next_subtitle_task() {
            self.start_translation_task(task, Some(ctx.clone()));
        }
        if self.media_plugin.take_stop_request() {
            self.stop_plugin_task(PluginId::MEDIA);
        }
        if !self.agent.active()
            && let Some(task) = self.media_plugin.pending_optimization.take()
        {
            if self.plugin_enabled(PluginId::AGENT) && self.service_config.agent_available() {
                if let Err(error) = self.start_media_agent(&task) {
                    self.media_plugin.set_error(error);
                }
            }
        }
    }

    pub(crate) fn render_media_plugin_page(&mut self, ui: &mut egui::Ui) {
        let preprocessing_models = self.audio_preprocessing_models();
        let snapshot = plugins::media::MediaUiSnapshot {
            download_proxy_url: self.download_proxy_url.clone(),
            language: self.ui_language,
            languages: self.language_capabilities(),
            can_optimize: self.service_config.agent_available()
                && self.plugin_enabled(PluginId::AGENT),
            optimizing_task: self
                .agent
                .scope()
                .and_then(|scope| scope["workspace"].as_str())
                .and_then(|id| self.media_plugin.workspace_task(id))
                .map(|task| task.id.clone()),
            default_preprocessing: self.service_config.default_audio_preprocessing(),
            preprocessing_models,
        };
        let action = self.media_plugin.render_page(&snapshot, ui);
        self.apply_media_action(action, ui.ctx().clone());
    }

    pub(crate) fn apply_media_action(
        &mut self,
        action: plugins::media::MediaAction,
        ctx: egui::Context,
    ) {
        match action {
            plugins::media::MediaAction::OptimizeSubtitles => {
                if let Some(task) = self.media_plugin.controller.active_task_id.clone() {
                    if let Err(error) = self.start_media_agent(&task) {
                        self.media_plugin.set_error(error);
                    }
                }
            }
            plugins::media::MediaAction::StopOptimization => self.stop_agent("Stopped by user"),
            plugins::media::MediaAction::None => {}
            plugins::media::MediaAction::AudioPreprocessing(action) => {
                self.apply_audio_preprocessing_action(action);
            }
            plugins::media::MediaAction::StopTranslation => {
                self.stop_plugin_task(PluginId::MEDIA);
            }
            plugins::media::MediaAction::TranslateSubtitles { restart } => {
                self.stop_plugin_task(PluginId::MEDIA);
                let capabilities = self.language_capabilities();
                if let Err(error) = self.media_plugin.start_subtitles(
                    capabilities,
                    restart,
                    self.service_config.agent_available() && self.plugin_enabled(PluginId::AGENT),
                ) {
                    self.media_plugin.set_error(error);
                }
            }
            plugins::media::MediaAction::RetranslateSubtitle => {
                let capabilities = self.language_capabilities();
                match self.media_plugin.retranslate_subtitle(capabilities) {
                    Ok(task) => {
                        self.start_translation_task(task, Some(ctx));
                    }
                    Err(error) => self.media_plugin.set_error(error),
                }
            }
            plugins::media::MediaAction::StartTranslation { request } => {
                use plugins::media::MediaTranslationRequest;
                let (source, target) = match &request {
                    MediaTranslationRequest::ImportMedia {
                        source_language,
                        target_language,
                        ..
                    }
                    | MediaTranslationRequest::LiveStream {
                        source_language,
                        target_language,
                        ..
                    } => (source_language, target_language),
                };
                let Some(mut languages) = self.select_languages(source, target) else {
                    return;
                };
                let recognition = match &request {
                    MediaTranslationRequest::ImportMedia { recognition, .. }
                    | MediaTranslationRequest::LiveStream { recognition, .. } => recognition,
                };
                if !self.audio_preprocessing_ready(recognition.preprocessing) {
                    self.media_plugin.set_error(crate::i18n::tr(
                        self.ui_language,
                        "Download every required model package to continue.",
                    ));
                    return;
                }
                self.stop_plugin_task(PluginId::MEDIA);
                let previous_recognition = self.loopback_recognition.clone();
                let input = match request {
                    request @ MediaTranslationRequest::ImportMedia { .. } => {
                        if let Err(error) = self.media_plugin.start_file_subtitles(
                            languages,
                            self.service_config.supports_ask(),
                            self.service_config.agent_available()
                                && self.plugin_enabled(PluginId::AGENT),
                        ) {
                            self.media_plugin.set_error(error);
                            return;
                        }
                        let (source, target) = languages.wire();
                        languages =
                            xrtranslate_engine::language::LanguageSelection::parse_with_options(
                                &source, &target, None, true,
                            )
                            .expect("validated recognition languages");
                        match request
                            .import_input(self.service_config.default_audio_preprocessing(), None)
                        {
                            Ok(input) => input,
                            Err(error) => {
                                self.media_plugin.set_error(error);
                                return;
                            }
                        }
                    }
                    MediaTranslationRequest::LiveStream { recognition, .. } => {
                        self.media_plugin.controller.subtitles.clear();
                        self.media_plugin.controller.start_task();
                        self.loopback_recognition = recognition;
                        TranslationInput::Live(CaptureSource::SystemAudio)
                    }
                };
                self.start_translation_task(
                    TranslationTask {
                        languages,
                        plugin: self.media_plugin.translation_session_binding(),
                        context: self.media_plugin.translation_context(),
                        text_workload: None,
                        input,
                        profiles: Vec::new(),
                    },
                    Some(ctx),
                );
                self.loopback_recognition = previous_recognition;
            }
        }
    }
}
