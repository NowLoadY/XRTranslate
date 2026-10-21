//! Application adapter for media tasks; providers and audio execution remain shared.
use crate::{
    CaptureSource, XRTranslateApp, media_import,
    plugins::{self, PluginId},
    session_coordinator::{TranslationInput, TranslationSessionPlugin, TranslationTask},
};
use eframe::egui;

impl XRTranslateApp {
    pub(crate) fn poll_media_translation(&mut self, ctx: &egui::Context) {
        self.media_plugin.poll_translation_events();
        self.media_plugin
            .poll_subtitle_refinement(&mut self.ask, ctx);
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
    }

    pub(crate) fn render_media_plugin_page(&mut self, ui: &mut egui::Ui) {
        let preprocessing_models = self.audio_preprocessing_models();
        let snapshot = plugins::media::MediaUiSnapshot {
            language: self.ui_language,
            languages: self.language_capabilities(),
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
                if let Err(error) = self.media_plugin.start_subtitles(capabilities, restart) {
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
                    MediaTranslationRequest::ImportMediaFile {
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
                    MediaTranslationRequest::ImportMediaFile { recognition, .. }
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
                    MediaTranslationRequest::ImportMediaFile {
                        path,
                        mut recognition,
                        audio_channels,
                        ..
                    } => {
                        let preprocessing = recognition
                            .preprocessing
                            .unwrap_or(self.service_config.default_audio_preprocessing());
                        if let Err(error) = self
                            .media_plugin
                            .start_file_subtitles(languages, self.service_config.supports_ask())
                        {
                            self.media_plugin.set_error(error);
                            return;
                        }
                        let (source, target) = languages.wire();
                        languages =
                            xrtranslate_engine::language::LanguageSelection::parse_with_options(
                                &source, &target, None, true,
                            )
                            .expect("validated recognition languages");
                        // File quality uses native delivery: complete utterances for
                        // non-streaming ASR, native streaming where supported.
                        recognition.continuous_recognition = false;
                        TranslationInput::File {
                            path,
                            recognition,
                            options: media_import::AudioImportOptions {
                                recognition_channels: audio_channels
                                    .iter()
                                    .filter(|channel| channel.recognition)
                                    .map(|channel| channel.index)
                                    .collect(),
                                ..media_import::AudioImportOptions::for_recognition(preprocessing)
                            },
                        }
                    }
                    MediaTranslationRequest::LiveStream { recognition, .. } => {
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
