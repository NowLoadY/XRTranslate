use super::*;
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    sync::Mutex,
};

struct Server {
    remote: Remote,
    calls: Arc<Mutex<Vec<Value>>>,
    running: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn new(mut handle: impl FnMut(&Value) -> Value + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let alive = running.clone();
        let calls = Arc::new(Mutex::new(vec![]));
        let history = calls.clone();
        let thread = thread::spawn(move || {
            for stream in listener.incoming() {
                if !alive.load(Ordering::Acquire) {
                    break;
                }
                let mut stream = stream.unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(value) = line.strip_prefix("Content-Length:") {
                        length = value.trim().parse().unwrap();
                    }
                }
                let mut bytes = vec![0; length];
                reader.read_exact(&mut bytes).unwrap();
                let request: Value = serde_json::from_slice(&bytes).unwrap();
                history.lock().unwrap().push(request.clone());
                let data = handle(&request);
                let body = json!({"success":true,"message":"Completed","data":data}).to_string();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        Self {
            remote: Remote {
                port,
                generation: 1,
                current: Arc::new(AtomicU64::new(1)),
            },
            calls,
            running,
            thread: Some(thread),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        let _ = TcpStream::connect(("127.0.0.1", self.remote.port));
        self.thread.take().unwrap().join().unwrap();
    }
}

fn control(id: &str, label: &str, kind: &str, value: Value, enabled: bool) -> Value {
    json!({"id_hex":id,"label":label,"kind":kind,"value":value,"enabled":enabled,
        "rect":[10,10,100,30],"actions":["click"]})
}

#[test]
fn unsupported_client_is_rejected_before_navigation() {
    let server = Server::new(|call| match call["name"].as_str().unwrap() {
        "ui.pages" => json!({"location":{"page":"Translation"},"pages":[]}),
        "tools.describe" => json!({"tools":[]}),
        other => panic!("unexpected mutation: {other}"),
    });
    assert_eq!(
        server.remote.connect(None).unwrap_err(),
        Error::Incompatible
    );
}

#[test]
fn cancelled_selection_cancels_its_pending_ui_operation() {
    let current = Arc::new(AtomicU64::new(1));
    let state = current.clone();
    let mut server = Server::new(move |call| match call["name"].as_str().unwrap() {
        "ui.page" => {
            state.store(2, Ordering::Release);
            json!({"pending":{"name":"ui.operation","arguments":{"operation":"old"}},
                "cancel":{"name":"ui.cancel","arguments":{"operation":"old"}}})
        }
        "ui.cancel" => json!({"status":"cancelled"}),
        other => panic!("cancelled selection continued with {other}"),
    });
    server.remote.current = current;
    assert_eq!(
        server.remote.call("ui.page", json!({"page":"Translation"})),
        Err(Error::Cancelled)
    );
    assert_eq!(server.calls.lock().unwrap()[1]["name"], "ui.cancel");
}

#[test]
fn old_client_uses_only_existing_operations_and_visible_admission() {
    let server = Server::new(|call| match call["name"].as_str().unwrap() {
        "ui.page" => json!({}),
        "ui.read" => json!({"location":{"page":"PromptStudio"},"controls":[
            control("style", "Choose style", "button", Value::Null, true),
            control("edit", "Edit prompts", "button", Value::Null, false)],"next_offset":null}),
        other => panic!("unexpected operation: {other}"),
    });
    let mut planner = xrt_guide::Planner::default();
    planner.update(Context {
        page: "PromptStudio".into(),
        language: "en".into(),
        pages: vec![Page {
            route: "PromptStudio".into(),
            label: "Prompt Studio".into(),
        }],
        capabilities: vec![],
    });
    let step = planner.catalog().unwrap()[0].steps[0].clone();
    assert_eq!(
        server.remote.present("PromptStudio", &step).unwrap(),
        Some(false)
    );
    assert!(
        server
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|call| call["name"].as_str().unwrap().starts_with("ui."))
    );
    assert_eq!(planner.catalog().unwrap()[0].steps.len(), 1);
}

#[test]
fn reveals_disclosure_without_changing_parameter_values() {
    let mut open = false;
    let server = Server::new(move |call| match call["name"].as_str().unwrap() {
        "ui.page" => json!({}),
        "ui.click" => {
            assert_eq!(call["arguments"]["target"], "settings");
            open = true;
            json!({})
        }
        "ui.read" => {
            let mut controls = vec![control(
                "settings",
                "识别设置",
                "checkbox",
                json!(open),
                true,
            )];
            if open {
                controls.push(control("filter", "滤除强度", "slider", json!(0.3), true));
            }
            json!({"location":{"page":"Translation"},"controls":controls,"next_offset":null})
        }
        other => panic!("unexpected parameter mutation: {other}"),
    });
    let step = Step {
        id: "filter-strength".into(),
        title: String::new(),
        text: String::new(),
        targets: vec!["Filter strength".into()],
        reveal: vec!["Recognition settings".into()],
        audio: None,
    };
    assert_eq!(server.remote.present("Translation", &step), Ok(None));
    assert!(
        server
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call["arguments"]["target"] == "filter")
    );
}
