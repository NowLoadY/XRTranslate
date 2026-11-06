use crate::{Environment, Message, Result, Widget};
use serde_json::Value;
use std::{
    io::{self, BufRead, Write},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

pub trait Plugin {
    fn initialize(
        &mut self,
        environment: Environment,
        context: &mut Context,
    ) -> std::result::Result<(), String>;
    fn call(&mut self, name: &str, arguments: Value, context: &mut Context) -> Result;
    fn event(&mut self, _name: &str, _value: Value, _context: &mut Context) {}
    fn reply(&mut self, _id: u64, _result: Result, _context: &mut Context) {}
    fn cancel(&mut self, _id: u64, _context: &mut Context) {}
    fn tick(&mut self, _elapsed: Duration, _context: &mut Context) -> Option<Vec<Widget>> {
        None
    }
}

pub struct Context {
    output: Arc<Mutex<io::BufWriter<io::Stdout>>>,
    client: crate::Client,
    #[cfg(feature = "egui")]
    pub(crate) binary_surfaces: bool,
}
impl Drop for Context {
    fn drop(&mut self) {
        // Wake workers before their plugin joins them during shutdown.
        self.client.disconnect();
    }
}
impl Context {
    #[cfg(feature = "egui")]
    pub(crate) fn surface(&self, frame: &crate::surface::Frame) -> std::result::Result<(), String> {
        crate::surface::wire::write(&mut *self.output.lock().unwrap(), frame)
            .map_err(|error| error.to_string())
    }
    pub fn client(&self) -> crate::Client {
        self.client.clone()
    }
    pub fn call(&mut self, name: &str, arguments: Value) -> u64 {
        let id = self.client.next_id();
        self.send(&Message::Call {
            id,
            name: name.into(),
            arguments,
        });
        id
    }
    pub fn event(&mut self, name: &str, value: Value) {
        self.send(&Message::Event {
            name: name.into(),
            value,
        });
    }
    pub fn view(&mut self, widgets: Vec<Widget>) {
        self.send(&Message::View { widgets });
    }
    pub fn cancel(&mut self, id: u64) {
        self.send(&Message::Cancel { id });
    }
    fn send(&mut self, message: &Message) {
        send(&self.output, message);
    }
}
fn send(output: &Mutex<io::BufWriter<io::Stdout>>, message: &Message) {
    let mut output = output.lock().unwrap();
    // A closed host pipe ends the plugin; stdout is reserved for the protocol.
    if serde_json::to_writer(&mut *output, message).is_err()
        || output.write_all(b"\n").is_err()
        || output.flush().is_err()
    {
        std::process::exit(0);
    }
}

pub fn serve(mut plugin: impl Plugin) -> std::result::Result<(), String> {
    let (tx, rx) = mpsc::sync_channel(64);
    let (client, outbox) = crate::Client::channel();
    let replies = client.clone();
    std::thread::spawn(move || {
        use std::io::Read;
        let mut input = io::stdin().lock();
        loop {
            let mut bytes = Vec::new();
            match input
                .by_ref()
                .take(crate::MAX_MESSAGE_BYTES as u64 + 1)
                .read_until(b'\n', &mut bytes)
            {
                Ok(n) if n > 0 && n <= crate::MAX_MESSAGE_BYTES => {}
                _ => break,
            }
            let Ok(message) = serde_json::from_slice::<Message>(&bytes) else {
                break;
            };
            // Worker replies must progress even while UI code acquires a worker's lock.
            if let Message::Reply { id, result } = &message
                && replies.deliver(*id, result.clone())
            {
                continue;
            }
            if tx.try_send(message).is_err() {
                eprintln!("Plugin input queue unavailable");
                break;
            }
        }
        replies.disconnect();
    });
    let output = Arc::new(Mutex::new(io::BufWriter::new(io::stdout())));
    let writer = output.clone();
    let requests = client.clone();
    std::thread::spawn(move || {
        while !requests.is_closed() {
            if let Ok(message) = outbox.recv_timeout(Duration::from_millis(16)) {
                // Cancellation may overtake a queued call. Never send it afterwards.
                if !matches!(&message, Message::Call { id, .. } if !requests.pending(*id)) {
                    send(&writer, &message);
                }
            }
            for id in requests.cancelled() {
                send(&writer, &Message::Cancel { id });
            }
        }
    });
    let mut context = Context {
        output,
        client,
        #[cfg(feature = "egui")]
        binary_surfaces: false,
    };
    let mut clock = Instant::now();
    let mut initialized = false;
    loop {
        match rx.recv_timeout(Duration::from_millis(16)) {
            Ok(Message::Initialize { environment }) => {
                #[cfg(feature = "egui")]
                {
                    context.binary_surfaces = environment
                        .capabilities
                        .iter()
                        .any(|name| name == crate::surface::wire::CAPABILITY);
                }
                plugin.initialize(environment, &mut context)?;
                initialized = true;
            }
            Ok(Message::Call {
                id,
                name,
                arguments,
            }) if initialized => {
                let result = plugin.call(&name, arguments, &mut context);
                context.send(&Message::Reply { id, result });
            }
            Ok(Message::Event { name, value }) if initialized => {
                plugin.event(&name, value, &mut context)
            }
            Ok(Message::Reply { id, result }) if initialized => {
                plugin.reply(id, result, &mut context)
            }
            Ok(Message::Cancel { id }) if initialized => plugin.cancel(id, &mut context),
            Ok(_) => return Err("Unexpected host message".into()),
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        let now = Instant::now();
        let elapsed = now.duration_since(clock).min(Duration::from_millis(100));
        clock = now;
        if initialized && let Some(widgets) = plugin.tick(elapsed, &mut context) {
            context.send(&Message::View { widgets });
        }
    }
}
