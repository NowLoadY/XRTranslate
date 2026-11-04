//! One diagnostic snapshot, written off the UI thread; newer snapshots supersede queued ones.
use serde_json::Value;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    thread::JoinHandle,
};

pub(super) struct Environment {
    pending: Arc<Mutex<Option<Value>>>,
    wake: Option<crossbeam_channel::Sender<()>>,
    worker: Option<JoinHandle<()>>,
}

impl Environment {
    pub fn open(directory: PathBuf) -> (Self, Option<Value>) {
        let path = directory.join("environment.json");
        let previous = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
        let pending = Arc::new(Mutex::new(None::<Value>));
        let next = pending.clone();
        let (wake, rx) = crossbeam_channel::bounded(1);
        let worker = std::thread::Builder::new()
            .name("agent-environment".into())
            .spawn(move || {
                while rx.recv().is_ok() {
                    let Some(value) = next.lock().unwrap().take() else {
                        continue;
                    };
                    let result = (|| -> std::io::Result<()> {
                        std::fs::create_dir_all(&directory)?;
                        let mut file = tempfile::NamedTempFile::new_in(&directory)?;
                        serde_json::to_writer(&mut file, &value).map_err(std::io::Error::other)?;
                        file.as_file().sync_all()?;
                        file.persist(&path).map_err(|e| e.error)?;
                        Ok(())
                    })();
                    if let Err(error) = result {
                        log::error!("Cannot save agent environment: {error}");
                    }
                }
            })
            .expect("agent environment worker");
        (
            Self {
                pending,
                wake: Some(wake),
                worker: Some(worker),
            },
            previous,
        )
    }
    pub fn save(&self, value: Value) {
        *self.pending.lock().unwrap() = Some(value);
        if let Some(wake) = &self.wake {
            let _ = wake.try_send(());
        }
    }
}

impl Drop for Environment {
    fn drop(&mut self) {
        self.wake.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
