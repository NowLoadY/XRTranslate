//! Android's selected-text action drives the shared translator without a window.
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicI32, Ordering},
    },
    time::{Duration, Instant},
};

use crossbeam_channel::bounded;
use jni::{
    JNIEnv,
    objects::{JClass, JObject, JString, JValue},
    sys::jint,
};
use xrtranslate_protocol::PromptGraphSet;

use crate::{
    backend::BackendManager,
    client_settings::ClientSettings,
    session_coordinator::{
        PluginSessionBinding, PluginSessionOwner, SessionEventSubscriber, SessionOutputPolicy,
        TranslationEvent, TranslationOutcome, TranslationSegment, TranslationSessionOwner,
        TranslationTask,
    },
    text_translation::TextTranslation,
};

const OWNER: &str = "android_text_actions";
static CANCELLED_THROUGH: AtomicI32 = AtomicI32::new(0);
static OPEN_TRANSLATION: AtomicBool = AtomicBool::new(false);
const CONTEXT_IDLE: Duration = Duration::from_secs(90);
static RUNTIME: (Mutex<Option<BackgroundRuntime>>, Condvar) = (Mutex::new(None), Condvar::new());

struct BackgroundRuntime {
    translator: TextTranslation,
    backend: BackendManager,
    server_url: String,
    idle_deadline: Instant,
}

fn start_idle_cleanup() -> bool {
    static RUNNING: AtomicBool = AtomicBool::new(false);
    if RUNNING.swap(true, Ordering::AcqRel) {
        return true;
    }
    let started = std::thread::Builder::new()
        .name("text-context-idle".into())
        .spawn(|| {
            let (runtime, changed) = &RUNTIME;
            let mut state = runtime.lock().unwrap();
            loop {
                let Some(current) = state.as_ref() else {
                    RUNNING.store(false, Ordering::Release);
                    return;
                };
                let remaining = current
                    .idle_deadline
                    .saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    let expired = state.take();
                    RUNNING.store(false, Ordering::Release);
                    drop(state);
                    drop(expired);
                    return;
                }
                state = changed.wait_timeout(state, remaining).unwrap().0;
            }
        })
        .is_ok();
    if !started {
        RUNNING.store(false, Ordering::Release);
    }
    started
}
static COMPLETED: Mutex<VecDeque<TranslationSegment>> = Mutex::new(VecDeque::new());

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_xrtranslate_app_MainActivity_openTranslation(
    _env: JNIEnv,
    _class: JClass,
) {
    OPEN_TRANSLATION.store(true, Ordering::Release);
}

pub(crate) fn take_open_translation() -> bool {
    OPEN_TRANSLATION.swap(false, Ordering::AcqRel)
}

pub(crate) fn take_completed() -> Vec<TranslationSegment> {
    COMPLETED.lock().unwrap().drain(..).collect()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_xrtranslate_app_textactions_TextTranslationService_cancel(
    _env: JNIEnv,
    _class: JClass,
    id: jint,
) {
    CANCELLED_THROUGH.fetch_max(id, Ordering::AcqRel);
}

#[derive(Default)]
struct Results {
    segments: BTreeMap<u32, TranslationSegment>,
    outcome: Option<TranslationOutcome>,
}
struct ResultSink(Arc<Mutex<Results>>);
impl SessionEventSubscriber for ResultSink {
    fn accepts_owner(&self, owner: &TranslationSessionOwner) -> bool {
        owner.is_plugin(OWNER)
    }
    fn on_translation_event(&self, _owner: &TranslationSessionOwner, event: &TranslationEvent) {
        let mut result = self.0.lock().unwrap();
        match event {
            TranslationEvent::Segment(segment) if segment.translated.is_some() => {
                result
                    .segments
                    .insert(segment.segment_index, segment.clone());
            }
            TranslationEvent::ReplaceSegments(segments)
                if segments.iter().any(|s| s.translated.is_some()) =>
            {
                result.segments = segments
                    .iter()
                    .filter(|s| s.translated.is_some())
                    .map(|s| (s.segment_index, s.clone()))
                    .collect();
            }
            TranslationEvent::Finished { outcome, .. } => result.outcome = Some(outcome.clone()),
            _ => {}
        }
    }
}

fn translate(id: i32, text: String, conversation: String) -> (i32, String) {
    if CANCELLED_THROUGH.load(Ordering::Acquire) >= id {
        return (4, String::new());
    }
    let Ok(directory) = std::env::current_dir() else {
        return (4, String::new());
    };
    let settings = ClientSettings::load(&directory);
    if settings.first_run {
        return (2, String::new());
    }
    let languages = crate::service_config::ServiceConfigEditor::load()
        .language_capabilities()
        .and_then(|capabilities| {
            crate::text_translation::select_languages(
                &text,
                &settings.source_lang,
                &settings.target_lang,
                capabilities,
            )
        });
    let Ok(languages) = languages else {
        return (2, String::new());
    };
    let (runtime, changed) = &RUNTIME;
    let mut state = runtime.lock().unwrap();
    if state.as_ref().is_some_and(|current| {
        current.idle_deadline <= Instant::now() || current.server_url != settings.server_url
    }) {
        state.take();
    }
    let runtime = state.get_or_insert_with(|| BackgroundRuntime {
        translator: TextTranslation::default(),
        backend: BackendManager::load(),
        server_url: settings.server_url.clone(),
        idle_deadline: Instant::now() + CONTEXT_IDLE,
    });
    let outcome = translate_request(runtime, id, text, conversation, settings, languages);
    if outcome.0 == 3 && CANCELLED_THROUGH.load(Ordering::Acquire) < id {
        runtime.idle_deadline = Instant::now() + CONTEXT_IDLE;
        if !start_idle_cleanup() {
            state.take();
        }
    } else {
        state.take();
    }
    changed.notify_one();
    outcome
}

fn translate_request(
    runtime: &mut BackgroundRuntime,
    id: i32,
    text: String,
    conversation: String,
    settings: ClientSettings,
    languages: xrtranslate_engine::language::LanguageSelection,
) -> (i32, String) {
    let BackgroundRuntime {
        translator,
        backend,
        ..
    } = runtime;
    let diagnostic_input = text.clone();
    let graphs = PromptGraphSet {
        graph: settings.prompt_library.active_graph(),
    };
    let owner = PluginSessionOwner::new(
        OWNER,
        id.to_string(),
        "Translation",
        "Translation",
        "Translating…",
    )
    .in_conversation(conversation);
    let session_owner = TranslationSessionOwner::Plugin(owner.clone());
    translator.update_prompts(graphs.clone());
    if let Err(error) = translator.submit(TranslationTask::text(
        text,
        languages,
        Some(PluginSessionBinding::text(
            owner,
            SessionOutputPolicy::PluginOnly,
        )),
    )) {
        log::warn!(
            "Selected-text request {id} was rejected: {}",
            error.replace(&diagnostic_input, "[selected text]")
        );
        return (4, String::new());
    }
    let (events, received) = bounded(256);
    let results = Arc::new(Mutex::new(Results::default()));
    let subscribers: Vec<Box<dyn SessionEventSubscriber>> =
        vec![Box::new(ResultSink(results.clone()))];
    let started = Instant::now();
    while CANCELLED_THROUGH.load(Ordering::Acquire) < id
        && started.elapsed() < Duration::from_secs(160)
    {
        translator.poll(backend, &settings.server_url, graphs.clone(), None, &events);
        for event in received.try_iter() {
            if event.scope.accepts_events() {
                let _ = event.scope.publish(&event.event, &subscribers);
            }
        }
        let mut result = results.lock().unwrap();
        if let Some(outcome) = result.outcome.take() {
            let text = result
                .segments
                .values()
                .filter_map(|s| s.translated.as_deref())
                .collect::<Vec<_>>()
                .join("\n");
            if matches!(outcome, TranslationOutcome::Completed) && !text.trim().is_empty() {
                let mut completed = COMPLETED.lock().unwrap();
                completed.extend(std::mem::take(&mut result.segments).into_values());
                while completed.len() > 32 {
                    completed.pop_front();
                }
                return (3, text);
            }
            match outcome {
                TranslationOutcome::Failed(error) => log::warn!(
                    "Selected-text request {id} failed after {:.1}s: {}",
                    started.elapsed().as_secs_f32(),
                    error.replace(&diagnostic_input, "[selected text]"),
                ),
                TranslationOutcome::Cancelled => {}
                TranslationOutcome::Completed => {
                    log::warn!("Selected-text request {id} completed without translated text")
                }
            }
            translator.cancel_owner(&session_owner);
            return (4, String::new());
        }
        drop(result);
        std::thread::sleep(Duration::from_millis(40));
    }
    if CANCELLED_THROUGH.load(Ordering::Acquire) < id {
        log::warn!(
            "Selected-text request {id} timed out after {:.1}s",
            started.elapsed().as_secs_f32()
        );
    }
    translator.cancel_owner(&session_owner);
    (4, String::new())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_xrtranslate_app_textactions_TextTranslationService_translate(
    mut env: JNIEnv,
    service: JObject,
    id: jint,
    text: JString,
    directory: JString,
    libraries: JString,
    conversation: JString,
) {
    let outcome = (|| {
        let text: String = env.get_string(&text).ok()?.into();
        let directory: String = env.get_string(&directory).ok()?.into();
        let libraries: String = env.get_string(&libraries).ok()?.into();
        let conversation: String = env.get_string(&conversation).ok()?.into();
        if text.trim().is_empty() || text.chars().count() > 32768 {
            return None;
        }
        if let Err(error) = crate::android::initialize_storage(directory.into(), libraries.into()) {
            log::warn!("Cannot prepare selected-text translation: {error}");
            return None;
        }
        Some(translate(id, text, conversation))
    })()
    .unwrap_or((4, String::new()));
    if let Ok(result) = env.new_string(outcome.1) {
        let _ = env.call_method(
            service,
            "complete",
            "(IILjava/lang/String;)V",
            &[
                JValue::Int(id),
                JValue::Int(outcome.0),
                JValue::Object(&result),
            ],
        );
    }
}

impl crate::XRTranslateApp {
    pub(crate) fn poll_android_text_actions(&mut self, ctx: &eframe::egui::Context) {
        if take_open_translation() {
            self.navigation.page = crate::ui::Page::Translation;
            self.fullscreen_history = None;
            ctx.request_repaint();
        }
        let completed = take_completed();
        if completed.is_empty() {
            return;
        }
        if let Ok(mut state) = self.shared_session_state.lock() {
            for segment in completed {
                let Some(translated) = segment.translated else {
                    continue;
                };
                crate::history::upsert_completed_translation(
                    &mut state.translations,
                    crate::history::TranslationHistoryEntry {
                        turn_id: segment.turn_id,
                        segment_index: segment.segment_index,
                        stream_id: Some(segment.stream_id),
                        audio_source: segment.audio_source,
                        live: false,
                        source: segment.source,
                        translated,
                        speaker_id: segment.speaker_id,
                        source_start_ms: segment.source_start_ms,
                        source_end_ms: segment.source_end_ms,
                        timing: segment.timing,
                        boundary: segment.boundary,
                        term_matches: Vec::new(),
                        revisable: false,
                        overlap_ratio: 0.0,
                        authoritative_snapshot: false,
                        revision_id: 0,
                        source_revision: None,
                        translated_revision: None,
                    },
                );
            }
            let excess = state.translations.len().saturating_sub(100);
            state.translations.drain(..excess);
        }
        ctx.request_repaint();
    }
}
