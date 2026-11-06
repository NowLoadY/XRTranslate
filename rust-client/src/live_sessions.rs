//! Readiness and retirement of exclusive live tasks, independent of their consumers.
use crate::{XRTranslateApp, session_coordinator::TranslationSessionOwner, translation_service};
use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant},
};
use xrtranslate_client_core::services::LiveState;

pub(crate) struct LiveSession {
    pub owner: TranslationSessionOwner,
    started: bool,
    deadline: Instant,
    terminal: Option<LiveState>,
    retiring: Vec<translation_service::AudioTask>,
}
impl LiveSession {
    pub fn new(owner: TranslationSessionOwner) -> Self {
        Self {
            owner,
            started: false,
            deadline: Instant::now() + Duration::from_secs(180),
            terminal: None,
            retiring: Vec::new(),
        }
    }
}
impl XRTranslateApp {
    pub(crate) fn live_task_ended(
        &mut self,
        owner: &TranslationSessionOwner,
        failed: bool,
    ) -> bool {
        if !self
            .live_sessions
            .iter()
            .any(|session| &session.owner == owner)
        {
            return false;
        }
        self.mark_live_terminal(
            owner,
            if failed {
                LiveState::Failed(
                    "The translation session failed. See the error notification for details."
                        .into(),
                )
            } else {
                LiveState::Stopped
            },
        );
        true
    }
    pub(crate) fn mark_live_terminal(
        &mut self,
        owner: &TranslationSessionOwner,
        outcome: LiveState,
    ) {
        if let Some(session) = self
            .live_sessions
            .iter_mut()
            .find(|session| &session.owner == owner)
        {
            if session.terminal.is_none() {
                session.terminal = Some(outcome);
            }
        }
    }
    pub(crate) fn retire_audio_task(&mut self, mut task: translation_service::AudioTask) {
        if let Some(session) = self
            .live_sessions
            .iter_mut()
            .find(|session| session.owner == task.owner)
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
    fn publish_live_state(&self, owner: &TranslationSessionOwner, state: LiveState) {
        if let Some(plugin) = owner.plugin() {
            self.plugin_host.send_event(
                plugin.plugin_id(),
                "translation.state",
                serde_json::json!({"owner":owner,"state":state}),
            );
        }
    }
    pub(crate) fn poll_live_sessions(&mut self) {
        for index in 0..self.live_sessions.len() {
            let session = &self.live_sessions[index];
            let owner = session.owner.clone();
            let task = self.audio_tasks.iter().find(|task| task.owner == owner);
            if session.terminal.is_none() {
                let terminal = if !session.started && Instant::now() >= session.deadline {
                    Some(LiveState::Failed(
                        "Translation did not become ready within 180 seconds.".into(),
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
                        && task.is_some_and(|task| {
                            task.channels.iter().any(|channel| !channel.capturing)
                        }))
                {
                    Some(LiveState::Stopped)
                } else {
                    None
                };
                if let Some(terminal) = terminal {
                    self.mark_live_terminal(&owner, terminal);
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
                    self.live_sessions[index].started = true;
                    self.publish_live_state(&owner, LiveState::Running);
                }
            }
            if self.live_sessions[index].terminal.is_some() {
                self.stop_task_owner(&owner);
            }
        }
        let mut kept = Vec::new();
        for session in std::mem::take(&mut self.live_sessions) {
            if session.terminal.is_some()
                && session.retiring.iter().all(|task| {
                    task.channels
                        .iter()
                        .all(|channel| channel.session.is_finished())
                })
            {
                for task in session.retiring {
                    task.cancel();
                }
                self.exclusive_live_inputs
                    .retain(|(owner, _)| owner != &session.owner);
                self.publish_live_state(&session.owner, session.terminal.unwrap());
            } else {
                kept.push(session);
            }
        }
        self.live_sessions = kept;
    }
}
