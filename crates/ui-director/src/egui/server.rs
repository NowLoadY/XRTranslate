use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use ::egui;
use crossbeam_channel::bounded;

use super::driver::{Command, Driver, Request};
use crate::Response;

pub struct Server {
    port: u16,
    running: Arc<std::sync::atomic::AtomicBool>,
    egui_ctx: Arc<Mutex<Option<egui::Context>>>,
}

impl Server {
    #[must_use]
    pub fn new(port: u16) -> Self {
        Self {
            port,
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            egui_ctx: Arc::new(Mutex::new(None)),
        }
    }

    pub fn start(&self, driver: Arc<Driver>, egui_ctx: egui::Context) -> Result<(), String> {
        // Android can recreate the UI while this process-wide server is alive.
        let mut current_context = self.egui_ctx.lock().unwrap();
        *current_context = Some(egui_ctx);
        if self.running.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(());
        }
        let addr = format!("127.0.0.1:{}", self.port);
        let listener = match TcpListener::bind(&addr) {
            Ok(l) => l,
            Err(e) => {
                log::warn!("Could not bind Director Server on {addr}: {e}");
                return Err(format!("Could not bind Director Server on {addr}: {e}"));
            }
        };
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;

        let running = self.running.clone();
        running.store(true, std::sync::atomic::Ordering::Relaxed);
        let port = self.port;
        let egui_ctx = self.egui_ctx.clone();

        let started = thread::Builder::new()
            .name("ui-director-server".into())
            .spawn(move || {
                log::info!("UI Director Server listening on http://127.0.0.1:{port}");
                while running.load(std::sync::atomic::Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _addr)) => {
                            let driver_clone = driver.clone();
                            let ctx_clone = egui_ctx.clone();
                            thread::spawn(move || {
                                handle_client(stream, driver_clone, ctx_clone);
                            });
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(20));
                        }
                        Err(e) => {
                            log::debug!("Director server accept error: {e}");
                            thread::sleep(Duration::from_millis(50));
                        }
                    }
                }
            })
            .map_err(|e| e.to_string());
        if let Err(error) = started {
            self.running
                .store(false, std::sync::atomic::Ordering::Relaxed);
            return Err(error);
        }

        Ok(())
    }
}

fn handle_client(
    mut stream: TcpStream,
    driver: Arc<Driver>,
    egui_ctx: Arc<Mutex<Option<egui::Context>>>,
) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));

    let mut reader = BufReader::new(stream.try_clone().expect("clone tcp stream"));
    let mut line = String::new();
    let mut timed_capture = false;
    while reader.read_line(&mut line).unwrap_or(0) > 0 {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            line.clear();
            continue;
        }
        let Some(egui_ctx) = egui_ctx.lock().unwrap().clone() else {
            break;
        };

        if let Some(time) = trimmed.strip_prefix("frame ") {
            if !timed_capture {
                // A recorder may deliberately hold a frame while real inference completes.
                let _ = stream.set_read_timeout(None);
            }
            timed_capture = true;
            let result = time
                .parse::<f64>()
                .map_err(std::io::Error::other)
                .and_then(|time| super::capture::timed_frame(&mut stream, &egui_ctx, time));
            if result.is_err() {
                break;
            }
            line.clear();
            continue;
        }

        // Check if this is an HTTP request
        if trimmed.starts_with("GET ")
            || trimmed.starts_with("POST ")
            || trimmed.starts_with("OPTIONS ")
        {
            handle_http_request(&mut stream, &mut reader, trimmed, &driver, &egui_ctx);
            break;
        }

        break;
    }
    if timed_capture && let Some(ctx) = egui_ctx.lock().unwrap().clone() {
        super::capture::resume_clock(&ctx);
    }
}

fn handle_http_request(
    stream: &mut TcpStream,
    reader: &mut BufReader<TcpStream>,
    initial_line: &str,
    driver: &Arc<Driver>,
    egui_ctx: &egui::Context,
) {
    let mut content_length: usize = 0;
    let mut browser_origin = false;

    let parts: Vec<&str> = initial_line.split_whitespace().collect();
    let method = parts.first().copied().unwrap_or("GET");
    let path = parts.get(1).copied().unwrap_or("/");

    let mut line = String::new();
    while reader.read_line(&mut line).unwrap_or(0) > 0 {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        if trimmed.to_lowercase().starts_with("content-length:") {
            if let Some(val_str) = trimmed.split(':').nth(1) {
                content_length = val_str.trim().parse().unwrap_or(0);
            }
        }
        browser_origin |= trimmed.to_ascii_lowercase().starts_with("origin:");
        line.clear();
    }

    if method == "OPTIONS" {
        let response = "HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\n\r\n";
        let _ = stream.write_all(response.as_bytes());
        return;
    }

    // Local automation clients need no browser-origin access or unbounded bodies.
    if content_length > 1_048_576 {
        let _ = stream.write_all(
            b"HTTP/1.1 413 Payload Too Large\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        return;
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 && std::io::Read::read_exact(reader, &mut body).is_err() {
        let _ = stream.write_all(
            b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        return;
    }
    let body_str = String::from_utf8_lossy(&body);

    if method == "GET" && path == "/frame" {
        super::capture::respond(stream, egui_ctx);
        return;
    }

    let director_response = if browser_origin {
        Response::err("Operations are available to local automation clients only")
    } else if method == "POST" {
        match serde_json::from_str::<crate::ToolCall>(&body_str) {
            Ok(call) => execute_driver_command(Command::Call(call), driver, egui_ctx),
            Err(error) => Response::err(format!("Invalid operation: {error}")),
        }
    } else {
        Response::err("POST a tool call; GET /frame captures the viewport")
    };

    let json_body = serde_json::to_string(&director_response).unwrap_or_else(|_| "{}".into());
    let http_response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        json_body.len(),
        json_body
    );
    let _ = stream.write_all(http_response.as_bytes());
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

fn execute_driver_command(
    command: Command,
    driver: &Arc<Driver>,
    egui_ctx: &egui::Context,
) -> Response {
    let (responder, receiver) = bounded(1);
    let envelope = Request {
        command,
        responder,
        expires: std::time::Instant::now() + Duration::from_secs(5),
        cancelled: Default::default(),
    };
    if driver.channel().try_send(envelope).is_err() {
        return Response::err("Automation queue is full or closed");
    }

    super::request_frame(egui_ctx);

    // Wait up to 5 seconds for execution
    match receiver.recv_timeout(Duration::from_secs(5)) {
        Ok(resp) => resp,
        Err(_) => Response::err("Command execution timed out"),
    }
}
