use eframe::egui;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
};

type FileResult = Result<Option<PathBuf>, String>;
static ERRORS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static CONTEXT: Mutex<Option<egui::Context>> = Mutex::new(None);

pub fn set_context(context: egui::Context) {
    *CONTEXT.lock().unwrap() = Some(context);
}
pub fn take_errors() -> Vec<String> {
    std::mem::take(&mut *ERRORS.lock().unwrap())
}

fn report_error(error: String) {
    ERRORS.lock().unwrap().push(error);
    if let Some(context) = CONTEXT.lock().unwrap().as_ref() {
        context.request_repaint();
    }
}

pub struct FileDialog {
    #[cfg(not(target_os = "android"))]
    native: rfd::FileDialog,
    #[cfg(target_os = "android")]
    name: String,
    #[cfg(target_os = "android")]
    extensions: Vec<String>,
}

impl FileDialog {
    pub fn new() -> Self {
        Self {
            #[cfg(not(target_os = "android"))]
            native: rfd::FileDialog::new(),
            #[cfg(target_os = "android")]
            name: String::new(),
            #[cfg(target_os = "android")]
            extensions: Vec::new(),
        }
    }
    pub fn add_filter(mut self, name: &str, extensions: &[&str]) -> Self {
        #[cfg(not(target_os = "android"))]
        {
            self.native = self.native.add_filter(name, extensions);
        }
        #[cfg(target_os = "android")]
        {
            let _ = name;
            self.extensions
                .extend(extensions.iter().map(|extension| extension.to_string()));
        }
        self
    }
    pub fn set_file_name(mut self, name: impl AsRef<str>) -> Self {
        #[cfg(not(target_os = "android"))]
        {
            self.native = self.native.set_file_name(name.as_ref());
        }
        #[cfg(target_os = "android")]
        {
            self.name = name.as_ref().to_owned();
        }
        self
    }
    pub fn start_pick(self) -> FileRequest {
        #[cfg(not(target_os = "android"))]
        {
            FileRequest::completed(Ok(self.native.pick_file()))
        }
        #[cfg(target_os = "android")]
        {
            bridge::start(self, false)
        }
    }
    pub fn pick_file(self, context: &egui::Context, key: &str, requested: bool) -> Option<PathBuf> {
        let id = egui::Id::new(key);
        let mut pending = context.data(|data| data.get_temp::<FileRequest>(id));
        if requested && pending.is_none() {
            pending = Some(self.start_pick());
            context.data_mut(|data| data.insert_temp(id, pending.as_ref().unwrap().clone()));
        }
        let Some(request) = pending else {
            return None;
        };
        match request.poll() {
            Some(result) => {
                context.data_mut(|data| data.remove::<FileRequest>(id));
                match result {
                    Ok(path) => path,
                    Err(error) => {
                        report_error(error);
                        None
                    }
                }
            }
            None => {
                context.request_repaint_after(std::time::Duration::from_millis(100));
                None
            }
        }
    }
    pub fn save(self, content: impl AsRef<[u8]>) -> Result<(), String> {
        #[cfg(not(target_os = "android"))]
        if let Some(path) = self.native.save_file() {
            std::fs::write(path, content).map_err(|error| error.to_string())?;
        }
        #[cfg(target_os = "android")]
        {
            let request = bridge::start(self, true);
            let bytes = content.as_ref().to_vec();
            std::thread::Builder::new()
                .name("document-export".into())
                .spawn(move || {
                    let result = (|| -> Result<(), String> {
                        let Some(path) = request
                            .receiver
                            .lock()
                            .unwrap()
                            .recv()
                            .map_err(|error| error.to_string())??
                        else {
                            return Ok(());
                        };
                        std::fs::write(path, bytes).map_err(|error| error.to_string())?;
                        crate::android::with_activity(|env, activity| {
                            env.call_method(
                                activity,
                                "commitFileSave",
                                "(J)V",
                                &[jni::objects::JValue::Long(request.id as i64)],
                            )?;
                            Ok(())
                        })
                    })();
                    if let Err(error) = result {
                        report_error(error);
                    }
                })
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct FileRequest {
    receiver: Arc<Mutex<mpsc::Receiver<FileResult>>>,
    #[cfg(target_os = "android")]
    id: u64,
}
impl FileRequest {
    #[cfg(not(target_os = "android"))]
    fn completed(result: FileResult) -> Self {
        let (sender, receiver) = mpsc::channel();
        let _ = sender.send(result);
        Self {
            receiver: Arc::new(Mutex::new(receiver)),
        }
    }
    pub fn poll(&self) -> Option<FileResult> {
        match self.receiver.lock().unwrap().try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Ok(None)),
        }
    }
}

#[cfg(target_os = "android")]
mod bridge {
    use super::*;
    use std::{
        collections::BTreeMap,
        sync::{
            OnceLock,
            atomic::{AtomicU64, Ordering},
        },
    };
    static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);
    static REQUESTS: OnceLock<Mutex<BTreeMap<u64, mpsc::Sender<FileResult>>>> = OnceLock::new();

    pub fn start(dialog: FileDialog, save: bool) -> FileRequest {
        let id = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::channel();
        REQUESTS
            .get_or_init(Mutex::default)
            .lock()
            .unwrap()
            .insert(id, sender);
        let result = crate::android::with_activity(|env, activity| {
            let name = env.new_string(dialog.name)?;
            let extensions = env.new_string(dialog.extensions.join(","))?;
            env.call_method(
                activity,
                "requestFileDialog",
                "(JZLjava/lang/String;Ljava/lang/String;)V",
                &[
                    jni::objects::JValue::Long(id as i64),
                    jni::objects::JValue::Bool(save.into()),
                    jni::objects::JValue::Object(&name),
                    jni::objects::JValue::Object(&extensions),
                ],
            )?;
            Ok(())
        });
        if let Err(error) = result {
            complete(id, Err(error));
        }
        FileRequest {
            receiver: Arc::new(Mutex::new(receiver)),
            id,
        }
    }

    fn complete(id: u64, result: FileResult) {
        let sender = REQUESTS
            .get_or_init(Mutex::default)
            .lock()
            .unwrap()
            .remove(&id);
        if let Some(sender) = sender {
            let _ = sender.send(result);
        } else if let Err(error) = result {
            report_error(error);
        }
        if let Some(context) = CONTEXT.lock().unwrap().as_ref() {
            context.request_repaint();
        }
    }

    #[unsafe(no_mangle)]
    extern "system" fn Java_org_xrtranslate_app_MainActivity_fileDialogCompleted(
        mut env: jni::JNIEnv<'_>,
        _class: jni::objects::JClass<'_>,
        id: jni::sys::jlong,
        path: jni::objects::JString<'_>,
        error: jni::objects::JString<'_>,
    ) {
        let result = if !error.is_null() {
            Err(env
                .get_string(&error)
                .map(String::from)
                .unwrap_or_else(|_| "Cannot access the selected document".into()))
        } else if path.is_null() {
            Ok(None)
        } else {
            env.get_string(&path)
                .map(|value| Some(PathBuf::from(String::from(value))))
                .map_err(|error| error.to_string())
        };
        complete(id as u64, result);
    }
}
