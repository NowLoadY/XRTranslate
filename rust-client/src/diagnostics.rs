//! Request inspection and error snapshots reuse the existing bounded service logs.
use eframe::egui;
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use xrtranslate_client_core::ui::error_report::{self, ErrorReport};
use xrtranslate_prompt::diagnostics::ModelRequestRecord;

/// No snapshot means the local service has not started (or has stopped).
pub(crate) type RequestHistory = Result<Option<Vec<ModelRequestRecord>>, String>;

async fn request_history(server: &str, clear: bool) -> RequestHistory {
    let mut url = reqwest::Url::parse(server).map_err(|error| error.to_string())?;
    let scheme = if matches!(url.scheme(), "wss" | "https") {
        "https"
    } else {
        "http"
    };
    url.set_scheme(scheme)
        .map_err(|_| "Invalid service address".to_owned())?;
    url.set_path("/diagnostics/requests");
    url.set_query(None);
    let local = url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    });
    let mut client = reqwest::Client::builder().timeout(Duration::from_secs(3));
    if local {
        client = client.no_proxy();
    }
    let client = client.build().map_err(|error| error.to_string())?;
    let request = if clear {
        client.delete(url)
    } else {
        client.get(url)
    };
    let response = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            // Refused loopback connections are normal before on-demand startup.
            // Preserve timeouts, TLS, HTTP and decoding failures as real errors.
            let mut source = std::error::Error::source(&error);
            while let Some(cause) = source {
                if local
                    && cause
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|error| error.kind() == std::io::ErrorKind::ConnectionRefused)
                {
                    return Ok(None);
                }
                source = cause.source();
            }
            return Err(format!("{error:?}"));
        }
    };
    response
        .error_for_status()
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map(Some)
        .map_err(|error| format!("{error:?}"))
}

pub(crate) fn fetch_requests(
    server: String,
    clear: bool,
    ctx: egui::Context,
    complete: impl FnOnce(RequestHistory) + Send + 'static,
) {
    std::thread::spawn(move || {
        let result = (|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())?;
            runtime.block_on(request_history(&server, clear))
        })();
        complete(result);
        ctx.request_repaint();
    });
}

pub(crate) fn install(ctx: &egui::Context, root: PathBuf, server: &str) {
    let id = egui::Id::unique("diagnostic_source");
    let source = (root.clone(), server.to_owned());
    if ctx
        .data(|data| data.get_temp::<(PathBuf, String)>(id))
        .as_ref()
        == Some(&source)
    {
        return;
    }
    ctx.data_mut(|data| data.insert_temp(id, source));
    let server = server.to_owned();
    error_report::install(
        ctx,
        Arc::new(move |error, log, ctx| {
            let captured_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let config = xrtranslate_config::AppConfig::from_path_with_user_config(
                root.join("config.json"),
                &root,
            )
            .ok();
            let mut secrets = Vec::new();
            if let Some(config) = &config {
                collect_secrets(&config.raw, &mut secrets);
            }
            secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
            let error = redact(error, &secrets);
            let mut text = format!(
                "{}\n{} {}\nCaptured: {captured_ms} ms since Unix epoch\n\nError:\n{error}\n",
                crate::version::full_metadata_summary(),
                std::env::consts::OS,
                std::env::consts::ARCH
            );
            if let Some(log) = log {
                text.push_str("\n--- Backend log ---\n");
                text.push_str(log);
            }
            if let Some(config) = &config {
                if let Ok(route) = config.native_model_route() {
                    for (label, model) in [("ASR", route.asr), ("Translation", route.translation)] {
                        text.push_str(&format!(
                        "\n{label}: {} / {} / {:?}\nContext: {} tokens; output limit: {} tokens; parallel slots: {}\n",
                        model.provider, model.model, model.model_asset,
                        model.runtime.context_window_tokens, model.runtime.max_tokens, model.runtime.parallel_slots
                        ));
                    }
                }
            }
            let storage = config
                .as_ref()
                .map(|config| config.storage.clone())
                .unwrap_or_default();
            for name in ["client.log", "backend_startup.log", "corpus_startup.log"] {
                let log = crate::logging::LogPolicy::new(&root, storage.clone(), name)
                    .read_current(16 * 1024);
                if !log.is_empty() {
                    text.push_str(&format!("\n--- {name} ---\n{log}"));
                }
            }
            let report = ErrorReport::new(&error, redact(&text, &secrets), true);
            let captured = report.clone();
            fetch_requests(server.clone(), false, ctx.clone(), move |result| {
                let mut report = captured.0.lock();
                match result {
                    Ok(Some(records)) => {
                        for record in records
                            .iter()
                            .rev()
                            .filter(|record| record.started_ms <= captured_ms)
                            .take(3)
                        {
                            report.text.push_str("\n\n--- Model request ---\n");
                            report.text.push_str(&redact(&record.report(), &secrets));
                        }
                    }
                    Ok(None) => report.text.push_str("\n\nModel service is not running."),
                    Err(error) => report.text.push_str(&format!(
                        "\n\nRequest history unavailable: {}",
                        redact(&error, &secrets)
                    )),
                }
                report.collecting = false;
            });
            report
        }),
    );
}

fn collect_secrets(value: &serde_json::Value, secrets: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(values) => {
            for (key, value) in values {
                if matches!(
                    key.to_ascii_lowercase().as_str(),
                    "api_key"
                        | "access_token"
                        | "refresh_token"
                        | "authorization"
                        | "password"
                        | "secret"
                ) {
                    if let Some(secret) = value.as_str().filter(|secret| !secret.is_empty()) {
                        secrets.push(secret.to_owned());
                    }
                } else {
                    collect_secrets(value, secrets);
                }
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                collect_secrets(value, secrets);
            }
        }
        _ => {}
    }
}

fn redact(text: &str, secrets: &[String]) -> String {
    secrets.iter().fold(text.to_owned(), |text, secret| {
        text.replace(secret, "[redacted]")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn diagnostics_reads_a_live_service_and_handles_its_lifecycle() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = format!("ws://{address}/ws?text_only=true");
        let worker = tokio::spawn(async move {
            for (method, status, body) in [
                (
                    "GET",
                    "200 OK",
                    serde_json::json!([{
                        "id": 1, "started_ms": 1, "operation": "Translation", "model": "fixture",
                        "request": {"messages": [{"role": "user", "content": "完整 prompt"}]},
                        "parts": [], "elapsed_ms": 5, "error": null, "output": "result"
                    }])
                    .to_string(),
                ),
                ("DELETE", "200 OK", "[]".into()),
                ("GET", "404 Not Found", "missing endpoint".into()),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0; 2048];
                let size = socket.read(&mut request).await.unwrap();
                assert!(
                    String::from_utf8_lossy(&request[..size])
                        .starts_with(&format!("{method} /diagnostics/requests HTTP/1.1\r\n"))
                );
                socket.write_all(format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()
                ).as_bytes()).await.unwrap();
            }
        });
        let records = request_history(&server, false).await.unwrap().unwrap();
        assert_eq!(records[0].prompt_text(), "[user]\n完整 prompt");
        assert!(
            request_history(&server, true)
                .await
                .unwrap()
                .unwrap()
                .is_empty()
        );
        assert!(
            request_history(&server, false)
                .await
                .unwrap_err()
                .contains("404")
        );
        worker.await.unwrap();
        // An idle local service is distinct from an invalid or incompatible endpoint.
        assert!(request_history(&server, false).await.unwrap().is_none());
    }

    #[test]
    fn diagnostics_redacts_configured_credentials_without_dumping_configuration() {
        let config = serde_json::json!({"translation":{"api_key":"private-api-key"},"accounts":[{"access_token":"private-token"}],"model":"sample-model"});
        let mut secrets = Vec::new();
        collect_secrets(&config, &mut secrets);
        assert_eq!(
            redact("sample-model: private-api-key / private-token", &secrets),
            "sample-model: [redacted] / [redacted]"
        );
    }
}
