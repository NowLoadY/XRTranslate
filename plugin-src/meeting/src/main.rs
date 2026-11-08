mod meeting;
use eframe::egui;
use meeting::{
    MeetingAction, MeetingAudioSource, MeetingInputRequest, MeetingPlugin, MeetingUiSnapshot,
};
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};
pub use xrtranslate_client_core::{
    audio_playback, audio_processing, compact_speaker_label, export, file_dialog, i18n,
    language_label, languages_conflict, media_import, recognition as client_settings,
    session_coordinator, ui,
};
use xrtranslate_client_core::{
    recognition::CaptureSource, services::Capabilities, session_coordinator::*,
};
pub use xrtranslate_engine::language::LANGUAGE_OPTIONS;
use xrtranslate_engine::language::LanguageSelection;
use xrtranslate_plugin::{Client, Context, Environment, Plugin, Request, surface};
const ID: &str = "meeting";
#[derive(Default)]
struct App {
    meeting: Option<MeetingPlugin>,
    client: Option<Client>,
    capabilities: Option<Capabilities>,
    owners: xrtranslate_client_core::services::ActiveTasks,
    requests: Vec<(Request, Option<TranslationSessionOwner>)>,
}
impl App {
    fn request(&mut self, name: &str, value: Value) {
        self.requests
            .push((self.client.as_ref().unwrap().request(name, value), None));
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
    fn stop(&mut self) {
        for owner in self.owners.take().collect::<Vec<_>>() {
            self.request("translation.stop", json!(owner));
        }
        if let Some(meeting) = &mut self.meeting {
            meeting.event_sink.cancel_sessions();
            meeting.event_sink.finish_active();
        }
    }
    fn start_input(
        &mut self,
        languages: LanguageSelection,
        path: Option<PathBuf>,
    ) -> Result<(), String> {
        let cap = self.capabilities.as_ref().ok_or("Host is not ready")?;
        let meeting = self.meeting.as_mut().unwrap();
        let online = meeting
            .controller
            .bundle
            .as_ref()
            .and_then(|bundle| bundle.meeting.input_source.as_ref())
            .filter(|value| value.starts_with("https://") || value.starts_with("http://"))
            .cloned();
        let input = if path.is_some() || online.is_some() {
            if let Some(path) = &path {
                if !meeting.controller.mark_imported_audio(path) {
                    return Err("Could not open meeting audio".into());
                }
            }
            let mut recognition = cap
                .profiles
                .iter()
                .find(|(source, _)| *source == CaptureSource::SystemAudio)
                .map(|(_, profile)| profile.clone())
                .unwrap_or_default();
            recognition.continuous_recognition = false;
            let processing = recognition
                .preprocessing
                .unwrap_or(cap.default_preprocessing);
            recognition.preprocessing = Some(if cap.preprocessing.is_ready(processing) {
                processing
            } else {
                xrtranslate_protocol::AudioPreprocessing::None
            });
            let mut options = media_import::AudioImportOptions::for_recognition(
                recognition.preprocessing.unwrap(),
            );
            if let Some(path) = path {
                TranslationInput::File {
                    path,
                    recognition,
                    options,
                }
            } else {
                options.pacing = media_import::AudioImportPacing::Realtime;
                TranslationInput::OnlineAudio {
                    url: online.unwrap(),
                    recognition,
                    options,
                }
            }
        } else {
            let source = meeting
                .controller
                .bundle
                .as_ref()
                .and_then(|bundle| bundle.meeting.input_source.as_deref())
                .unwrap_or("microphone");
            TranslationInput::Live(match source {
                "system_audio" => CaptureSource::SystemAudio,
                "both" => CaptureSource::Both,
                _ => CaptureSource::Microphone,
            })
        };
        let profiles = input
            .live_source()
            .map(|source| {
                source
                    .routes()
                    .iter()
                    .map(|source| {
                        let mut profile = cap
                            .profiles
                            .iter()
                            .find(|(s, _)| s == source)
                            .map(|(_, profile)| profile.clone())
                            .unwrap_or_default();
                        profile.continuous_recognition = false;
                        (*source, profile)
                    })
                    .collect()
            })
            .unwrap_or_default();
        meeting.event_sink.begin_translation(languages);
        meeting.event_sink.begin_sessions(
            input
                .live_source()
                .map_or(1, |source| source.routes().len()),
            input.live_source().is_none(),
        );
        let (source, target) = languages.wire();
        let task = TranslationTask {
            languages: LanguageSelection::parse_with_options(&source, &target, None, true)?,
            plugin: meeting.translation_session_binding(),
            context: Default::default(),
            text_workload: None,
            input,
            profiles,
        };
        self.start(task);
        Ok(())
    }
    fn stored(
        &mut self,
        id: &str,
        path: Option<PathBuf>,
        topic: Option<&str>,
    ) -> Result<(), String> {
        let stored = self
            .meeting
            .as_ref()
            .unwrap()
            .controller
            .meeting(id)
            .map_err(|e| e.to_string())?;
        let languages = self
            .capabilities
            .as_ref()
            .unwrap()
            .languages
            .select(&stored.source_language, &stored.target_language)?;
        self.stop();
        let meeting = self.meeting.as_mut().unwrap();
        if !meeting.controller.begin_capture(id) {
            return Err("Could not start meeting".into());
        }
        if let Some(topic) = topic {
            meeting.controller.create_capture_topic(id, topic);
        }
        self.start_input(languages, path)
    }
    fn action(&mut self, action: MeetingAction) -> Result<(), String> {
        match action {
            MeetingAction::None => {}
            MeetingAction::CreateAndStart(request) => {
                if self.meeting.as_ref().unwrap().is_busy() {
                    return Err("Finish the active meeting first".into());
                }
                let languages = self
                    .capabilities
                    .as_ref()
                    .unwrap()
                    .languages
                    .select(&request.source_language, &request.target_language)?;
                let path = match &request.input {
                    MeetingInputRequest::ImportedAudio { path } => Some(path.clone()),
                    _ => None,
                };
                let meeting = self.meeting.as_mut().unwrap();
                if let Some(id) = meeting.controller.create(&request)
                    && meeting.controller.begin_capture(&id)
                {
                    self.start_input(languages, path)?;
                }
            }
            MeetingAction::Continue(id) => {
                let meeting = self.meeting.as_mut().unwrap();
                if meeting.controller.active_meeting_id().as_deref() == Some(id.as_str()) {
                    if let Some(binding) = meeting.translation_session_binding() {
                        meeting.event_sink.set_translation_paused(false);
                        meeting
                            .controller
                            .resume_active_meeting()
                            .map_err(|e| e.to_string())?;
                        self.request(
                            "translation.resume",
                            json!(TranslationSessionOwner::Plugin(binding.owner)),
                        );
                    }
                } else {
                    self.stored(&id, None, None)?;
                }
            }
            MeetingAction::Pause => {
                let meeting = self.meeting.as_mut().unwrap();
                if let Some(binding) = meeting.translation_session_binding() {
                    meeting.event_sink.set_translation_paused(true);
                    let _ = meeting.controller.pause_capture();
                    self.request(
                        "translation.pause",
                        json!(TranslationSessionOwner::Plugin(binding.owner)),
                    );
                }
            }
            MeetingAction::End => {
                let meeting = self.meeting.as_mut().unwrap();
                let capture = meeting
                    .controller
                    .active_capture
                    .lock()
                    .ok()
                    .and_then(|active| active.clone());
                if let Some(capture) = capture {
                    if capture.imported_audio {
                        self.stop();
                    } else {
                        meeting
                            .controller
                            .store
                            .process_meeting(&capture.meeting_id)
                            .map_err(|e| e.to_string())?;
                        meeting.event_sink.request_finish();
                        if let Some(binding) = meeting.translation_session_binding() {
                            self.request(
                                "translation.finish",
                                json!(TranslationSessionOwner::Plugin(binding.owner)),
                            );
                        }
                    }
                }
            }
            MeetingAction::Export(id) => {
                if let Some((name, markdown)) = self
                    .meeting
                    .as_mut()
                    .unwrap()
                    .controller
                    .export_markdown(&id)
                {
                    export::save(&name, export::Format::Markdown, markdown)?;
                }
            }
            MeetingAction::Reprocess(request) => self.stored(
                &request.meeting_id,
                Some(request.audio_path),
                Some(&request.topic_title),
            )?,
        }
        Ok(())
    }
}
impl Plugin for App {
    fn initialize(
        &mut self,
        environment: Environment,
        context: &mut Context,
    ) -> Result<(), String> {
        if self.meeting.is_none() {
            std::fs::create_dir_all(&environment.data_directory).map_err(|e| e.to_string())?;
            let destination = environment.data_directory.join("meetings.sqlite3");
            if !destination.exists()
                && let Some(runtime) = environment.data_directory.ancestors().nth(3)
            {
                let old = runtime.join("meetings.sqlite3");
                // VACUUM INTO includes committed WAL content in the new database.
                if old.is_file() {
                    let source = rusqlite::Connection::open(old).map_err(|e| e.to_string())?;
                    source
                        .execute("VACUUM INTO ?1", [destination.to_string_lossy().as_ref()])
                        .map_err(|e| e.to_string())?;
                }
            }
            self.meeting = Some(MeetingPlugin::open(&environment.data_directory));
        }
        #[cfg(target_os = "android")]
        xrtranslate_client_core::file_dialog::install_client(context.client());
        self.client = Some(context.client());
        Ok(())
    }
    fn call(&mut self, _: &str, _: Value, _: &mut Context) -> xrtranslate_plugin::Result {
        Err("Use the meeting page's UI controls".into())
    }
    fn event(&mut self, name: &str, value: Value, _: &mut Context) {
        let result = (|| -> Result<(), String> {
            match name {
                "host.services" => {
                    self.capabilities =
                        Some(serde_json::from_value(value).map_err(|e| e.to_string())?)
                }
                "translation.event" => {
                    let owner: TranslationSessionOwner =
                        serde_json::from_value(value["owner"].clone())
                            .map_err(|e| e.to_string())?;
                    let event: TranslationEvent = serde_json::from_value(value["event"].clone())
                        .map_err(|e| e.to_string())?;
                    self.meeting
                        .as_ref()
                        .unwrap()
                        .event_sink
                        .on_translation_event(&owner, &event);
                    self.owners.observe(&owner, &event);
                }
                "translation.import" => {
                    let event: media_import::AudioImportEvent =
                        serde_json::from_value(value["event"].clone())
                            .map_err(|e| e.to_string())?;
                    match event {
                        media_import::AudioImportEvent::Completed { .. } => {
                            self.meeting.as_ref().unwrap().event_sink.request_finish()
                        }
                        media_import::AudioImportEvent::Error(error) => {
                            self.meeting
                                .as_ref()
                                .unwrap()
                                .event_sink
                                .fail_active(&error);
                            self.stop();
                            return Err(error);
                        }
                        _ => {}
                    }
                }
                "ui.visibility" if value == false => {
                    self.meeting.as_mut().unwrap().controller.pause_playback()
                }
                _ => {}
            }
            Ok(())
        })();
        if let Err(error) = result {
            log::warn!("Meeting event {name}: {error}");
            if let Some(meeting) = &mut self.meeting {
                meeting.set_error(error);
            }
        }
    }
    fn tick(&mut self, _: Duration, _: &mut Context) -> Option<Vec<xrtranslate_plugin::Widget>> {
        let Some(meeting) = &mut self.meeting else {
            return None;
        };
        self.requests.retain(|(request, owner)| {
            if let Some(result) = request.poll() {
                if let Err(error) = result {
                    if let Some(owner) = owner {
                        self.owners.remove(owner);
                        meeting.event_sink.on_translation_event(
                            owner,
                            &TranslationEvent::Finished {
                                stream_id: 0,
                                outcome: TranslationOutcome::Failed(error.clone()),
                            },
                        );
                        meeting.fail_active_startup(&error);
                    }
                    meeting.set_error(error);
                }
                false
            } else {
                true
            }
        });
        meeting.controller.poll_live_view();
        if let Some(error) = meeting.event_sink.take_translation_error() {
            meeting.set_error(error);
        }
        if let Some(task) = meeting.event_sink.next_translation_task() {
            self.start(task);
        }
        None
    }
}
impl surface::UiPlugin for App {
    fn back(&mut self) -> bool {
        self.meeting.as_mut().is_some_and(|meeting| meeting.controller.handle_back())
    }
    fn configure_ui(&mut self, ctx: &egui::Context, options: &Value) {
        ui::configure_surface(ctx, options)
    }
    fn ui(&mut self, ui: &mut egui::Ui, context: &mut Context) {
        let (Some(cap), Some(meeting)) = (&self.capabilities, &mut self.meeting) else {
            ui.spinner();
            return;
        };
        let snapshot = MeetingUiSnapshot {
            download_proxy_url: cap.download_proxy.clone(),
            default_audio_source: match cap.capture {
                CaptureSource::Microphone => MeetingAudioSource::Microphone,
                CaptureSource::SystemAudio => MeetingAudioSource::SystemAudio,
                CaptureSource::Both => MeetingAudioSource::Both,
            },
            default_source_language: cap.source_language.clone(),
            default_target_language: cap.target_language.clone(),
            languages: cap.languages,
            waiting_for_microphone: !cap.microphone_enabled
                && !cap.system_audio_enabled
                && meeting.is_busy(),
            language: cap.language,
        };
        let action = meeting.render_page(&snapshot, ui);
        if let Err(error) = self.action(action) {
            self.meeting.as_mut().unwrap().set_error(error);
        }
        ui::forward_notices(ui.ctx(), context);
    }
}
fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .target(env_logger::Target::Stderr)
        .init();
    if let Err(error) = xrtranslate_client_core::ui::surface::serve(App::default()) {
        eprintln!("Meeting: {error}");
        std::process::exit(1)
    }
}
