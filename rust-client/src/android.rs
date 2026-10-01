use android_activity::AndroidApp;
use std::{path::PathBuf, sync::Mutex};

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
    *NATIVE_LIBRARIES.lock().unwrap() =
        Some(PathBuf::from(String::from(env.get_string(&directory)?)));
    // Set once, before any application workers are created.
    std::env::set_current_dir(&data)?;
    std::fs::create_dir_all(data.join("runtime"))?;
    let selection = serde_json::json!({
        "schema_version": 1, "backend": "cpu", "llama_cpp_backend": "cpu", "onnx_backend": "cpu",
        "onnx_core_library": native_executable("onnxruntime"), "preload_libraries": [],
    });
    std::fs::write(
        data.join("runtime/native-runtime.json"),
        serde_json::to_vec_pretty(&selection)?,
    )?;
    Ok(())
}

#[unsafe(no_mangle)]
fn android_main(activity: AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("XRTranslate"),
    );
    *APP.lock().unwrap() = Some(activity.clone());
    let result = initialize(&activity)
        .map_err(|error| error.to_string())
        .and_then(|()| crate::run().map_err(|error| error.to_string()));
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
    *NATIVE_LIBRARIES.lock().unwrap() = None;
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
    atomic::{AtomicBool, AtomicU8, Ordering},
};
static FOREGROUND: AtomicBool = AtomicBool::new(true);
static MICROPHONE_RESULT: AtomicU8 = AtomicU8::new(0);
static RESUME_MICROPHONE: AtomicBool = AtomicBool::new(false);
static INPUT_STREAMS: Mutex<Vec<Weak<InputStream>>> = Mutex::new(Vec::new());

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
// AAudio capture cannot pause. Close the input while away and recreate it on return.
struct InputState {
    create: Box<dyn FnMut() -> Result<cpal::Stream, cpal::Error> + Send>,
    stream: Option<cpal::Stream>,
    playing: bool,
}
pub struct InputStream {
    state: Mutex<InputState>,
}
impl InputStream {
    fn play(&self) -> Result<(), cpal::Error> {
        let mut state = self.state.lock().unwrap();
        state.playing = true;
        Self::update(&mut state)
    }
    fn update(state: &mut InputState) -> Result<(), cpal::Error> {
        if !state.playing || !foreground() {
            state.stream.take();
            return Ok(());
        }
        if state.stream.is_none() {
            state.stream = Some((state.create)()?);
        }
        state.stream.as_ref().unwrap().play()
    }
    fn pause(&self) {
        let mut state = self.state.lock().unwrap();
        state.playing = false;
        state.stream.take();
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
    pub fn pause(&self) -> Result<(), cpal::Error> {
        match self {
            Self::Input(stream) => {
                stream.pause();
                Ok(())
            }
            Self::Output(stream) => stream.pause(),
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
    INPUT_STREAMS.lock().unwrap().retain(|stream| {
        let Some(stream) = stream.upgrade() else {
            return false;
        };
        let result = InputStream::update(&mut stream.state.lock().unwrap());
        if let Err(error) = result {
            log::warn!("Cannot change microphone activity: {error}");
        }
        true
    });
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
