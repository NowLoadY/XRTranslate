use android_activity::AndroidApp;
use std::{path::PathBuf, sync::Mutex};

pub(crate) mod ime;

static APP: Mutex<Option<AndroidApp>> = Mutex::new(None);
static NATIVE_LIBRARIES: Mutex<Option<PathBuf>> = Mutex::new(None);

pub fn app() -> AndroidApp {
    APP.lock()
        .unwrap()
        .as_ref()
        .expect("Android activity initialized")
        .clone()
}
pub fn native_library_directory() -> PathBuf {
    NATIVE_LIBRARIES
        .lock()
        .unwrap()
        .as_ref()
        .expect("Android library directory initialized")
        .clone()
}
pub fn native_executable(name: &str) -> PathBuf {
    native_library_directory().join(format!("lib{name}.so"))
}

pub fn native_options(mut options: eframe::NativeOptions) -> eframe::NativeOptions {
    options.android_app = Some(app());
    options.viewport = eframe::egui::ViewportBuilder::default().with_fullscreen(true);
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup {
        setup.instance_descriptor.backends = eframe::wgpu::Backends::VULKAN;
    }
    options
}

pub(crate) fn install_clipboard(ctx: &eframe::egui::Context) {
    ctx.on_end_pass(
        "android_clipboard",
        std::sync::Arc::new(|ui| {
            ui.ctx().output(|output| {
                for command in &output.commands {
                    let eframe::egui::OutputCommand::CopyText(text) = command else {
                        continue;
                    };
                    let result = with_activity(|env, activity| {
                        let text = env.new_string(text)?;
                        env.call_method(
                            activity,
                            "copyText",
                            "(Ljava/lang/String;)V",
                            &[jni::objects::JValue::Object(&text)],
                        )?;
                        Ok(())
                    });
                    if let Err(error) = &result {
                        log::error!("Could not copy text: {error}");
                    }
                }
            });
        }),
    );
}

fn initialize(activity: &AndroidApp) -> Result<(), Box<dyn std::error::Error>> {
    let data = activity
        .internal_data_path()
        .ok_or("Application storage unavailable")?;
    std::fs::create_dir_all(&data)?;
    RESOURCES_READY.store(false, Ordering::Release);
    RESOURCES_RESULT.lock().unwrap().take();
    MICROPHONE_RESULT.store(0, Ordering::Release);
    RESUME_MICROPHONE.store(false, Ordering::Release);
    // The activity owns both pointers for the lifetime of android_main.
    let vm = unsafe { jni::JavaVM::from_raw(activity.vm_as_ptr().cast()) }?;
    let mut env = vm.attach_current_thread()?;
    let object = unsafe { jni::objects::JObject::from_raw(activity.activity_as_ptr().cast()) };
    env.call_method(&object, "prepareConfiguration", "()V", &[])?;
    let info = env
        .call_method(
            &object,
            "getApplicationInfo",
            "()Landroid/content/pm/ApplicationInfo;",
            &[],
        )?
        .l()?;
    let directory: jni::objects::JString = env
        .get_field(info, "nativeLibraryDir", "Ljava/lang/String;")?
        .l()?
        .into();
    initialize_storage(
        data,
        PathBuf::from(String::from(env.get_string(&directory)?)),
    )
    .map_err(Into::into)
}

pub(crate) fn initialize_storage(data: PathBuf, libraries: PathBuf) -> Result<(), String> {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("XRTranslate"),
    );
    let mut installed = NATIVE_LIBRARIES.lock().unwrap();
    if installed.is_some() {
        return Ok(());
    }
    // The activity and the headless text service use the same process storage.
    std::env::set_current_dir(&data).map_err(|error| error.to_string())?;
    std::fs::create_dir_all(data.join("runtime")).map_err(|error| error.to_string())?;
    let selection = serde_json::json!({
        "schema_version": 1, "backend": "cpu", "llama_cpp_backend": "cpu", "onnx_backend": "cpu",
        "onnx_core_library": libraries.join("libonnxruntime.so"), "preload_libraries": [],
    });
    std::fs::write(
        data.join("runtime/native-runtime.json"),
        serde_json::to_vec_pretty(&selection).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    *installed = Some(libraries);
    Ok(())
}

#[unsafe(no_mangle)]
fn android_main(activity: AndroidApp) {
    *APP.lock().unwrap() = Some(activity.clone());
    let result = initialize(&activity)
        .map_err(|error| error.to_string())
        .and_then(|()| crate::run().map_err(|error| error.to_string()));
    ime::uninstall();
    if let Err(error) = result {
        log::error!("Application stopped: {error}");
        let shown = with_activity(|env, activity| {
            if env.exception_check()? {
                env.exception_clear()?;
            }
            let message = env.new_string(error)?;
            env.call_method(
                activity,
                "showStartupError",
                "(Ljava/lang/String;)V",
                &[jni::objects::JValue::Object(&message)],
            )?;
            Ok(())
        });
        if shown.is_ok() {
            // Returning finishes GameActivity. Keep lifecycle events flowing until
            // the native dialog is dismissed, even if no renderer could start.
            let mut destroyed = false;
            while !destroyed {
                activity.poll_events(None, |event| {
                    if matches!(
                        event,
                        android_activity::PollEvent::Main(android_activity::MainEvent::Destroy)
                    ) {
                        destroyed = true;
                    }
                });
            }
        }
    }
    if let Some(task) = RESOURCES_TASK.lock().unwrap().take() {
        let _ = task.join();
    }
    *APP.lock().unwrap() = None;
}

pub fn available_memory() -> u64 {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| {
                line.strip_prefix("MemAvailable:")?
                    .split_whitespace()
                    .next()?
                    .parse::<u64>()
                    .ok()
                    .and_then(|value| value.checked_mul(1024))
            })
        })
        .unwrap_or(0)
}

pub fn with_activity<T>(
    operation: impl FnOnce(&mut jni::JNIEnv<'_>, &jni::objects::JObject<'_>) -> jni::errors::Result<T>,
) -> Result<T, String> {
    let activity = APP
        .lock()
        .unwrap()
        .as_ref()
        .ok_or("Activity unavailable")?
        .clone();
    let vm = unsafe { jni::JavaVM::from_raw(activity.vm_as_ptr().cast()) }
        .map_err(|error| error.to_string())?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|error| error.to_string())?;
    let object = unsafe { jni::objects::JObject::from_raw(activity.activity_as_ptr().cast()) };
    let result = operation(&mut env, &object).map_err(|error| error.to_string());
    if result.is_err() && env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
    result
}

use cpal::traits::StreamTrait;
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
};
static FOREGROUND: AtomicBool = AtomicBool::new(true);
static MICROPHONE_RESULT: AtomicU8 = AtomicU8::new(0);
static RESUME_MICROPHONE: AtomicBool = AtomicBool::new(false);
static INPUT_STREAMS: Mutex<Vec<Weak<InputStream>>> = Mutex::new(Vec::new());
static CAPTURE_STOPPED: AtomicU64 = AtomicU64::new(0);
struct CaptureState {
    inputs: usize,
    generation: u64,
    service: bool,
}
static CAPTURE: Mutex<CaptureState> = Mutex::new(CaptureState {
    inputs: 0,
    generation: 0,
    service: false,
});

pub fn take_capture_stop_request() -> bool {
    let stopped = CAPTURE_STOPPED.swap(0, Ordering::AcqRel);
    stopped != 0 && stopped == CAPTURE.lock().unwrap().generation
}

fn set_capture_service(active: bool, generation: u64) -> Result<(), String> {
    with_activity(|env, activity| {
        env.call_method(
            activity,
            "setMicrophoneCaptureActive",
            "(ZJ)V",
            &[
                jni::objects::JValue::Bool(active.into()),
                jni::objects::JValue::Long(generation as i64),
            ],
        )?;
        Ok(())
    })
}

fn release_capture(generation: u64) {
    let mut capture = CAPTURE.lock().unwrap();
    if capture.generation == generation {
        capture.inputs -= 1;
        if capture.inputs == 0 {
            capture.service = false;
            drop(capture);
            let _ = set_capture_service(false, generation);
        }
    }
}

fn input_streams() -> Vec<Arc<InputStream>> {
    let mut streams = INPUT_STREAMS.lock().unwrap();
    streams.retain(|stream| stream.strong_count() > 0);
    streams.iter().filter_map(Weak::upgrade).collect()
}

pub fn microphone_allowed() -> bool {
    with_activity(|env, activity| {
        env.call_method(activity, "hasMicrophonePermission", "()Z", &[])?
            .z()
    })
    .unwrap_or(false)
}
pub fn request_microphone(resume: bool) -> Result<(), String> {
    if resume {
        RESUME_MICROPHONE.store(true, Ordering::Release);
    }
    with_activity(|env, activity| {
        env.call_method(activity, "requestMicrophonePermission", "()V", &[])?;
        Ok(())
    })
}
pub fn cancel_microphone_request() {
    RESUME_MICROPHONE.store(false, Ordering::Release);
}
pub fn take_microphone_result() -> Option<bool> {
    let result = MICROPHONE_RESULT.swap(0, Ordering::AcqRel);
    if result == 0 || !RESUME_MICROPHONE.swap(false, Ordering::AcqRel) {
        None
    } else {
        Some(result == 1)
    }
}
// AAudio capture cannot pause. A microphone service owns capture across UI pauses.
struct InputState {
    create: Box<dyn FnMut() -> Result<cpal::Stream, cpal::Error> + Send>,
    stream: Option<cpal::Stream>,
    playing: bool,
    generation: u64,
}
pub struct InputStream {
    state: Mutex<InputState>,
}
impl InputStream {
    fn play(&self) -> Result<(), cpal::Error> {
        let mut state = self.state.lock().unwrap();
        if state.playing {
            return Self::update(&mut state);
        }
        if !foreground() && !CAPTURE.lock().unwrap().service {
            return Err(cpal::Error::new(cpal::ErrorKind::PermissionDenied));
        }
        state.playing = true;
        if let Err(error) = Self::update(&mut state) {
            state.playing = false;
            state.stream.take();
            return Err(error);
        }
        let mut capture = CAPTURE.lock().unwrap();
        let first = capture.inputs == 0;
        if first {
            capture.generation += 1;
            capture.service = false;
        }
        capture.inputs += 1;
        let generation = capture.generation;
        state.generation = generation;
        drop(capture);
        drop(state);
        if first && let Err(error) = set_capture_service(true, generation) {
            self.pause();
            log::warn!("Cannot keep microphone capture active: {error}");
            return Err(cpal::Error::new(cpal::ErrorKind::PermissionDenied));
        }
        Ok(())
    }
    fn update(state: &mut InputState) -> Result<(), cpal::Error> {
        if !state.playing || (!foreground() && !CAPTURE.lock().unwrap().service) {
            state.stream.take();
            return Ok(());
        }
        if state.stream.is_none() {
            let stream = (state.create)()?;
            stream.play()?;
            state.stream = Some(stream);
        }
        Ok(())
    }
    fn pause(&self) {
        self.pause_generation(None);
    }
    fn pause_generation(&self, generation: Option<u64>) {
        let mut state = self.state.lock().unwrap();
        if generation.is_some_and(|generation| state.generation != generation) {
            return;
        }
        let playing = state.playing;
        let generation = state.generation;
        state.playing = false;
        state.stream.take();
        drop(state);
        if playing {
            release_capture(generation);
        }
    }
}
impl Drop for InputStream {
    fn drop(&mut self) {
        let state = self.state.get_mut().unwrap();
        state.stream.take();
        if state.playing {
            release_capture(state.generation);
        }
    }
}
pub enum AudioStream {
    Input(Arc<InputStream>),
    Output(Arc<cpal::Stream>),
}
impl AudioStream {
    pub fn play(&self) -> Result<(), cpal::Error> {
        match self {
            Self::Input(stream) => stream.play(),
            Self::Output(stream) => stream.play(),
        }
    }
}
pub fn input_stream(
    create: impl FnMut() -> Result<cpal::Stream, cpal::Error> + Send + 'static,
) -> Result<AudioStream, cpal::Error> {
    let stream = Arc::new(InputStream {
        state: Mutex::new(InputState {
            create: Box::new(create),
            stream: None,
            playing: false,
            generation: 0,
        }),
    });
    let mut streams = INPUT_STREAMS.lock().unwrap();
    streams.retain(|stream| stream.strong_count() > 0);
    streams.push(Arc::downgrade(&stream));
    Ok(AudioStream::Input(stream))
}

pub fn foreground() -> bool {
    FOREGROUND.load(Ordering::Acquire)
}

#[unsafe(no_mangle)]
extern "system" fn Java_org_xrtranslate_app_MainActivity_microphonePermissionResult(
    _env: jni::JNIEnv<'_>,
    _class: jni::objects::JClass<'_>,
    granted: jni::sys::jboolean,
) {
    MICROPHONE_RESULT.store(if granted != 0 { 1 } else { 2 }, Ordering::Release);
}

#[unsafe(no_mangle)]
extern "system" fn Java_org_xrtranslate_app_MainActivity_foregroundChanged(
    _env: jni::JNIEnv<'_>,
    _class: jni::objects::JClass<'_>,
    foreground: jni::sys::jboolean,
) {
    FOREGROUND.store(foreground != 0, Ordering::Release);
    refresh_input_streams();
}

fn refresh_input_streams() {
    for stream in input_streams() {
        let (result, generation) = {
            let mut state = stream.state.lock().unwrap();
            (InputStream::update(&mut state), state.generation)
        };
        if let Err(error) = result {
            log::warn!("Cannot change microphone activity: {error}");
            stream.pause_generation(Some(generation));
            CAPTURE_STOPPED.fetch_max(generation, Ordering::AcqRel);
        }
    }
}

#[unsafe(no_mangle)]
extern "system" fn Java_org_xrtranslate_app_audio_MicrophoneService_captureGeneration(
    _env: jni::JNIEnv<'_>,
    _class: jni::objects::JClass<'_>,
) -> jni::sys::jlong {
    let capture = CAPTURE.lock().unwrap();
    if capture.inputs == 0 {
        0
    } else {
        capture.generation as i64
    }
}

#[unsafe(no_mangle)]
extern "system" fn Java_org_xrtranslate_app_audio_MicrophoneService_captureServiceChanged(
    _env: jni::JNIEnv<'_>,
    _class: jni::objects::JClass<'_>,
    generation: jni::sys::jlong,
    active: jni::sys::jboolean,
) {
    let mut capture = CAPTURE.lock().unwrap();
    if capture.generation != generation as u64 {
        return;
    }
    capture.service = active != 0;
    let capturing = capture.inputs > 0;
    drop(capture);
    if active != 0 {
        refresh_input_streams();
    } else if capturing {
        CAPTURE_STOPPED.fetch_max(generation as u64, Ordering::AcqRel);
        for stream in input_streams() {
            stream.pause_generation(Some(generation as u64));
        }
    }
}

static RESOURCES_READY: AtomicBool = AtomicBool::new(false);
static RESOURCES_RESULT: Mutex<Option<Result<(), String>>> = Mutex::new(None);
static RESOURCES_TASK: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);
pub fn resources_ready() -> bool {
    RESOURCES_READY.load(Ordering::Acquire)
}
pub fn take_resources_result() -> Option<Result<(), String>> {
    RESOURCES_RESULT.lock().unwrap().take()
}
pub fn prepare_resources(context: eframe::egui::Context) {
    let result = std::thread::Builder::new()
        .name("application-resources".into())
        .spawn(move || {
            let result = with_activity(|env, activity| {
                env.call_method(activity, "prepareResources", "()V", &[])?;
                Ok(())
            });
            RESOURCES_READY.store(result.is_ok(), Ordering::Release);
            *RESOURCES_RESULT.lock().unwrap() = Some(result);
            context.request_repaint();
        });
    match result {
        Ok(task) => *RESOURCES_TASK.lock().unwrap() = Some(task),
        Err(error) => *RESOURCES_RESULT.lock().unwrap() = Some(Err(error.to_string())),
    }
}

static UPDATE_INSTALL_RESULT: Mutex<Option<Result<(), String>>> = Mutex::new(None);

pub fn validate_update(path: &std::path::Path, version: &str) -> Result<(), String> {
    with_activity(|env, activity| {
        let path = env.new_string(path.to_string_lossy())?;
        let version = env.new_string(version)?;
        let error = env
            .call_method(
                activity,
                "validateUpdate",
                "(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;",
                &[(&path).into(), (&version).into()],
            )?
            .l()?;
        if error.is_null() {
            Ok(Ok(()))
        } else {
            let error = jni::objects::JString::from(error);
            Ok(Err(env.get_string(&error)?.into()))
        }
    })?
}

pub fn request_update_install(path: &std::path::Path) -> Result<(), String> {
    UPDATE_INSTALL_RESULT.lock().unwrap().take();
    with_activity(|env, activity| {
        let path = env.new_string(path.to_string_lossy())?;
        env.call_method(
            activity,
            "installUpdate",
            "(Ljava/lang/String;)V",
            &[(&path).into()],
        )?;
        Ok(())
    })
}

pub fn take_update_install_result() -> Option<Result<(), String>> {
    UPDATE_INSTALL_RESULT.lock().unwrap().take()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_xrtranslate_app_MainActivity_updateInstallCompleted(
    mut env: jni::JNIEnv,
    _class: jni::objects::JClass,
    error: jni::objects::JString,
) {
    let result = if error.is_null() {
        Ok(())
    } else {
        Err(env
            .get_string(&error)
            .map(|value| value.into())
            .unwrap_or_else(|_| "Cannot start Android installer.".into()))
    };
    *UPDATE_INSTALL_RESULT.lock().unwrap() = Some(result);
}
