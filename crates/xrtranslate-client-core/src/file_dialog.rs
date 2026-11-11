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

/// Open a local directory in the desktop file manager, without a browser URL.
#[cfg(any(windows, target_os = "linux"))]
pub fn open_directory(path: &std::path::Path) -> std::io::Result<()> {
    let path = std::path::absolute(path)?;
    std::fs::create_dir_all(&path)?;
    #[cfg(windows)]
    let mut command = std::process::Command::new("explorer.exe");
    #[cfg(target_os = "linux")]
    let mut command = std::process::Command::new("xdg-open");
    let mut child = command.arg(path).spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
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
            let (sender, receiver) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = sender.send(Ok(self.native.pick_file()));
                if let Some(context) = CONTEXT.lock().unwrap().as_ref() {
                    context.request_repaint();
                }
            });
            FileRequest {
                receiver: Arc::new(Mutex::new(receiver)),
            }
        }
        #[cfg(target_os = "android")]
        {
            bridge::start(self, false)
        }
    }
    pub fn pick_file(self, context: &egui::Context, key: &str, requested: bool) -> Option<PathBuf> {
        let id = egui::Id::unique(key);
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
        let bytes = content.as_ref().to_vec();
        self.write(move |path| std::fs::write(path, bytes))
    }
    pub(crate) fn write(
        self,
        write: impl FnOnce(PathBuf) -> std::io::Result<()> + Send + 'static,
    ) -> Result<(), String> {
        #[cfg(target_os = "android")]
        let request = bridge::start(self, true);
        std::thread::Builder::new()
            .name("document-export".into())
            .spawn(move || {
                let result = (|| -> Result<(), String> {
                    #[cfg(not(target_os = "android"))]
                    let path = self.native.save_file();
                    #[cfg(target_os = "android")]
                    let path = request
                        .receiver
                        .lock()
                        .unwrap()
                        .recv()
                        .map_err(|e| e.to_string())??;
                    let Some(path) = path else {
                        return Ok(());
                    };
                    write(path).map_err(|e| e.to_string())?;
                    #[cfg(target_os = "android")]
                    commit_save(request.id.load(std::sync::atomic::Ordering::Acquire))?;
                    Ok(())
                })();
                if let Err(error) = result {
                    report_error(error);
                }
            })
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
    pub fn copy_file(self, source: PathBuf) -> Result<(), String> {
        self.write(move |path| {
            if path.canonicalize().ok() != source.canonicalize().ok() {
                std::fs::copy(source, path)?;
            }
            Ok(())
        })
    }
    #[cfg(any(windows, target_os = "linux"))]
    pub fn save_path(self) -> Option<PathBuf> {
        self.native.save_file()
    }

    #[cfg(any(windows, target_os = "linux"))]
    pub fn start_pick_folder(self) -> FileRequest {
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(Ok(self.native.pick_folder()));
            if let Some(context) = CONTEXT.lock().unwrap().as_ref() {
                context.request_repaint();
            }
        });
        FileRequest {
            receiver: Arc::new(Mutex::new(receiver)),
        }
    }
}

#[derive(Clone)]
pub struct FileRequest {
    receiver: Arc<Mutex<mpsc::Receiver<FileResult>>>,
    #[cfg(target_os = "android")]
    id: Arc<std::sync::atomic::AtomicU64>,
}
impl FileRequest {
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
        if let Some(client) = CLIENT.lock().unwrap().clone() {
            let remote_id = Arc::new(std::sync::atomic::AtomicU64::new(0));
            let pending_id = remote_id.clone();
            std::thread::spawn(move || {
                let result = client.request("files.dialog", serde_json::json!({"name":dialog.name,"extensions":dialog.extensions,"save":save})).wait()
                    .and_then(|value| {
                        pending_id.store(value["id"].as_u64().ok_or("Missing document request ID")?, Ordering::Release);
                        serde_json::from_value(value["path"].clone()).map_err(|e| e.to_string())
                    });
                complete(id, result);
            });
            return FileRequest {
                receiver: Arc::new(Mutex::new(receiver)),
                id: remote_id,
            };
        }
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
            id: Arc::new(std::sync::atomic::AtomicU64::new(id)),
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

#[cfg(target_os = "android")]
static CLIENT: Mutex<Option<xrtranslate_plugin::Client>> = Mutex::new(None);

#[cfg(target_os = "android")]
pub fn install_client(client: xrtranslate_plugin::Client) {
    *CLIENT.lock().unwrap() = Some(client);
}

#[cfg(target_os = "android")]
pub fn host_dialog(arguments: serde_json::Value) -> Result<FileRequest, String> {
    let extensions: Vec<String> =
        serde_json::from_value(arguments["extensions"].clone()).map_err(|e| e.to_string())?;
    let dialog = FileDialog {
        name: arguments["name"].as_str().unwrap_or_default().into(),
        extensions,
    };
    Ok(bridge::start(
        dialog,
        arguments["save"].as_bool().unwrap_or(false),
    ))
}

#[cfg(target_os = "android")]
pub fn commit_save(id: u64) -> Result<(), String> {
    if let Some(client) = CLIENT.lock().unwrap().clone() {
        return client
            .request("files.commit", serde_json::json!({"id":id}))
            .wait()
            .map(|_| ());
    }
    crate::android::with_activity(|env, activity| {
        env.call_method(
            activity,
            "commitFileSave",
            "(J)V",
            &[jni::objects::JValue::Long(id as i64)],
        )?;
        Ok(())
    })
}

#[cfg(target_os = "android")]
impl FileRequest {
    pub fn host_reply(&self) -> Option<xrtranslate_plugin::Result> {
        self.poll().map(|result| result.map(|path| serde_json::json!({"id":self.id.load(std::sync::atomic::Ordering::Acquire),"path":path})))
    }
}
