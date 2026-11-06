mod media;
use eframe::egui;
use serde_json::{Value, json};
use std::time::Duration;
use ui_director::ToolProvider;
pub use xrtranslate_client_core::{
    audio_playback, audio_processing, export, file_dialog, i18n, language_label,
    languages_conflict, media_import, recognition as client_settings, session_coordinator, ui,
};
use xrtranslate_client_core::{
    playback::remote,
    recognition::CaptureSource,
    services::{Capabilities, Inference},
    session_coordinator::*,
};
use xrtranslate_plugin::{Client, Context, Environment, Plugin, Request, surface};
const ID: &str = "media";

#[derive(Default)]
struct App {
    media: Option<media::MediaPlugin>,
    client: Option<Client>,
    capabilities: Option<Capabilities>,
    context: egui::Context,
    players: remote::Snapshots,
    inference: Inference,
    requests: Vec<(Request, Option<TranslationSessionOwner>)>,
    owners: xrtranslate_client_core::services::ActiveTasks,
    agent: Value,
    state: Value,
}
impl App {
    fn request(&mut self, name: &str, args: Value) {
        self.requests
            .push((self.client.as_ref().unwrap().request(name, args), None));
    }
    fn start(&mut self, task: TranslationTask) {
        let owner = task.owner();
        self.owners.begin(&task);
        self.requests.push((
            self.client
                .as_ref()
                .unwrap()
                .request("translation.start", json!(task)),
            Some(owner),
        ));
    }
    fn cancel(&mut self, owner: &TranslationSessionOwner) {
        self.request("translation.stop", json!(owner));
        self.owners.remove(owner);
    }
    fn stop(&mut self) {
        for owner in self.owners.take().collect::<Vec<_>>() {
            self.cancel(&owner);
        }
        if let Some(media) = &mut self.media {
            media.poll_translation_events();
            media.stop_import();
            media.pause_task();
        }
    }
    fn optimize(&mut self, task: &str) -> Result<(), String> {
        if !self.capabilities.as_ref().is_some_and(|c| c.agent) {
            return Err("Choose a model that supports Agent first.".into());
        }
        let (scope, goal) = self.media.as_mut().unwrap().prepare_optimization(task)?;
        self.request("agent.run", json!({"scope":scope,"goal":goal}));
        let language = self.capabilities.as_ref().unwrap().language;
        self.client.as_ref().unwrap().event(
            "message",
            json!(i18n::tr(language, "I'm improving the translation.")),
        )?;
        Ok(())
    }
    fn action(&mut self, action: media::MediaAction) -> Result<(), String> {
        use media::{MediaAction, MediaTranslationRequest};
        let cap = self
            .capabilities
            .as_ref()
            .ok_or("Host is not ready")?
            .clone();
        match action {
            MediaAction::None => {}
            MediaAction::GenerateSpeech => {
                if cap.tts {
                    self.media.as_mut().unwrap().controller.start_speech();
                }
            }
            MediaAction::OptimizeSubtitles => {
                if let Some(task) = self
                    .media
                    .as_ref()
                    .unwrap()
                    .controller
                    .active_task_id
                    .clone()
                {
                    self.optimize(&task)?;
                }
            }
            MediaAction::StopOptimization => self.request("agent.stop", json!({})),
            MediaAction::AudioPreprocessing(action) => {
                self.request("models.preprocessing", json!(action))
            }
            MediaAction::StopTranslation => self.stop(),
            MediaAction::TranslateSubtitles { restart } => {
                self.stop();
                self.media
                    .as_mut()
                    .unwrap()
                    .start_subtitles(cap.languages, restart, cap.agent)?;
            }
            MediaAction::RetranslateSubtitle => {
                let task = self
                    .media
                    .as_mut()
                    .unwrap()
                    .retranslate_subtitle(cap.languages)?;
                self.start(task);
            }
            MediaAction::StartTranslation { request } => {
                let (source, target, recognition) = match &request {
                    MediaTranslationRequest::ImportMedia {
                        source_language,
                        target_language,
                        recognition,
                        ..
                    }
                    | MediaTranslationRequest::LiveStream {
                        source_language,
                        target_language,
                        recognition,
                        ..
                    } => (source_language, target_language, recognition.clone()),
                };
                let mut languages = cap.languages.select(source, target)?;
                if !cap.preprocessing.is_ready(
                    recognition
                        .preprocessing
                        .unwrap_or(cap.default_preprocessing),
                ) {
                    return Err("Download every required model package to continue.".into());
                }
                self.stop();
                let media = self.media.as_mut().unwrap();
                let mut profiles = Vec::new();
                let input = match request {
                    request @ MediaTranslationRequest::ImportMedia { .. } => {
                        media.start_file_subtitles(languages, cap.ask, cap.agent)?;
                        let (source, target) = languages.wire();
                        languages =
                            xrtranslate_engine::language::LanguageSelection::parse_with_options(
                                &source, &target, None, true,
                            )?;
                        request.import_input(cap.default_preprocessing, None)?
                    }
                    MediaTranslationRequest::LiveStream { recognition, .. } => {
                        media.controller.subtitles.clear();
                        media.controller.start_task();
                        profiles.push((CaptureSource::SystemAudio, recognition));
                        TranslationInput::Live(CaptureSource::SystemAudio)
                    }
                };
                let task = TranslationTask {
                    languages,
                    plugin: media.translation_session_binding(),
                    context: media.translation_context(),
                    text_workload: None,
                    input,
                    profiles,
                };
                self.start(task);
            }
        }
        Ok(())
    }
    fn import_event(
        &mut self,
        owner: TranslationSessionOwner,
        event: media_import::AudioImportEvent,
    ) {
        use media_import::{AudioImportEvent as E, AudioImportStage};
        let media = self.media.as_mut().unwrap();
        if let Some(operation) = owner
            .operation_id()
            .filter(|id| media.is_workspace_operation(id))
        {
            if let E::Error(error) = event {
                media.accept_workspace_event(
                    operation,
                    &TranslationEvent::Finished {
                        stream_id: 0,
                        outcome: TranslationOutcome::Failed(error),
                    },
                );
                self.cancel(&owner);
            }
            return;
        }
        if owner.operation_id() != media.controller.active_task_id.as_deref() {
            return;
        }
        match event {
            E::Progress(progress) => media.update_import_progress(
                match progress.stage {
                    AudioImportStage::Extracting => media::ImportProgressStage::Extracting,
                    AudioImportStage::Recognizing => media::ImportProgressStage::Recognizing,
                },
                progress.fraction,
                progress.position,
                progress.duration,
            ),
            E::Completed { .. } => media.complete_import(),
            E::Stopped { .. } | E::OutputClosed => media.stop_import(),
            E::Error(error) => {
                media.stop_import();
                media.set_error(error);
                self.cancel(&owner);
            }
            E::Started(_) => {}
        }
    }
}
impl Plugin for App {
    fn initialize(
        &mut self,
        environment: Environment,
        context: &mut Context,
    ) -> Result<(), String> {
        let client = context.client();
        if self.media.is_none() {
            std::fs::create_dir_all(&environment.data_directory).map_err(|e| e.to_string())?;
            // Adopt existing projects once. Absolute source/audio paths stay valid.
            let destination = environment.data_directory.join("video_tasks.json");
            if !destination.exists()
                && let Some(runtime) = environment.data_directory.ancestors().nth(3)
            {
                let source = runtime.join("video_tasks.json");
                if source.is_file() {
                    std::fs::copy(source, &destination).map_err(|e| e.to_string())?;
                }
            }
            let player = remote::Player::new(client.clone(), 1, self.players.clone())?;
            let controller = media::controller::MediaController::open(
                environment.data_directory,
                Some(Box::new(player)),
            );
            self.media = Some(media::MediaPlugin::new(controller));
        }
        self.client = Some(client);
        Ok(())
    }
    fn call(
        &mut self,
        name: &str,
        arguments: Value,
        _: &mut Context,
    ) -> xrtranslate_plugin::Result {
        self.media
            .as_mut()
            .ok_or("Media is not ready")?
            .call(&ui_director::ToolCall {
                name: name.into(),
                arguments,
            })
    }
    fn event(&mut self, name: &str, value: Value, _: &mut Context) {
        let result = (|| -> Result<(), String> {
            match name {
                "host.services" => {
                    let cap: Capabilities =
                        serde_json::from_value(value).map_err(|e| e.to_string())?;
                    if let Some(media) = &mut self.media {
                        media.available_preprocessing = [
                            xrtranslate_protocol::AudioPreprocessing::None,
                            xrtranslate_protocol::AudioPreprocessing::Gtcrn,
                            xrtranslate_protocol::AudioPreprocessing::VocalSeparation,
                        ]
                        .into_iter()
                        .filter(|value| cap.preprocessing.is_ready(*value))
                        .collect();
                        media.default_preprocessing = cap.default_preprocessing;
                    }
                    self.capabilities = Some(cap);
                }
                "translation.event" => {
                    let owner: TranslationSessionOwner =
                        serde_json::from_value(value["owner"].clone())
                            .map_err(|e| e.to_string())?;
                    let event: TranslationEvent = serde_json::from_value(value["event"].clone())
                        .map_err(|e| e.to_string())?;
                    self.media
                        .as_ref()
                        .unwrap()
                        .event_sink
                        .on_translation_event(&owner, &event);
                    self.owners.observe(&owner, &event);
                }
                "translation.import" => self.import_event(
                    serde_json::from_value(value["owner"].clone()).map_err(|e| e.to_string())?,
                    serde_json::from_value(value["event"].clone()).map_err(|e| e.to_string())?,
                ),
                "playback.state" => {
                    if let Some(id) = value["id"].as_u64() {
                        self.players.lock().unwrap().insert(
                            id,
                            serde_json::from_value(value["snapshot"].clone())
                                .map_err(|e| e.to_string())?,
                        );
                    }
                }
                "playback.error" => {
                    if let Some(error) = value["error"].as_str() {
                        self.media.as_mut().unwrap().set_error(error);
                    }
                }
                "plugin.state" if value["plugin"] == "agent" => self.agent = value["state"].clone(),
                "ui.visibility" => self
                    .media
                    .as_mut()
                    .unwrap()
                    .on_visibility_changed(value.as_bool().unwrap_or(false)),
                _ => {}
            }
            Ok(())
        })();
        if let Err(error) = result {
            log::warn!("Media event {name}: {error}");
            if let Some(media) = &mut self.media {
                media.set_error(error);
            }
        }
    }
    fn tick(
        &mut self,
        _: Duration,
        context: &mut Context,
    ) -> Option<Vec<xrtranslate_plugin::Widget>> {
        let (Some(client), Some(cap), Some(media)) =
            (&self.client, &self.capabilities, &mut self.media)
        else {
            return None;
        };
        self.inference.poll();
        self.requests.retain(|(request, owner)| {
            if let Some(result) = request.poll() {
                if let Err(error) = result {
                    if let Some(owner) = owner {
                        self.owners.remove(owner);
                        media.event_sink.on_translation_event(
                            owner,
                            &TranslationEvent::Finished {
                                stream_id: 0,
                                outcome: TranslationOutcome::Failed(error.clone()),
                            },
                        );
                    }
                    media.set_error(error);
                }
                false
            } else {
                true
            }
        });
        media.controller.tick();
        media.controller.poll_speech(client, &self.context);
        media.poll_translation_events();
        media.poll_subtitle_processing(
            cap.ask,
            cap.agent,
            |prompt| self.inference.request(client, prompt),
            &self.context,
        );
        let mut cancelled = std::mem::take(&mut media.workspace_cancelled);
        cancelled.extend(media.take_cancelled_retranslation());
        let tasks = [media.next_workspace_task(), media.next_subtitle_task()];
        let stop = media.take_stop_request();
        let optimize = if !self.agent["running"].as_bool().unwrap_or(false) {
            media.pending_optimization.take()
        } else {
            None
        };
        let state = json!({"fullscreen":media.controller.fullscreen_mode});
        if state != self.state {
            context.event("state", state.clone());
            self.state = state;
        }
        for owner in cancelled {
            self.cancel(&owner);
        }
        for task in tasks.into_iter().flatten() {
            self.start(task);
        }
        if stop {
            self.stop();
        }
        if let Some(task) = optimize {
            if let Err(error) = self.optimize(&task) {
                self.media.as_mut().unwrap().set_error(error);
            }
        }
        None
    }
}
impl surface::UiPlugin for App {
    fn configure_ui(&mut self, ctx: &egui::Context, options: &Value) {
        ui::configure_surface(ctx, options);
        self.context = ctx.clone();
        if let Some(media) = &mut self.media {
            media.controller.context = ctx.clone();
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui, context: &mut Context) {
        let (Some(cap), Some(media)) = (&self.capabilities, &mut self.media) else {
            ui.spinner();
            return;
        };
        let snapshot = media::MediaUiSnapshot {
            download_proxy_url: cap.download_proxy.clone(),
            language: cap.language,
            languages: cap.languages,
            can_optimize: cap.agent,
            can_synthesize: cap.tts,
            optimizing_task: self.agent["scope"]["workspace"]
                .as_str()
                .and_then(|id| media.workspace_task(id))
                .map(|task| task.id.clone()),
            default_preprocessing: cap.default_preprocessing,
            preprocessing_models: cap.preprocessing.clone(),
        };
        let action = media.render_page(&snapshot, ui);
        if let Err(error) = self.action(action) {
            self.media.as_mut().unwrap().set_error(error);
        }
        ui::forward_notices(ui.ctx(), context);
    }
}
fn main() {
    if std::env::args().any(|arg| arg == "--manifest") {
        println!(
            "{}",
            json!({"id":ID,"version":env!("CARGO_PKG_VERSION"),"min_host_version":"0.4.0","title":{"en":"Media","zh":"媒体"},"executable":"xrt-media","page":true,"icon":"assets/icon.svg","navigation_order":150,"requires":["translation.start","playback"],"subscriptions":["host.services","plugin.state"],"tools":media::workspace::tools()})
        );
        return;
    }
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .target(env_logger::Target::Stderr)
        .init();
    if let Err(error) = surface::serve(App::default()) {
        eprintln!("Media: {error}");
        std::process::exit(1)
    }
}

pub use xrtranslate_client_core::recognition::pause_tolerance_to_ms;
