//! Android's user-initiated text entry points consume the shared translator.
use std::{collections::BTreeMap, sync::Mutex};

use crossbeam_channel::{Receiver, Sender, unbounded};
use jni::{
    JNIEnv,
    objects::{JClass, JString, JValue},
    sys::jboolean,
};

use crate::session_coordinator::{
    PluginSessionBinding, PluginSessionOwner, SessionEventSubscriber, SessionOutputPolicy,
    TranslationEvent, TranslationOutcome, TranslationSessionOwner,
};

const OWNER: &str = "android_text_actions";
const TRANSLATING: i32 = 1;
const SETUP: i32 = 2;
const COMPLETED: i32 = 3;
const FAILED: i32 = 4;

struct Request {
    id: String,
    text: String,
    configure: bool,
}

enum Command {
    Translate(Request),
    Cancel,
}

// Only the latest user intent matters; replacing it cancels this adapter's owner.
static COMMAND: Mutex<Option<Command>> = Mutex::new(None);
static CONTEXT: Mutex<Option<eframe::egui::Context>> = Mutex::new(None);

fn enqueue(command: Command) {
    *COMMAND.lock().unwrap() = Some(command);
    if let Some(ctx) = CONTEXT.lock().unwrap().as_ref() {
        ctx.request_repaint();
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_xrtranslate_app_textactions_TextActions_submit(
    mut env: JNIEnv,
    _class: JClass,
    id: JString,
    text: JString,
    configure: jboolean,
) {
    let (Ok(id), Ok(text)) = (env.get_string(&id), env.get_string(&text)) else {
        return;
    };
    let id: String = id.into();
    let text: String = text.into();
    if !id.is_empty() && !text.trim().is_empty() && text.chars().count() <= 32_768 {
        enqueue(Command::Translate(Request {
            id,
            text,
            configure: configure != 0,
        }));
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_xrtranslate_app_textactions_TextActions_cancel(
    _env: JNIEnv,
    _class: JClass,
) {
    enqueue(Command::Cancel);
}

#[derive(Clone)]
pub struct EventSink {
    tx: Sender<(String, TranslationEvent)>,
    rx: Receiver<(String, TranslationEvent)>,
}

impl Default for EventSink {
    fn default() -> Self {
        let (tx, rx) = unbounded();
        Self { tx, rx }
    }
}

impl SessionEventSubscriber for EventSink {
    fn accepts_owner(&self, owner: &TranslationSessionOwner) -> bool {
        owner.is_plugin(OWNER)
    }

    fn on_translation_event(&self, owner: &TranslationSessionOwner, event: &TranslationEvent) {
        if self.accepts_owner(owner)
            && let Some(id) = owner.operation_id()
        {
            let _ = self.tx.send((id.to_owned(), event.clone()));
        }
    }
}

#[derive(Default)]
pub struct TextActions {
    pub event_sink: EventSink,
    request: Option<Request>,
    owner: Option<PluginSessionOwner>,
    segments: BTreeMap<u32, String>,
    waiting_for_setup: bool,
    submitted: bool,
}

impl TextActions {
    pub(crate) fn configuration_applied(&mut self) {
        self.waiting_for_setup = false;
    }
}

fn notify(id: &str, state: i32, result: &str) {
    let _ = crate::android::with_activity(|env, activity| {
        env.with_local_frame(4, |env| {
            let id = env.new_string(id)?;
            let result = env.new_string(result)?;
            env.call_method(
                activity,
                "updateTextAction",
                "(Ljava/lang/String;ILjava/lang/String;)V",
                &[
                    JValue::Object(&id),
                    JValue::Int(state),
                    JValue::Object(&result),
                ],
            )?;
            Ok::<_, jni::errors::Error>(())
        })
    });
}

impl Drop for TextActions {
    fn drop(&mut self) {
        *CONTEXT.lock().unwrap() = None;
    }
}

impl crate::XRTranslateApp {
    pub(crate) fn poll_android_text_actions(&mut self, ctx: &eframe::egui::Context) {
        *CONTEXT.lock().unwrap() = Some(ctx.clone());
        let command = COMMAND.lock().unwrap().take();
        if let Some(command) = command {
            let duplicate = matches!(&command, Command::Translate(request)
                if self.android_text_actions.request.as_ref().is_some_and(|current| current.id == request.id));
            if !duplicate {
                if let Some(owner) = self.android_text_actions.owner.take() {
                    self.stop_task_owner(&TranslationSessionOwner::Plugin(owner));
                }
                let state = &mut self.android_text_actions;
                state.segments.clear();
                state.submitted = false;
                state.request = match command {
                    Command::Translate(request) => Some(request),
                    Command::Cancel => None,
                };
                state.waiting_for_setup = self.first_run
                    || state
                        .request
                        .as_ref()
                        .is_some_and(|request| request.configure);
                if let Some(request) = state.request.as_ref() {
                    if request.configure && !self.first_run {
                        self.navigation.page = crate::ui::Page::Settings;
                        self.settings_section =
                            crate::ui::pages::settings::SettingsSection::ServiceProviders;
                    }
                    if state.waiting_for_setup {
                        notify(&request.id, SETUP, "");
                    }
                }
            }
        }

        let state = &mut self.android_text_actions;
        while let Ok((id, event)) = state.event_sink.rx.try_recv() {
            if state
                .owner
                .as_ref()
                .is_none_or(|owner| owner.operation_id() != id)
            {
                continue;
            }
            match event {
                TranslationEvent::Segment(segment) => {
                    if let Some(text) = segment.translated {
                        state.segments.insert(segment.segment_index, text);
                    }
                }
                TranslationEvent::ReplaceSegments(segments) => {
                    state.segments = segments
                        .into_iter()
                        .filter_map(|segment| {
                            segment.translated.map(|text| (segment.segment_index, text))
                        })
                        .collect();
                }
                TranslationEvent::Finished { outcome, .. } => {
                    let text = state
                        .segments
                        .values()
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                        .join("\n");
                    let completed =
                        matches!(outcome, TranslationOutcome::Completed) && !text.trim().is_empty();
                    notify(
                        &id,
                        if completed { COMPLETED } else { FAILED },
                        if completed { &text } else { "" },
                    );
                    state.owner = None;
                    state.segments.clear();
                }
                TranslationEvent::StreamEnded { .. } => {}
            }
        }
        if state.waiting_for_setup {
            if self.first_run || self.navigation.page == crate::ui::Page::Settings {
                return;
            }
            state.waiting_for_setup = false;
        }
        if self.first_run || state.submitted {
            return;
        }
        let Some(request) = state.request.as_ref() else {
            return;
        };
        let id = request.id.clone();
        let text = request.text.clone();
        let owner = PluginSessionOwner::new(
            OWNER,
            id.clone(),
            "Translation",
            "Translation",
            "Translating…",
        );
        state.owner = Some(owner.clone());
        state.submitted = true;
        notify(&id, TRANSLATING, "");
        if !self.submit_text_translation(
            &text,
            None,
            None,
            Some(PluginSessionBinding::text(
                owner,
                SessionOutputPolicy::PluginOnly,
            )),
        ) {
            self.android_text_actions.owner = None;
            notify(&id, FAILED, "");
        }
    }
}
