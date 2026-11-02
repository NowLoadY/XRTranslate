//! Official Sign in with ChatGPT for open-source/local apps. OAuth credentials
//! and account model discovery live here; inference and UI never own renewal.
mod login;
mod store;

use crate::{AccountModel, AccountStore, ConnectionError as Error};
use reqwest::{Client, Response};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use store::{Credentials, State};

pub use login::sign_in;
pub use store::{select_profile, snapshot};

pub const RESPONSES_ENDPOINT: &str = "https://api.openai.com/v1/responses";
const RESOURCE: &str = "https://api.openai.com/v1";
const ISSUER: &str = "https://auth.openai.com";
const TOKEN_ENDPOINT: &str = "https://auth.openai.com/api/accounts/oauth/token";
const REGISTRATION_CLIENT: &str = "dynamic_agent_client";
const PLAN_SCOPE: &str = "chatgpt.tokens.use.direct";
type Result<T> = std::result::Result<T, Error>;

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn client() -> Result<Client> {
    let builder = Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("XRTranslate/", env!("CARGO_PKG_VERSION")));
    #[cfg(target_os = "android")]
    let builder = builder.use_preconfigured_tls(
        rustls::ClientConfig::builder()
            .with_root_certificates(rustls::RootCertStore {
                roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
            })
            .with_no_client_auth(),
    );
    builder.build().map_err(|_| Error::Network)
}

async fn json<T: serde::de::DeserializeOwned>(mut response: Response) -> Result<T> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| Error::Network)? {
        if bytes.len() + chunk.len() > 1024 * 1024 {
            return Err(Error::InvalidAuthorization);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| Error::InvalidAuthorization)
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    token_type: String,
    expires_in: u64,
    #[serde(default)]
    scope: Option<String>,
}

impl TokenResponse {
    fn credentials(self, previous: Option<&Credentials>) -> Result<Credentials> {
        let scopes = self
            .scope
            .map(|s| s.split_whitespace().map(str::to_owned).collect::<Vec<_>>())
            .or_else(|| previous.map(|p| p.scopes.clone()))
            .unwrap_or_default();
        if !scopes.iter().any(|s| s == PLAN_SCOPE) {
            return Err(Error::PlanRequired);
        }
        if !scopes.iter().any(|s| s == "resource.invoke") {
            return Err(Error::InvalidAuthorization);
        }
        let result = Credentials {
            access_token: self.access_token,
            refresh_token: self
                .refresh_token
                .or_else(|| previous.map(|p| p.refresh_token.clone()))
                .ok_or(Error::InvalidAuthorization)?,
            id_token: self
                .id_token
                .or_else(|| previous.map(|p| p.id_token.clone()))
                .ok_or(Error::InvalidAuthorization)?,
            expires_at: now().saturating_add(self.expires_in.min(86400)),
            scopes,
        };
        if !self.token_type.eq_ignore_ascii_case("bearer")
            || self.expires_in == 0
            || [
                &result.access_token,
                &result.refresh_token,
                &result.id_token,
            ]
            .iter()
            .any(|s| s.is_empty() || s.len() > 65536 || s.bytes().any(|b| b <= 0x20 || b >= 0x7f))
        {
            return Err(Error::InvalidAuthorization);
        }
        Ok(result)
    }
}

async fn exchange(client: &Client, fields: &[(&str, &str)]) -> Result<TokenResponse> {
    let response = client
        .post(TOKEN_ENDPOINT)
        .form(fields)
        .send()
        .await
        .map_err(|_| Error::Network)?;
    if response.status().is_success() {
        return json(response).await;
    }
    let status = response.status();
    if status.is_server_error() || status.as_u16() == 429 {
        return Err(Error::Network);
    }
    let body: serde_json::Value = json(response).await?;
    match body.get("error").and_then(serde_json::Value::as_str) {
        Some("invalid_grant" | "invalid_token") => Err(Error::LoginRequired),
        _ => Err(Error::InvalidAuthorization),
    }
}

/// A short-lived credential tied to one saved login generation. Debug is redacted.
pub struct AccessGrant {
    pub(crate) token: String,
    client_id: String,
    generation: String,
}
impl fmt::Debug for AccessGrant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ChatGptAccessGrant { .. }")
    }
}
impl AccessGrant {
    pub fn authorization(&self) -> String {
        format!("Bearer {}", self.token)
    }
}

/// Called on each request, so sign-out/account switches and renewal take effect
/// without keeping a stale bearer token in a long-lived inference adapter.
pub async fn access(store: &AccountStore, model: Option<&str>) -> Result<AccessGrant> {
    let _lock = store::lock(store).await?;
    let mut state = State::load(store)?;
    let profile = state.selected().ok_or(Error::LoginRequired)?;
    let credentials = profile.credentials.as_ref().ok_or(Error::LoginRequired)?;
    if model.is_some_and(|model| !profile.models.iter().any(|m| m.id == model)) {
        return Err(Error::ModelUnavailable);
    }
    if credentials.expires_at <= now() + 60 {
        let tokens = exchange(
            &client()?,
            &[
                ("grant_type", "refresh_token"),
                ("client_id", &profile.id),
                ("refresh_token", &credentials.refresh_token),
                ("resource", RESOURCE),
            ],
        )
        .await;
        let tokens = match tokens {
            Ok(tokens) => tokens,
            Err(error @ Error::LoginRequired) => {
                store::expire(&mut state);
                state.save(store)?;
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        // A newly supplied ID token must still describe this registration.
        if let Some(id_token) = &tokens.id_token {
            let identity = login::verify_identity(&client()?, id_token, &profile.id, None).await?;
            if identity.sub != profile.subject {
                return Err(Error::InvalidAuthorization);
            }
        }
        let renewed = tokens.credentials(Some(credentials))?;
        let profile = state.selected_mut().ok_or(Error::LoginRequired)?;
        profile.credentials = Some(renewed);
        profile.generation = uuid::Uuid::new_v4().to_string();
        state.save(store)?;
    }
    let profile = state.selected().ok_or(Error::LoginRequired)?;
    Ok(AccessGrant {
        token: profile
            .credentials
            .as_ref()
            .ok_or(Error::LoginRequired)?
            .access_token
            .clone(),
        client_id: profile.id.clone(),
        generation: profile.generation.clone(),
    })
}

pub async fn invalidate(store: &AccountStore, grant: &AccessGrant) -> Result<()> {
    let _lock = store::lock(store).await?;
    let mut state = State::load(store)?;
    if state
        .selected()
        .is_some_and(|p| p.id == grant.client_id && p.generation == grant.generation)
    {
        store::expire(&mut state);
        state.save(store)?;
    }
    Ok(())
}

async fn discover_models(client: &Client, token: &str) -> Result<Vec<AccountModel>> {
    let response = client
        .get(format!("{RESOURCE}/models"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| Error::Network)?;
    if response.status().as_u16() == 401 {
        return Err(Error::LoginRequired);
    }
    if !response.status().is_success() {
        return Err(Error::NoModels);
    }
    let body: serde_json::Value = json(response).await?;
    let models = body
        .get("models")
        .and_then(serde_json::Value::as_array)
        .ok_or(Error::NoModels)?;
    let mut result = Vec::new();
    for model in models {
        if model["visibility"] != "list" || model["supported_in_api"] == false {
            continue;
        }
        let Some(id) = model["slug"].as_str().filter(|id| {
            !id.is_empty()
                && id.len() < 256
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-._/".contains(&b))
        }) else {
            continue;
        };
        if result.iter().any(|m: &AccountModel| m.id == id) {
            continue;
        }
        result.push(AccountModel {
            id: id.into(),
            label: model["display_name"]
                .as_str()
                .unwrap_or(id)
                .chars()
                .take(160)
                .collect(),
        });
    }
    if result.is_empty() {
        return Err(Error::NoModels);
    }
    Ok(result)
}

pub async fn refresh_models(store: &AccountStore) -> Result<()> {
    let grant = access(store, None).await?;
    let models = match discover_models(&client()?, &grant.token).await {
        Err(Error::LoginRequired) => {
            invalidate(store, &grant).await?;
            return Err(Error::LoginRequired);
        }
        result => result?,
    };
    let _lock = store::lock(store).await?;
    let mut state = State::load(store)?;
    let profile = state.selected_mut().ok_or(Error::LoginRequired)?;
    if profile.id != grant.client_id || profile.generation != grant.generation {
        return Err(Error::Cancelled);
    }
    profile.models = models;
    state.save(store)
}

pub async fn sign_out(store: &AccountStore, profile_id: &str) -> Result<()> {
    let (client_id, credentials) = {
        let _lock = store::lock(store).await?;
        let mut state = State::load(store)?;
        let Some(profile) = state.profile(profile_id) else {
            return Ok(());
        };
        let client_id = profile.id.clone();
        let credentials = state.disconnect(profile_id, false);
        state.save(store)?; // Stop local use even when remote revocation fails.
        (client_id, credentials)
    };
    let Some(credentials) = credentials else {
        return Ok(());
    };
    let client = client().map_err(|_| Error::RemoteRevocationUnconfirmed)?;
    let response = client
        .get(format!("{ISSUER}/.well-known/openid-configuration"))
        .send()
        .await
        .map_err(|_| Error::RemoteRevocationUnconfirmed)?;
    let discovery: serde_json::Value = json(response)
        .await
        .map_err(|_| Error::RemoteRevocationUnconfirmed)?;
    let endpoint = discovery["revocation_endpoint"]
        .as_str()
        .ok_or(Error::RemoteRevocationUnconfirmed)?;
    let url = url::Url::parse(endpoint).map_err(|_| Error::RemoteRevocationUnconfirmed)?;
    if url.origin().ascii_serialization() != ISSUER
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(Error::RemoteRevocationUnconfirmed);
    }
    for attempt in 0..2 {
        let result = client
            .post(url.clone())
            .form(&[
                ("token", credentials.refresh_token.as_str()),
                ("token_type_hint", "refresh_token"),
                ("client_id", client_id.as_str()),
            ])
            .send()
            .await;
        if result.as_ref().is_ok_and(|r| r.status().as_u16() == 200) {
            return Ok(());
        }
        if attempt == 0 {
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }
    Err(Error::RemoteRevocationUnconfirmed)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn token_renewal_does_not_block_other_provider_sign_out() {
        let directory = tempfile::tempdir().unwrap();
        let accounts = AccountStore::for_project_root(directory.path());
        let renewal = store::lock(&accounts).await.unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            sender
                .send(accounts.logout_website_profile(crate::AccountProvider::Doubao, "unused"))
                .unwrap();
        });
        let result = receiver.recv_timeout(Duration::from_secs(2));
        drop(renewal);
        worker.join().unwrap();
        result
            .expect("another provider waited for OAuth renewal")
            .unwrap();
    }

    #[tokio::test]
    async fn stale_requests_cannot_expire_a_new_login_or_another_account() {
        let directory = tempfile::tempdir().unwrap();
        let store = AccountStore::for_project_root(directory.path());
        let mut state = State {
            context: store::Host {
                host_id: format!("urn:uuid:{}", uuid::Uuid::new_v4()),
                ..Default::default()
            },
            active: Some("first".into()),
            ..Default::default()
        };
        for id in ["first", "second"] {
            state.profiles.push(store::Profile {
                id: id.into(),
                subject: id.into(),
                email: "same@example.org".into(),
                display_name: String::new(),
                generation: "original".into(),
                expired: false,
                models: vec![AccountModel {
                    id: "allowed".into(),
                    label: "Allowed model".into(),
                }],
                credentials: Some(Credentials {
                    access_token: "private-test-token".into(),
                    refresh_token: "refresh".into(),
                    id_token: "id".into(),
                    expires_at: now() + 3600,
                    scopes: vec![PLAN_SCOPE.into()],
                }),
            });
        }
        {
            let _lock = store.lock(crate::AccountProvider::Chatgpt).unwrap();
            state.save(&store).unwrap();
        }
        let old = access(&store, Some("allowed")).await.unwrap();
        assert!(!format!("{old:?}").contains("private-test-token"));
        assert_eq!(
            access(&store, Some("unknown")).await.unwrap_err(),
            Error::ModelUnavailable
        );
        {
            let _lock = store.lock(crate::AccountProvider::Chatgpt).unwrap();
            state.profiles[0].generation = "new-login".into();
            state.save(&store).unwrap();
        }
        invalidate(&store, &old).await.unwrap();
        assert_eq!(
            snapshot(&store).unwrap().status,
            crate::AccountStatus::Connected
        );
        select_profile(&store, "second").unwrap();
        invalidate(&store, &old).await.unwrap();
        assert_eq!(
            snapshot(&store).unwrap().active_profile.as_deref(),
            Some("second")
        );
        assert_eq!(
            snapshot(&store).unwrap().status,
            crate::AccountStatus::Connected
        );
        let grant = access(&store, None).await.unwrap();
        invalidate(&store, &grant).await.unwrap();
        let view = snapshot(&store).unwrap();
        assert_eq!(view.status, crate::AccountStatus::Expired);
        assert_eq!(view.profiles.len(), 2); // Reauthorization retains issued IDs and host identity.
        assert!(view.models.unwrap().is_empty());
        assert_eq!(
            State::load(&store).unwrap().context.host_id,
            state.context.host_id
        );
    }
}
