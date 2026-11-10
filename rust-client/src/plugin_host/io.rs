//! Serial filesystem and process work; plugin state remains owned by the host.
use std::{sync::mpsc, thread::JoinHandle};

type Job = Box<dyn FnOnce() + Send>;

pub(super) struct Worker {
    sender: Option<mpsc::Sender<Job>>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel::<Job>();
        let thread = std::thread::Builder::new()
            .name("plugin-io".into())
            .spawn(move || {
                for job in receiver {
                    job();
                }
            })
            .expect("failed to start plugin IO worker");
        Self {
            sender: Some(sender),
            thread: Some(thread),
        }
    }

    pub fn submit<T: Send + 'static>(
        &self,
        action: impl FnOnce() -> T + Send + 'static,
        repaint: Option<eframe::egui::Context>,
    ) -> mpsc::Receiver<T> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .as_ref()
            .unwrap()
            .send(Box::new(move || {
                // Abandoned results, including processes, are released on this worker.
                let _ = reply.send(action());
                if let Some(repaint) = repaint {
                    repaint.request_repaint();
                }
            }))
            .expect("plugin IO worker exited");
        receiver
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
