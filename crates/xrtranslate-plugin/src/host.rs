use crate::{Environment, Manifest, Message, Result, Widget};
use serde_json::Value;
use std::{
    collections::{BTreeMap, VecDeque},
    io::{BufRead, BufReader, BufWriter, Write},
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread::JoinHandle,
    time::Instant,
};

#[derive(Default)]
struct State {
    view: Vec<Widget>,
    replies: BTreeMap<u64, (Instant, Option<Result>)>,
    events: VecDeque<Message>,
    error: Option<String>,
    #[cfg(feature = "egui")]
    frames: VecDeque<crate::surface::Frame>,
}

pub struct Process {
    child: Child,
    output: Option<crate::mailbox::Sender>,
    state: Arc<Mutex<State>>,
    threads: Vec<JoinHandle<()>>,
    next_id: u64,
}
impl Process {
    pub fn start(
        executable: &Path,
        manifest: &Manifest,
        environment: Environment,
        wake: impl Fn() + Send + 'static,
    ) -> std::result::Result<Self, String> {
        let process = Self::spawn(executable, manifest, &environment.host_executable, wake)?;
        process.initialize(environment)?;
        Ok(process)
    }

    /// Launch without initializing; hosts may perform the blocking launch on a
    /// worker, then supply the current environment when accepting the result.
    pub fn spawn(
        executable: &Path,
        manifest: &Manifest,
        host_executable: &Path,
        wake: impl Fn() + Send + 'static,
    ) -> std::result::Result<Self, String> {
        manifest.validate()?;
        let _ = host_executable;
        let mut command = Command::new(executable);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
            // Share the app-local CRT even when launched from another directory.
            if let Some(directory) = host_executable.parent() {
                let path = std::env::var_os("PATH").unwrap_or_default();
                let paths =
                    std::iter::once(directory.to_owned()).chain(std::env::split_paths(&path));
                command.env(
                    "PATH",
                    std::env::join_paths(paths).map_err(|e| e.to_string())?,
                );
            }
        }
        let mut child = command.spawn().map_err(|e| e.to_string())?;
        let mut input = BufWriter::new(child.stdin.take().unwrap());
        let output = child.stdout.take().unwrap();
        let errors = child.stderr.take().unwrap();
        let state = Arc::new(Mutex::new(State::default()));
        let (tx, rx) = crate::mailbox::channel();
        let write_state = state.clone();
        let writer = std::thread::spawn(move || {
            loop {
                let message = match rx.recv() {
                    Ok(message) => message,
                    Err(_) => break,
                };
                if let Err(error) = serde_json::to_writer(&mut input, &message)
                    .map_err(std::io::Error::other)
                    .and_then(|_| input.write_all(b"\n"))
                    .and_then(|_| input.flush())
                {
                    write_state.lock().unwrap().error = Some(format!("Plugin input: {error}"));
                    break;
                }
            }
        });
        let read_state = state.clone();
        let reader = std::thread::spawn(move || {
            let mut output = BufReader::new(output);
            loop {
                #[cfg(feature = "egui")]
                if output
                    .fill_buf()
                    .is_ok_and(|bytes| bytes.first() == Some(&0))
                {
                    output.consume(1);
                    let frame = crate::surface::wire::read(&mut output);
                    let mut state = read_state.lock().unwrap();
                    match frame {
                        Ok(frame) if state.frames.len() < 8 => state.frames.push_back(frame),
                        Ok(_) => {
                            state.error = Some("Too many pending plugin frames".into());
                            break;
                        }
                        Err(error) => {
                            state.error = Some(format!("Plugin frame: {error}"));
                            break;
                        }
                    }
                    drop(state);
                    wake();
                    continue;
                }
                let mut bytes = Vec::new();
                use std::io::Read;
                let read = output
                    .by_ref()
                    .take(crate::MAX_MESSAGE_BYTES as u64 + 1)
                    .read_until(b'\n', &mut bytes);
                let message = match read {
                    Ok(0) => {
                        read_state
                            .lock()
                            .unwrap()
                            .error
                            .get_or_insert("Plugin exited".into());
                        break;
                    }
                    Ok(n) if n <= crate::MAX_MESSAGE_BYTES => {
                        serde_json::from_slice::<Message>(&bytes).map_err(|e| e.to_string())
                    }
                    Ok(_) => Err("Plugin message is too large".into()),
                    Err(error) => Err(error.to_string()),
                };
                let mut state = read_state.lock().unwrap();
                match message {
                    Ok(Message::View { widgets }) => state.view = widgets,
                    Ok(Message::Reply { id, result }) => {
                        if let Some((_, reply)) = state.replies.get_mut(&id) {
                            *reply = Some(result);
                        }
                    }
                    Ok(
                        message @ (Message::Call { .. }
                        | Message::Event { .. }
                        | Message::Cancel { .. }),
                    ) if state.events.len() < 128 => state.events.push_back(message),
                    Ok(_) => {
                        state.error =
                            Some("Plugin sent an unexpected message or too many requests".into());
                        break;
                    }
                    Err(error) => {
                        state.error = Some(format!("Plugin output: {error}"));
                        break;
                    }
                }
                drop(state);
                wake();
            }
            wake();
        });
        let plugin = manifest.id.clone();
        let error_state = state.clone();
        let stderr = std::thread::spawn(move || {
            for line in BufReader::new(errors)
                .lines()
                .map_while(std::result::Result::ok)
            {
                log::info!("Plugin {plugin}: {line}");
                // Keep the final diagnostic when the process exits unexpectedly.
                if line.starts_with("Error:") {
                    error_state.lock().unwrap().error = Some(line.chars().take(2048).collect());
                }
            }
        });
        Ok(Self {
            child,
            output: Some(tx),
            state,
            threads: vec![writer, reader, stderr],
            next_id: 0,
        })
    }
    pub fn initialize(&self, environment: Environment) -> std::result::Result<(), String> {
        #[cfg(feature = "egui")]
        let environment = {
            let mut environment = environment;
            environment
                .capabilities
                .push(crate::surface::wire::CAPABILITY.into());
            environment
        };
        self.send(Message::Initialize { environment })
    }
    pub fn send(&self, message: Message) -> std::result::Result<(), String> {
        match self
            .output
            .as_ref()
            .ok_or("Plugin is stopped")?
            .try_send(message)
        {
            Ok(()) => Ok(()),
            // Backpressure is not a process failure. Calls may retry without losing the plugin.
            Err(mpsc::TrySendError::Full(_)) => Err("Plugin is busy. Please try again.".into()),
            Err(mpsc::TrySendError::Disconnected(_)) => {
                let error = "Plugin is stopped".to_owned();
                self.state.lock().unwrap().error = Some(error.clone());
                Err(error)
            }
        }
    }
    pub fn request(&mut self, name: &str, arguments: Value) -> std::result::Result<u64, String> {
        let mut state = self.state.lock().unwrap();
        if let Some(error) = &state.error {
            return Err(error.clone());
        }
        if state.replies.len() >= 128 {
            return Err("Too many pending plugin operations".into());
        }
        self.next_id += 1;
        let id = self.next_id;
        state.replies.insert(id, (Instant::now(), None));
        drop(state);
        if let Err(error) = self.send(Message::Call {
            id,
            name: name.into(),
            arguments,
        }) {
            self.state.lock().unwrap().replies.remove(&id);
            return Err(error);
        }
        Ok(id)
    }
    pub fn result(&mut self, id: u64) -> Option<Result> {
        let mut state = self.state.lock().unwrap();
        let error = state.error.clone();
        let Some((started, result)) = state.replies.get_mut(&id) else {
            return Some(Err("Unknown plugin operation".into()));
        };
        let result = result.take().or_else(|| error.map(Err)).or_else(|| {
            (started.elapsed().as_secs() > 180).then(|| Err("Plugin operation timed out".into()))
        });
        if result.is_some() {
            state.replies.remove(&id);
        }
        result
    }
    pub fn cancel(&mut self, id: u64) {
        self.state.lock().unwrap().replies.remove(&id);
        let _ = self.send(Message::Cancel { id });
    }
    pub fn events(&mut self) -> Vec<Message> {
        self.state.lock().unwrap().events.drain(..).collect()
    }
    #[cfg(feature = "egui")]
    pub fn frames(&mut self) -> Vec<crate::surface::Frame> {
        self.state.lock().unwrap().frames.drain(..).collect()
    }
    pub fn view(&self) -> Vec<Widget> {
        self.state.lock().unwrap().view.clone()
    }
    pub fn error(&self) -> Option<String> {
        self.state.lock().unwrap().error.clone()
    }
    pub fn exit_status(&mut self) -> Option<ExitStatus> {
        self.child.try_wait().ok().flatten()
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        self.output.take();
        // Closing stdin lets plugin workers flush their owned data. A stuck plugin
        // still cannot outlive removal or shutdown.
        let deadline = Instant::now() + std::time::Duration::from_millis(250);
        while self.child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}
