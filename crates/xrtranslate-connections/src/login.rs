//! Provider-specific login pages and extraction scripts. Hosts supply only a
//! dedicated WebView and its native cookie jar; no arbitrary browser import.
use crate::{AccountProvider, AccountSession};
use std::collections::BTreeMap;

pub struct LoginSpec {
    pub label: &'static str,
    pub url: &'static str,
    pub origin: &'static str,
    pub allowed_hosts: &'static [&'static str],
    pub required_attributes: &'static [&'static str],
    pub initialization_script: &'static str,
    pub user_agent: &'static str,
    pub capture_script: &'static str,
}

pub fn spec(provider: AccountProvider) -> Option<&'static LoginSpec> {
    match provider {
        AccountProvider::Doubao => Some(&LoginSpec {
            label: "Doubao",
            url: "https://www.doubao.com/chat",
            origin: "https://www.doubao.com",
            allowed_hosts: &["doubao.com", "douyin.com"],
            required_attributes: &["device_id", "web_id"],
            initialization_script: include_str!("doubao-login.js"),
            user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
            capture_script: "window.__xrAccountCapture ? window.__xrAccountCapture() : null",
        }),
        AccountProvider::Chatgpt => None, // Official system-browser OAuth, not cookie capture.
    }
}

impl LoginSpec {
    pub fn allows_navigation(&self, value: &str) -> bool {
        let Ok(url) = url::Url::parse(value) else {
            return false;
        };
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port_or_known_default() == Some(443)
            && url.host_str().is_some_and(|host| {
                self.allowed_hosts
                    .iter()
                    .any(|allowed| host == *allowed || host.ends_with(&format!(".{allowed}")))
            })
    }

    pub fn is_capture_origin(&self, value: &str) -> bool {
        url::Url::parse(value).is_ok_and(|url| url.origin().ascii_serialization() == self.origin)
    }
}

/// Cookie header comes from the native cookie store, including HttpOnly cookies.
/// Page JavaScript supplies only non-secret device attributes and login evidence.
pub fn captured_session(
    provider: AccountProvider,
    current_url: &str,
    capture: &str,
    cookies: String,
) -> Option<AccountSession> {
    if !spec(provider)?.is_capture_origin(current_url) {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(capture).ok()?;
    if value.get("confirmed")?.as_bool()? != true {
        return None;
    }
    let attributes: BTreeMap<String, String> =
        serde_json::from_value(value.get("attributes")?.clone()).ok()?;
    AccountSession::new(provider, cookies, attributes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_official_https_origins_can_capture_sessions() {
        let spec = spec(AccountProvider::Doubao).unwrap();
        assert!(spec.allows_navigation("https://www.doubao.com/chat"));
        for bad in [
            "https://doubao.com.evil.test",
            "http://www.doubao.com",
            "https://evil.test/?doubao.com",
            "https://www.doubao.com:444/",
        ] {
            assert!(!spec.allows_navigation(bad));
        }
        assert!(!spec.is_capture_origin("https://www.douyin.com"));
        assert!(
            captured_session(
                AccountProvider::Doubao,
                spec.url,
                r#"{"confirmed":false,"attributes":{}}"#,
                "sessionid=x".into()
            )
            .is_none()
        );
    }
}
