//! Blocking entry for catalog workers; all voice requests share URL and error handling.
use serde::de::DeserializeOwned;
use std::time::Duration;

pub(super) fn request<T: DeserializeOwned>(
    server_url: &str,
    path: &str,
    build: impl FnOnce(reqwest::Client, reqwest::Url) -> reqwest::RequestBuilder,
) -> Result<T, String> {
    let mut url = reqwest::Url::parse(server_url).map_err(|e| e.to_string())?;
    let scheme = match url.scheme() {
        "ws" | "http" => "http",
        "wss" | "https" => "https",
        _ => return Err("Unsupported server URL.".into()),
    };
    url.set_scheme(scheme).map_err(|_| "Invalid server URL.")?;
    url.set_path(path);
    url.set_query(None);
    url.set_fragment(None);
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?
        .block_on(async {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .map_err(|e| e.to_string())?;
            let response = build(client, url).send().await.map_err(|e| e.to_string())?;
            if !response.status().is_success() {
                return Err(response.text().await.unwrap_or_else(|e| e.to_string()));
            }
            response.json().await.map_err(|e| e.to_string())
        })
}
