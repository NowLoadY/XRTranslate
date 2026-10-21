//! Composes Avatar control intent with host-owned capture and translation tasks.
//! Transport policy stays in the plugin; shared tasks keep their existing pipeline.
use crate::{
    XRTranslateApp,
    plugins::avatar::{
        HostAction as AvatarHostAction, HostResult as AvatarHostResult,
        OperationId as AvatarOperationId,
    },
    plugins::{self, game_plugins::Selection},
    session_coordinator::{TranslationInput, TranslationSessionOwner, TranslationTask},
    translation_service,
};
use eframe::egui;
use std::sync::atomic::Ordering;

pub(crate) struct AvatarHostSession {
    operation: AvatarOperationId,
    owner: TranslationSessionOwner,
    started: bool,
    startup_deadline: std::time::Instant,
    terminal: Option<AvatarHostResult>,
    retiring: Vec<translation_service::AudioTask>,
}

impl XRTranslateApp {
    pub(crate) fn avatar_enabled(&self) -> bool {
        self.game_plugins.active(Selection::Avatar)
    }

    pub(crate) fn avatar_owns(&self, owner: &TranslationSessionOwner) -> bool {
        self.avatar_host_session
            .as_ref()
            .is_some_and(|session| &session.owner == owner)
    }

    pub(crate) fn stop_avatar_owner(&mut self, owner: &TranslationSessionOwner) {
        if let Some(session) = &self.avatar_host_session
            && &session.owner == owner
        {
            self.avatar_plugin.request_host_stop(session.operation);
        }
        self.mark_avatar_terminal(owner, AvatarHostResult::Stopped);
    }

    pub(crate) fn stop_avatar(&mut self) {
        if let Some(owner) = self
            .avatar_host_session
            .as_ref()
            .map(|session| session.owner.clone())
        {
            self.stop_avatar_owner(&owner);
        }
    }

    pub(crate) fn avatar_task_ended(
        &mut self,
        owner: &TranslationSessionOwner,
        failed: bool,
    ) -> bool {
        if !self.avatar_owns(owner) {
            return false;
        }
        self.mark_avatar_terminal(owner, if failed {
            AvatarHostResult::Failed("The Avatar translation session failed. See XRTranslate's error notification for details.".into())
        } else { AvatarHostResult::Stopped });
        true
    }

    pub(crate) fn fail_avatar_startup(&mut self, owner: &TranslationSessionOwner, error: &str) {
        self.mark_avatar_terminal(owner, AvatarHostResult::Failed(error.to_owned()));
    }

    pub(crate) fn shutdown_avatar(&mut self) {
        self.avatar_plugin.poll(false, &self.osc_transport.handle());
        let actions = self.avatar_plugin.take_host_actions();
        self.apply_avatar_actions(actions, None);
    }

    pub(crate) fn poll_avatar(&mut self, ctx: Option<egui::Context>) {
        let enabled = self.avatar_enabled();
        self.avatar_plugin
            .poll(enabled, &self.osc_transport.handle());
        let actions = self.avatar_plugin.take_host_actions();
        self.apply_avatar_actions(actions, ctx);
        self.poll_avatar_session();
    }

    fn apply_avatar_actions(&mut self, actions: Vec<AvatarHostAction>, ctx: Option<egui::Context>) {
        for action in actions {
            match action {
                AvatarHostAction::Start(operation) => {
                    if self.first_run || !self.avatar_enabled() {
                        self.avatar_plugin.report_host_result(operation, AvatarHostResult::Failed(
                            "Complete XRTranslate setup and select your game in Game Plugins before starting translation.".into(),
                        ));
                        continue;
                    }
                    if self.avatar_host_session.is_some() {
                        self.avatar_plugin.report_host_result(
                            operation,
                            AvatarHostResult::Failed(
                                "The previous Avatar translation is still stopping.".into(),
                            ),
                        );
                        continue;
                    }
                    let languages =
                        match self
                            .service_config
                            .language_capabilities()
                            .and_then(|caps| {
                                caps.select_with_options(
                                    &self.source_lang,
                                    &self.target_lang,
                                    self.additional_target_lang.as_deref(),
                                    self.asr_only,
                                )
                            }) {
                            Ok(languages) => languages,
                            Err(error) => {
                                self.avatar_plugin
                                    .report_host_result(operation, AvatarHostResult::Failed(error));
                                continue;
                            }
                        };
                    let binding = plugins::avatar::session_binding(operation);
                    let owner = TranslationSessionOwner::Plugin(binding.owner.clone());
                    self.avatar_host_session = Some(AvatarHostSession {
                        operation,
                        owner,
                        started: false,
                        startup_deadline: std::time::Instant::now()
                            + std::time::Duration::from_secs(180),
                        terminal: None,
                        retiring: Vec::new(),
                    });
                    let profiles = self
                        .capture_source
                        .routes()
                        .iter()
                        .map(|source| (*source, self.recognition_settings(*source).clone()))
                        .collect();
                    self.start_translation_task(
                        TranslationTask {
                            languages,
                            context: plugins::avatar::translation_context(operation),
                            text_workload: None,
                            plugin: Some(binding),
                            input: TranslationInput::ExclusiveLive(self.capture_source),
                            profiles,
                        },
                        ctx.clone(),
                    );
                }
                AvatarHostAction::Cancel(operation) => {
                    let owner = self
                        .avatar_host_session
                        .as_ref()
                        .filter(|session| session.operation == operation)
                        .map(|session| session.owner.clone());
                    if let Some(owner) = owner {
                        self.stop_task_owner(&owner);
                    } else {
                        // A rejected or already-retired operation owns no resources.
                        self.avatar_plugin
                            .report_host_result(operation, AvatarHostResult::Stopped);
                    }
                }
            }
        }
    }

    fn mark_avatar_terminal(&mut self, owner: &TranslationSessionOwner, outcome: AvatarHostResult) {
        if let Some(session) = &mut self.avatar_host_session
            && &session.owner == owner
            && session.terminal.is_none()
        {
            session.terminal = Some(outcome);
        }
    }

    /// Keep cancelled Avatar workers owned until their sockets have exited.
    pub(crate) fn retire_audio_task(&mut self, mut task: translation_service::AudioTask) {
        if let Some(session) = &mut self.avatar_host_session
            && session.owner == task.owner
        {
            task.invalidate();
            for channel in &mut task.channels {
                channel.session.cancel();
                channel.audio_tx.take();
            }
            session.retiring.push(task);
        } else {
            self.exclusive_live_inputs
                .retain(|(owner, _)| owner != &task.owner);
            task.cancel();
        }
    }

    pub(crate) fn poll_avatar_session(&mut self) {
        let Some(session) = &self.avatar_host_session else {
            return;
        };
        let owner = session.owner.clone();
        let operation = session.operation;
        let task = self.audio_tasks.iter().find(|task| task.owner == owner);
        if session.terminal.is_none() {
            let outcome = if !session.started
                && std::time::Instant::now() >= session.startup_deadline
            {
                Some(AvatarHostResult::Failed(
                    "Avatar translation did not become ready within 180 seconds.".into(),
                ))
            } else if self
                .exclusive_live_inputs
                .iter()
                .any(|(lease_owner, source)| {
                    lease_owner == &owner
                        && source
                            .routes()
                            .iter()
                            .any(|source| !self.input_enabled(*source))
                })
                || (session.started
                    && task
                        .is_some_and(|task| task.channels.iter().any(|channel| !channel.capturing)))
            {
                // Desktop input controls remain authoritative. A paused input
                // must not leave the Avatar reporting active translation.
                Some(AvatarHostResult::Stopped)
            } else {
                None
            };
            if let Some(outcome) = outcome {
                self.mark_avatar_terminal(&owner, outcome);
            } else if !session.started
                && task.is_some_and(|task| {
                    !task.channels.is_empty()
                        && task.channels.iter().all(|channel| {
                            channel.capturing
                                && channel.scope.ready.load(Ordering::Acquire)
                                && channel.scope.accepts_events()
                        })
                })
            {
                self.avatar_host_session.as_mut().unwrap().started = true;
                self.avatar_plugin
                    .report_host_result(operation, AvatarHostResult::Started);
            }
        }
        if self
            .avatar_host_session
            .as_ref()
            .is_some_and(|session| session.terminal.is_some())
        {
            self.stop_task_owner(&owner);
            let finished = self.avatar_host_session.as_ref().is_some_and(|session| {
                session.retiring.iter().all(|task| {
                    task.channels
                        .iter()
                        .all(|channel| channel.session.is_finished())
                })
            });
            if finished {
                let session = self.avatar_host_session.take().unwrap();
                for task in session.retiring {
                    task.cancel();
                }
                self.exclusive_live_inputs
                    .retain(|(lease_owner, _)| lease_owner != &owner);
                self.avatar_plugin
                    .report_host_result(operation, session.terminal.unwrap());
            }
        }
    }
}
