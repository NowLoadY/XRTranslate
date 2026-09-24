use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crossbeam_channel::bounded;
use eframe::egui;

use super::driver::{AutomationDriver, CommandEnvelope, DirectorCommand, DirectorResponse};
use super::registry::ElementValue;

pub const DEFAULT_DIRECTOR_PORT: u16 = 18920;

pub struct DirectorServer {
    port: u16,
    running: Arc<std::sync::atomic::AtomicBool>,
}

impl DirectorServer {
    #[must_use]
    pub fn new(port: u16) -> Self {
        Self {
            port,
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    pub fn start(&self, driver: Arc<AutomationDriver>, egui_ctx: egui::Context) -> Result<(), String> {
        let addr = format!("127.0.0.1:{}", self.port);
        let listener = match TcpListener::bind(&addr) {
            Ok(l) => l,
            Err(e) => {
                log::warn!("Could not bind Director Server on {addr}: {e}");
                return Err(format!("Could not bind Director Server on {addr}: {e}"));
            }
        };
        listener
            .set_nonblocking(true)
            .map_err(|e| e.to_string())?;

        let running = self.running.clone();
        running.store(true, std::sync::atomic::Ordering::Relaxed);
        let port = self.port;

        thread::Builder::new()
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
            .map_err(|e| e.to_string())?;

        Ok(())
    }
}

fn handle_client(mut stream: TcpStream, driver: Arc<AutomationDriver>, egui_ctx: egui::Context) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));

    let mut reader = BufReader::new(stream.try_clone().expect("clone tcp stream"));
    let mut line = String::new();
    while reader.read_line(&mut line).unwrap_or(0) > 0 {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            line.clear();
            continue;
        }

        // Check if this is an HTTP request
        if trimmed.starts_with("GET ") || trimmed.starts_with("POST ") || trimmed.starts_with("OPTIONS ") {
            handle_http_request(&mut stream, &mut reader, trimmed, &driver, &egui_ctx);
            break;
        }

        // Otherwise, process as plain line-based CLI/TCP command
        let response = process_line_command(trimmed, &driver, &egui_ctx);
        let serialized = serde_json::to_string(&response).unwrap_or_else(|_| "{}".into());
        if writeln!(stream, "{serialized}").is_err() {
            break;
        }
        let _ = stream.flush();
        line.clear();
    }
}

fn handle_http_request(
    stream: &mut TcpStream,
    reader: &mut BufReader<TcpStream>,
    initial_line: &str,
    driver: &Arc<AutomationDriver>,
    egui_ctx: &egui::Context,
) {
    let mut content_length: usize = 0;

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
        line.clear();
    }

    if method == "OPTIONS" {
        let response = "HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\n\r\n";
        let _ = stream.write_all(response.as_bytes());
        return;
    }

    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        let _ = std::io::Read::read_exact(reader, &mut body);
    }
    let body_str = String::from_utf8_lossy(&body);

    let director_response = if method == "POST" || !body_str.trim().is_empty() {
        if let Ok(cmd) = serde_json::from_str::<DirectorCommand>(&body_str) {
            execute_driver_command(cmd, driver, egui_ctx)
        } else if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(&body_str) {
            parse_json_value_command(json_val, driver, egui_ctx)
        } else {
            DirectorResponse::err(format!("Invalid JSON body: '{body_str}'"))
        }
    } else if path == "/list" {
        execute_driver_command(DirectorCommand::List { filter: None }, driver, egui_ctx)
    } else if path == "/status" {
        execute_driver_command(DirectorCommand::Status, driver, egui_ctx)
    } else if path == "/page" {
        execute_driver_command(DirectorCommand::GetPage, driver, egui_ctx)
    } else {
        DirectorResponse::ok("XRTranslate UI Director Server Active", None)
    };

    let json_body = serde_json::to_string(&director_response).unwrap_or_else(|_| "{}".into());
    let http_response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        json_body.len(),
        json_body
    );
    let _ = stream.write_all(http_response.as_bytes());
    let _ = stream.flush();
}

fn parse_json_value_command(
    json: serde_json::Value,
    driver: &Arc<AutomationDriver>,
    egui_ctx: &egui::Context,
) -> DirectorResponse {
    let cmd_str = json
        .get("cmd")
        .or_else(|| json.get("action"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    match cmd_str.to_lowercase().as_str() {
        "page" => {
            let target = json
                .get("args")
                .or_else(|| json.get("target"))
                .or_else(|| json.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            execute_driver_command(DirectorCommand::Page(target.to_string()), driver, egui_ctx)
        }
        "get_page" => execute_driver_command(DirectorCommand::GetPage, driver, egui_ctx),
        "list" => {
            let filter = json.get("filter").and_then(|v| v.as_str()).map(str::to_owned);
            execute_driver_command(DirectorCommand::List { filter }, driver, egui_ctx)
        }
        "inspect" => {
            let target = json
                .get("args")
                .or_else(|| json.get("target"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            execute_driver_command(DirectorCommand::Inspect(target.to_string()), driver, egui_ctx)
        }
        "click" => {
            let target = json
                .get("args")
                .or_else(|| json.get("target"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            execute_driver_command(DirectorCommand::Click(target.to_string()), driver, egui_ctx)
        }
        "set" => {
            let target = json
                .get("target")
                .or_else(|| json.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let val = json.get("value").cloned().unwrap_or(serde_json::Value::Null);
            let elem_val = match val {
                serde_json::Value::Bool(b) => ElementValue::Bool(b),
                serde_json::Value::Number(n) => {
                    ElementValue::Number(n.as_f64().unwrap_or(0.0))
                }
                serde_json::Value::String(s) => ElementValue::Text(s),
                _ => ElementValue::None,
            };
            execute_driver_command(
                DirectorCommand::Set {
                    target: target.to_string(),
                    value: elem_val,
                },
                driver,
                egui_ctx,
            )
        }
        "get" => {
            let target = json
                .get("args")
                .or_else(|| json.get("target"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            execute_driver_command(DirectorCommand::Get(target.to_string()), driver, egui_ctx)
        }
        "status" => execute_driver_command(DirectorCommand::Status, driver, egui_ctx),
        "wait" => {
            let ms = json
                .get("ms")
                .or_else(|| json.get("args"))
                .and_then(|v| v.as_u64())
                .unwrap_or(1000);
            thread::sleep(Duration::from_millis(ms));
            DirectorResponse::ok(format!("Waited {ms}ms"), None)
        }
        _ => DirectorResponse::err(format!("Unknown command: {cmd_str}")),
    }
}

pub fn process_line_command(
    line: &str,
    driver: &Arc<AutomationDriver>,
    egui_ctx: &egui::Context,
) -> DirectorResponse {
    let tokens = tokenize_command_line(line);
    if tokens.is_empty() {
        return DirectorResponse::err("Empty command");
    }

    let cmd = tokens[0].to_lowercase();
    match cmd.as_str() {
        "page" => {
            if tokens.len() < 2 {
                DirectorResponse::err("Usage: page <name>")
            } else {
                execute_driver_command(DirectorCommand::Page(tokens[1].clone()), driver, egui_ctx)
            }
        }
        "get_page" => execute_driver_command(DirectorCommand::GetPage, driver, egui_ctx),
        "list" => {
            let filter = tokens.get(1).cloned();
            execute_driver_command(DirectorCommand::List { filter }, driver, egui_ctx)
        }
        "inspect" => {
            if tokens.len() < 2 {
                DirectorResponse::err("Usage: inspect <target>")
            } else {
                execute_driver_command(DirectorCommand::Inspect(tokens[1].clone()), driver, egui_ctx)
            }
        }
        "click" => {
            if tokens.len() < 2 {
                DirectorResponse::err("Usage: click <target>")
            } else {
                execute_driver_command(DirectorCommand::Click(tokens[1].clone()), driver, egui_ctx)
            }
        }
        "set" => {
            if tokens.len() < 3 {
                DirectorResponse::err("Usage: set <target> <value>")
            } else {
                let target = tokens[1].clone();
                let raw_val = tokens[2].clone();
                let val = if raw_val.eq_ignore_ascii_case("true") {
                    ElementValue::Bool(true)
                } else if raw_val.eq_ignore_ascii_case("false") {
                    ElementValue::Bool(false)
                } else if let Ok(num) = raw_val.parse::<f64>() {
                    ElementValue::Number(num)
                } else {
                    ElementValue::Text(raw_val)
                };
                execute_driver_command(DirectorCommand::Set { target, value: val }, driver, egui_ctx)
            }
        }
        "get" => {
            if tokens.len() < 2 {
                DirectorResponse::err("Usage: get <target>")
            } else {
                execute_driver_command(DirectorCommand::Get(tokens[1].clone()), driver, egui_ctx)
            }
        }
        "status" => execute_driver_command(DirectorCommand::Status, driver, egui_ctx),
        "wait" => {
            let ms = tokens.get(1).and_then(|s| s.parse::<u64>().ok()).unwrap_or(1000);
            thread::sleep(Duration::from_millis(ms));
            DirectorResponse::ok(format!("Waited {ms} ms"), None)
        }
        _ => DirectorResponse::err(format!("Unknown command '{cmd}'. Available: page, get_page, list, inspect, click, set, get, status, wait")),
    }
}

fn execute_driver_command(
    command: DirectorCommand,
    driver: &Arc<AutomationDriver>,
    egui_ctx: &egui::Context,
) -> DirectorResponse {
    let (responder, receiver) = bounded(1);
    let envelope = CommandEnvelope {
        command,
        responder,
    };
    if driver.channel().send(envelope).is_err() {
        return DirectorResponse::err("Automation driver channel closed");
    }

    // Request immediate repaint on egui thread
    egui_ctx.request_repaint();

    // Wait up to 5 seconds for execution
    match receiver.recv_timeout(Duration::from_secs(5)) {
        Ok(resp) => resp,
        Err(_) => DirectorResponse::err("Command execution timed out"),
    }
}

fn tokenize_command_line(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut quote_char = '"';

    for c in input.chars() {
        match c {
            '"' | '\'' if !in_quotes => {
                in_quotes = true;
                quote_char = c;
            }
            c if in_quotes && c == quote_char => {
                in_quotes = false;
            }
            ' ' | '\t' if !in_quotes => {
                if !current.is_empty() {
                    tokens.push(current.clone());
                    current.clear();
                }
            }
            _ => {
                current.push(c);
            }
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_tokenizer_handles_quotes_and_spaces() {
        let tokens = tokenize_command_line("click \"Check for Updates\" 123");
        assert_eq!(tokens, vec!["click", "Check for Updates", "123"]);

        let tokens2 = tokenize_command_line("set 'Receive beta updates' true");
        assert_eq!(tokens2, vec!["set", "Receive beta updates", "true"]);
    }
}
