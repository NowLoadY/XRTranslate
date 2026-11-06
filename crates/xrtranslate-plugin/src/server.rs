use crate::{Environment, Message, Result, Widget};
use serde_json::Value;
use std::{
    io::{self, BufRead, Write},
    sync::mpsc,
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
    output: io::BufWriter<io::Stdout>,
    client: crate::Client,
}
impl Drop for Context {
    fn drop(&mut self) {
        // Wake workers before their plugin joins them during shutdown.
        self.client.disconnect();
    }
}
impl Context {
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
        // A closed host pipe ends the plugin; stdout is reserved for the protocol.
        if serde_json::to_writer(&mut self.output, message).is_err()
            || self.output.write_all(b"\n").is_err()
            || self.output.flush().is_err()
        {
            std::process::exit(0);
        }
    }
}

pub fn serve(mut plugin: impl Plugin) -> std::result::Result<(), String> {
    let (tx, rx) = mpsc::sync_channel(64);
    std::thread::spawn(move || {
        use std::io::Read;
        let mut input = io::stdin().lock();
        loop {
            let mut bytes = Vec::new();
            match input
                .by_ref()
                .take(8 * 1024 * 1024 + 1)
                .read_until(b'\n', &mut bytes)
            {
                Ok(1..=8_388_608) => {}
                _ => break,
            }
            let Ok(message) = serde_json::from_slice::<Message>(&bytes) else {
                break;
            };
            if tx.send(message).is_err() {
                break;
            }
        }
    });
    let (client, outbox) = crate::Client::channel();
    let mut context = Context {
        output: io::BufWriter::new(io::stdout()),
        client,
    };
    let mut clock = Instant::now();
    let mut initialized = false;
    loop {
        match rx.recv_timeout(Duration::from_millis(16)) {
            Ok(Message::Initialize { environment }) => {
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
                if !context.client.deliver(id, result.clone()) {
                    plugin.reply(id, result, &mut context)
                }
            }
            Ok(Message::Cancel { id }) if initialized => plugin.cancel(id, &mut context),
            Ok(_) => return Err("Unexpected host message".into()),
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        for message in outbox.try_iter() {
            context.send(&message);
        }
        for id in context.client.cancelled() {
            context.send(&Message::Cancel { id });
        }
        let now = Instant::now();
        let elapsed = now.duration_since(clock).min(Duration::from_millis(100));
        clock = now;
        if initialized && let Some(widgets) = plugin.tick(elapsed, &mut context) {
            context.send(&Message::View { widgets });
        }
    }
}
