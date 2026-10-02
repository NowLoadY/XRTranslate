use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, SyncSender, channel, sync_channel},
};
use std::thread;

use crate::overlay_ipc::{
    OcrOverlayState, OverlayCommand, OverlayControls, OverlayEvent, OverlayState,
};

#[derive(Clone, Default)]
struct Snapshot {
    language: crate::i18n::UiLanguage,
    controls: OverlayControls,
    subtitles: Option<OverlayState>,
    ocr: Option<OcrOverlayState>,
}

struct Writer {
    snapshot: Arc<Mutex<Snapshot>>,
    wake: SyncSender<()>,
    failed: Arc<AtomicBool>,
}

enum ChildEvent {
    Ui(OverlayEvent),
    OutputClosed,
}

pub struct OverlayManager {
    child: Option<Child>,
    writer: Option<Writer>,
    event_rx: Option<Receiver<ChildEvent>>,
    output_closed: bool,
    pending_events: Vec<OverlayEvent>,
    snapshot: Snapshot,
    restart_blocked: bool,
}

impl OverlayManager {
    pub fn new() -> Self {
        Self {
            child: None,
            writer: None,
            event_rx: None,
            output_closed: false,
            pending_events: Vec::new(),
            snapshot: Snapshot::default(),
            restart_blocked: false,
        }
    }

    pub fn start(&mut self) {
        self.check_process();
        if self.restart_blocked || self.child.is_some() {
            return;
        }
        if let Err(error) = self.spawn() {
            self.fail(format!("Unable to open the floating window: {error}"));
        }
    }

    fn spawn(&mut self) -> std::io::Result<()> {
        let mut command = Command::new(std::env::current_exe()?);
        #[cfg(target_os = "linux")]
        crate::overlay_native::configure_software_environment(&mut command)?;
        let mut child = crate::child_process::hide_console(&mut command)
            .arg("--overlay")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let stdout = child.stdout.take().expect("piped overlay stdout");
        let stdin = child.stdin.take().expect("piped overlay stdin");
        let (tx, rx) = channel();
        self.event_rx = Some(rx);
        self.output_closed = false;
        self.writer = Some(Writer::new(stdin, self.snapshot.clone()));
        self.child = Some(child);
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(event) = serde_json::from_str::<OverlayEvent>(&line)
                    && tx.send(ChildEvent::Ui(event)).is_err()
                {
                    break;
                }
            }
            let _ = tx.send(ChildEvent::OutputClosed);
        });
        self.publish();
        Ok(())
    }

    /// Explicitly disabling the window permits a later user-requested start.
    pub fn stop(&mut self) {
        self.stop_process();
        self.event_rx = None;
        self.pending_events.clear();
        self.restart_blocked = false;
    }

    fn stop_process(&mut self) {
        self.writer = None;
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            // A stalled graphics driver must not make the UI wait for teardown.
            thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }

    fn fail(&mut self, message: String) {
        if !self.restart_blocked {
            self.pending_events.push(OverlayEvent::Failed(message));
            self.restart_blocked = true;
        }
        self.stop_process();
    }

    fn check_process(&mut self) {
        if let Some(receiver) = &self.event_rx {
            for event in receiver.try_iter() {
                match event {
                    ChildEvent::Ui(event) => self.pending_events.push(event),
                    ChildEvent::OutputClosed => self.output_closed = true,
                }
            }
        }
        if self
            .pending_events
            .iter()
            .any(|event| matches!(event, OverlayEvent::CloseRequested))
        {
            self.pending_events
                .retain(|event| !matches!(event, OverlayEvent::Failed(_)));
            self.restart_blocked = true;
            self.stop_process();
        }
        if self.restart_blocked {
            return;
        }
        if let Some(child) = self.child.as_mut() {
            match child.try_wait() {
                // stdout owns the final close notification. Wait for its EOF
                // marker before classifying an exit, even a nonzero one.
                Ok(Some(_)) if !self.output_closed => return,
                Ok(Some(status)) if status.success() => {
                    self.pending_events.push(OverlayEvent::CloseRequested);
                    self.restart_blocked = true;
                    self.stop_process();
                }
                Ok(Some(status)) => self.fail(format!("The floating window stopped ({status}).")),
                Err(error) => self.fail(format!("Unable to check the floating window: {error}")),
                Ok(None) => {}
            }
        }
        if self.output_closed
            && self
                .writer
                .as_ref()
                .is_some_and(|writer| writer.failed.load(Ordering::Acquire))
        {
            self.fail("The floating window stopped receiving updates.".into());
        }
    }

    pub fn set_language(&mut self, language: crate::i18n::UiLanguage) {
        if self.snapshot.language != language {
            self.snapshot.language = language;
            self.publish();
        }
    }

    pub fn set_controls(&mut self, controls: OverlayControls) {
        if self.snapshot.controls != controls {
            self.snapshot.controls = controls;
            self.publish();
        }
    }

    pub fn send_state(&mut self, state: &OverlayState) {
        if self.snapshot.subtitles.as_ref() != Some(state) {
            self.snapshot.subtitles = Some(state.clone());
            self.publish();
        }
    }

    #[cfg(any(windows, target_os = "linux"))]
    pub fn send_ocr(&mut self, state: Option<OcrOverlayState>) {
        if self.snapshot.ocr != state {
            self.snapshot.ocr = state;
            self.publish();
        }
    }

    fn publish(&self) {
        if let Some(writer) = &self.writer {
            *writer
                .snapshot
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = self.snapshot.clone();
            // One wake-up is sufficient: the writer always takes the latest
            // complete state, so a slow window cannot build an update backlog.
            let _ = writer.wake.try_send(());
        }
    }

    pub fn poll_events(&mut self) -> Vec<OverlayEvent> {
        self.check_process();
        std::mem::take(&mut self.pending_events)
    }
}

impl Writer {
    fn new(mut stdin: ChildStdin, initial: Snapshot) -> Self {
        let snapshot = Arc::new(Mutex::new(initial));
        let pending = Arc::clone(&snapshot);
        let failed = Arc::new(AtomicBool::new(false));
        let fault = Arc::clone(&failed);
        let (wake, receiver) = sync_channel(1);
        thread::spawn(move || {
            while receiver.recv().is_ok() {
                let state = pending
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .clone();
                let language = state.language;
                let commands = state
                    .subtitles
                    .map(OverlayCommand::Subtitles)
                    .into_iter()
                    .chain([
                        OverlayCommand::Ocr(state.ocr),
                        OverlayCommand::Language(language),
                        OverlayCommand::Controls(state.controls),
                    ]);
                for command in commands {
                    let result = serde_json::to_writer(&mut stdin, &command)
                        .map_err(std::io::Error::other)
                        .and_then(|()| stdin.write_all(b"\n"));
                    if result.is_err() {
                        fault.store(true, Ordering::Release);
                        return;
                    }
                }
            }
        });
        Self {
            snapshot,
            wake,
            failed,
        }
    }
}

impl Drop for OverlayManager {
    fn drop(&mut self) {
        self.stop_process();
    }
}
