//! HTTP setup shared by the application and native worker processes.
#![forbid(unsafe_code)]

pub fn client_builder() -> reqwest::ClientBuilder {
    let builder = reqwest::Client::builder();
    #[cfg(target_os = "android")]
    {
        let tls = rustls::ClientConfig::builder_with_provider(
            rustls::crypto::aws_lc_rs::default_provider().into(),
        )
        .with_safe_default_protocol_versions()
        .expect("default TLS protocol versions are supported")
        .with_root_certificates(rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        })
        .with_no_client_auth();
        builder
            .use_preconfigured_tls(tls)
            .proxy(reqwest::Proxy::custom(android_proxy))
    }
    #[cfg(not(target_os = "android"))]
    builder
}

#[cfg(target_os = "android")]
fn android_proxy(url: &reqwest::Url) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct Settings {
        url: String,
        exclusions: Vec<String>,
    }
    let host = url.host_str()?;
    // Local services and OAuth callbacks must never leave this device.
    if host == "localhost"
        || host
            .trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
    {
        return None;
    }
    // Android atomically refreshes this small file when the system proxy changes.
    // Native workers inherit its path and do not need a Java VM or mutable env.
    let path = std::env::var_os("XRT_ANDROID_PROXY_FILE")?;
    let settings: Settings = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    for pattern in settings.exclusions {
        let pattern = format!("(?i)^{}$", regex::escape(&pattern).replace("\\*", ".*"));
        if regex::Regex::new(&pattern).ok()?.is_match(host) {
            return None;
        }
    }
    (!settings.url.is_empty()).then_some(settings.url)
}
