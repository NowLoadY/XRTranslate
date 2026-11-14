//! Serialize view work; a newer selection cancels the old request before it can continue.
use crate::remote::{Error, Remote, Result};
use eframe::egui;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
};
use xrt_guide::guide::{Context, Step};

pub enum Task {
    Connect(Option<PathBuf>),
    Present(String, Step),
}
pub enum Event {
    Connected(Context),
    Selected(Context),
    Presented(Option<bool>),
}

pub struct Worker {
    generation: Arc<AtomicU64>,
    input: Option<mpsc::Sender<(u64, Task)>>,
    output: mpsc::Receiver<(u64, Result<Event>)>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Worker {
    pub fn new(port: u16, ctx: egui::Context) -> Self {
        let generation = Arc::new(AtomicU64::new(0));
        let current = generation.clone();
        let (input, commands) = mpsc::channel();
        let (events, output) = mpsc::channel();
        let thread = thread::spawn(move || {
            while let Ok((generation, task)) = commands.recv() {
                if current.load(Ordering::Acquire) != generation {
                    continue;
                }
                let remote = Remote {
                    port,
                    generation,
                    current: current.clone(),
                };
                let result = match task {
                    Task::Connect(path) => remote.connect(path.as_deref()).map(Event::Connected),
                    Task::Present(page, step) => remote.context().and_then(|context| {
                        let _ = events.send((generation, Ok(Event::Selected(context))));
                        ctx.request_repaint();
                        remote.present(&page, &step).map(Event::Presented)
                    }),
                };
                if !matches!(result, Err(Error::Cancelled)) {
                    let _ = events.send((generation, result));
                    ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
                    ctx.request_repaint();
                }
            }
        });
        Self {
            generation,
            input: Some(input),
            output,
            thread: Some(thread),
        }
    }
    pub fn submit(&self, task: Task) {
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let _ = self.input.as_ref().unwrap().send((generation, task));
    }
    pub fn cancel(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }
    pub fn poll(&self) -> Vec<Result<Event>> {
        self.output
            .try_iter()
            .filter(|(id, _)| *id == self.generation.load(Ordering::Acquire))
            .map(|(_, result)| result)
            .collect()
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel();
        self.input.take();
        // Give a pending old-client action its cancellation before process exit.
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
