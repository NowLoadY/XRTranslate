use super::*;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use sha2::{Digest, Sha256};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

#[derive(Clone, Deserialize)]
pub(super) struct Identity {
    pub sub: String,
    #[serde(default)]
    email: String,
    #[serde(default)]
    phone_number: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    nonce: Option<String>,
}

pub(super) async fn verify_identity(
    client: &Client,
    token: &str,
    client_id: &str,
    nonce: Option<&str>,
) -> Result<Identity> {
    let response = client
        .get(format!("{ISSUER}/.well-known/jwks.json"))
        .send()
        .await
        .map_err(|_| Error::Network)?;
    if !response.status().is_success() {
        return Err(Error::Network);
    }
    let keys: JwkSet = json(response).await?;
    validate_identity(token, client_id, nonce, &keys)
}

fn validate_identity(
    token: &str,
    client_id: &str,
    nonce: Option<&str>,
    keys: &JwkSet,
) -> Result<Identity> {
    let header = decode_header(token).map_err(|_| Error::InvalidAuthorization)?;
    if header.alg != Algorithm::RS256 {
        return Err(Error::InvalidAuthorization);
    }
    let key = keys
        .find(header.kid.as_deref().ok_or(Error::InvalidAuthorization)?)
        .ok_or(Error::InvalidAuthorization)?;
    let key = DecodingKey::from_jwk(key).map_err(|_| Error::InvalidAuthorization)?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_audience(&[client_id]);
    validation.set_issuer(&[ISSUER]);
    validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
    validation.validate_nbf = true;
    let identity = decode::<Identity>(token, &key, &validation)
        .map_err(|_| Error::InvalidAuthorization)?
        .claims;
    if identity.sub.is_empty() || nonce.is_some_and(|n| identity.nonce.as_deref() != Some(n)) {
        return Err(Error::InvalidAuthorization);
    }
    Ok(identity)
}

struct Attempt {
    state: String,
    nonce: String,
    verifier: String,
    redirect: String,
    client_id: String,
}
impl Attempt {
    fn url(&self, host: &str, returning: Option<&store::Profile>) -> String {
        let mut url = url::Url::parse(&format!("{ISSUER}/api/accounts/authorize")).unwrap();
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(self.verifier.as_bytes()));
        {
            let mut query = url.query_pairs_mut();
            query.extend_pairs([
                ("client_id", self.client_id.as_str()),
                ("ext_agent_host_id", host),
                ("response_type", "code"),
                ("redirect_uri", &self.redirect),
                (
                    "scope",
                    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct",
                ),
                ("resource", RESOURCE),
                ("state", &self.state),
                ("nonce", &self.nonce),
                ("code_challenge_method", "S256"),
                ("code_challenge", &challenge),
            ]);
            if self.client_id == REGISTRATION_CLIENT {
                query.append_pair("agent_name_hint", "XRTranslate");
            }
            if let Some(profile) = returning {
                if !profile.email.is_empty() {
                    query.append_pair("login_hint", &profile.email);
                }
                if let Some(credentials) = &profile.credentials {
                    query.append_pair("id_token_hint", &credentials.id_token);
                }
            }
        }
        url.into()
    }

    fn callback(&self, target: &str) -> Result<(String, String)> {
        let url = url::Url::parse(&format!("http://127.0.0.1{target}"))
            .map_err(|_| Error::InvalidAuthorization)?;
        if url.path() != "/auth/callback" {
            return Err(Error::InvalidAuthorization);
        }
        let pairs: Vec<_> = url.query_pairs().collect();
        if ["state", "code", "client_id", "error"]
            .iter()
            .any(|name| pairs.iter().filter(|(key, _)| key == name).count() > 1)
        {
            return Err(Error::InvalidAuthorization);
        }
        let field = |name: &str| {
            let values: Vec<_> = pairs
                .iter()
                .filter(|(key, _)| key == name)
                .map(|(_, value)| value.as_ref())
                .collect();
            (values.len() == 1).then(|| values[0])
        };
        if field("state") != Some(self.state.as_str()) {
            return Err(Error::InvalidAuthorization);
        }
        if field("error").is_some() {
            return Err(Error::Cancelled);
        }
        let code = field("code")
            .filter(|code| !code.is_empty())
            .ok_or(Error::InvalidAuthorization)?;
        let issued = if self.client_id == REGISTRATION_CLIENT {
            field("client_id")
                .filter(|id| !id.is_empty() && *id != REGISTRATION_CLIENT)
                .ok_or(Error::InvalidAuthorization)?
        } else {
            if field("client_id").is_some_and(|id| id != self.client_id) {
                return Err(Error::InvalidAuthorization);
            }
            &self.client_id
        };
        Ok((code.to_owned(), issued.to_owned()))
    }
}

/// Hosts open the supplied URL with the system browser. No browser profile or
/// credential is imported. Cancellation drops the listener and pending I/O.
pub async fn sign_in(
    store: &AccountStore,
    profile_id: Option<&str>,
    cancelled: Arc<AtomicBool>,
    open_browser: impl FnOnce(String) -> Result<()>,
) -> Result<()> {
    let (host, returning, pending) = {
        let _lock = store::lock(store).await?;
        let mut state = State::load(store)?;
        if state.context.host_id.is_empty() {
            state.context.host_id = format!("urn:uuid:{}", uuid::Uuid::new_v4());
        }
        state.save(store)?;
        (
            state.context.host_id.clone(),
            profile_id
                .map(|id| state.profile(id).cloned().ok_or(Error::LoginRequired))
                .transpose()?,
            state.context.pending_client.clone(),
        )
    };
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|_| Error::Network)?;
    let attempt = Attempt {
        state: uuid::Uuid::new_v4().simple().to_string(),
        nonce: uuid::Uuid::new_v4().simple().to_string(),
        verifier: format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        ),
        redirect: format!(
            "http://127.0.0.1:{}/auth/callback",
            listener.local_addr().map_err(|_| Error::Network)?.port()
        ),
        client_id: returning
            .as_ref()
            .map(|p| p.id.clone())
            .or(pending)
            .unwrap_or_else(|| REGISTRATION_CLIENT.into()),
    };
    if cancelled.load(Ordering::Acquire) {
        return Err(Error::Cancelled);
    }
    open_browser(attempt.url(&host, returning.as_ref()))?;
    let operation = async {
        loop {
            let (mut socket, peer) = listener.accept().await.map_err(|_| Error::Network)?;
            if !peer.ip().is_loopback() {
                continue;
            }
            let request =
                tokio::time::timeout(Duration::from_secs(3), read_target(&mut socket)).await;
            let Ok(Ok(target)) = request else {
                continue;
            };
            let (code, issued) = match attempt.callback(&target) {
                Ok(value) => value,
                Err(Error::Cancelled) => {
                    reply(&mut socket, false).await;
                    return Err(Error::Cancelled);
                }
                Err(_) => {
                    reply(&mut socket, false).await;
                    continue;
                }
            };
            let result = complete(
                store,
                &attempt,
                &issued,
                &code,
                returning.as_ref(),
                &cancelled,
            )
            .await;
            reply(&mut socket, result.is_ok()).await;
            return result;
        }
    };
    tokio::select! {
        result = operation => result,
        _ = async {
            loop {
                if cancelled.load(Ordering::Acquire) { break; }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        } => Err(Error::Cancelled),
        _ = tokio::time::sleep(Duration::from_secs(15 * 60)) => Err(Error::Cancelled),
    }
}

async fn complete(
    store: &AccountStore,
    attempt: &Attempt,
    issued: &str,
    code: &str,
    returning: Option<&store::Profile>,
    cancelled: &AtomicBool,
) -> Result<()> {
    if returning.is_none() {
        let _lock = store::lock(store).await?;
        let mut state = State::load(store)?;
        state.context.pending_client = Some(issued.into());
        state.save(store)?;
    }
    let client = client()?;
    let tokens = exchange(
        &client,
        &[
            ("grant_type", "authorization_code"),
            ("client_id", issued),
            ("code", code),
            ("code_verifier", &attempt.verifier),
            ("redirect_uri", &attempt.redirect),
            ("resource", RESOURCE),
        ],
    )
    .await?;
    let id_token = tokens
        .id_token
        .as_deref()
        .ok_or(Error::InvalidAuthorization)?;
    let identity = verify_identity(&client, id_token, issued, Some(&attempt.nonce)).await?;
    if returning.is_some_and(|p| p.subject != identity.sub) {
        return Err(Error::InvalidAuthorization);
    }
    let credentials = tokens.credentials(None)?;
    let models = discover_models(&client, &credentials.access_token).await?;
    let _lock = store::lock(store).await?;
    if cancelled.load(Ordering::Acquire) {
        return Err(Error::Cancelled);
    }
    let mut state = State::load(store)?;
    if let Some(original) = returning {
        if !state
            .profiles
            .iter()
            .any(|p| p.id == original.id && p.generation == original.generation)
        {
            return Err(Error::Cancelled);
        }
    }
    if state
        .profiles
        .iter()
        .any(|p| p.id == issued && p.subject != identity.sub)
    {
        return Err(Error::InvalidAuthorization);
    }
    let profile = store::Profile {
        id: issued.into(),
        subject: identity.sub,
        email: identity.email,
        display_name: if !identity.phone_number.trim().is_empty() {
            identity.phone_number
        } else {
            identity.name
        }
        .chars()
        .filter(|c| !c.is_control())
        .take(160)
        .collect(),
        generation: uuid::Uuid::new_v4().to_string(),
        credentials: Some(credentials),
        expired: false,
        models,
    };
    state.connect(profile);
    state.context.pending_client = None;
    state.save(store)
}

async fn read_target(socket: &mut TcpStream) -> Result<String> {
    let mut bytes = Vec::new();
    let mut chunk = [0; 1024];
    while !bytes.windows(4).any(|s| s == b"\r\n\r\n") {
        let count = socket.read(&mut chunk).await.map_err(|_| Error::Network)?;
        if count == 0 || bytes.len() + count > 16384 {
            return Err(Error::InvalidAuthorization);
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    let request = std::str::from_utf8(&bytes).map_err(|_| Error::InvalidAuthorization)?;
    let mut parts = request
        .lines()
        .next()
        .ok_or(Error::InvalidAuthorization)?
        .split_whitespace();
    if parts.next() != Some("GET") {
        return Err(Error::InvalidAuthorization);
    }
    let target = parts
        .next()
        .filter(|s| s.starts_with("/auth/callback?"))
        .ok_or(Error::InvalidAuthorization)?;
    Ok(target.into())
}

async fn reply(socket: &mut TcpStream, success: bool) {
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let body = include_str!("callback.html")
        .replace("{{success}}", if success { "true" } else { "false" })
        .replace(
            "{{title}}",
            if success {
                "You're connected"
            } else {
                "Sign-in wasn't completed"
            },
        )
        .replace(
            "{{detail}}",
            if success {
                "Return to XRTranslate. You can close this tab."
            } else {
                "Return to XRTranslate to try again."
            },
        )
        .replace("{{nonce}}", &nonce);
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'; style-src 'nonce-{nonce}'; script-src 'nonce-{nonce}'; base-uri 'none'; form-action 'none'\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = socket.write_all(response.as_bytes()).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn id_token_requires_signature_issuer_audience_expiry_and_nonce() {
        use rsa::{
            RsaPrivateKey, RsaPublicKey, pkcs1::EncodeRsaPrivateKey, traits::PublicKeyParts,
        };
        let private = RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
        let public = RsaPublicKey::from(&private);
        let der = private.to_pkcs1_der().unwrap();
        let encoding = jsonwebtoken::EncodingKey::from_rsa_der(der.as_bytes());
        let keys: JwkSet = serde_json::from_value(serde_json::json!({"keys":[{
            "kty":"RSA", "alg":"RS256", "use":"sig", "kid":"fixture",
            "n":URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()),
            "e":URL_SAFE_NO_PAD.encode(public.e().to_bytes_be())
        }]}))
        .unwrap();
        let mut header = jsonwebtoken::Header::new(Algorithm::RS256);
        header.kid = Some("fixture".into());
        let claims = serde_json::json!({"sub":"user", "iss":ISSUER, "aud":"issued-client", "exp":now()+300, "nonce":"attempt"});
        let token = jsonwebtoken::encode(&header, &claims, &encoding).unwrap();
        assert_eq!(
            validate_identity(&token, "issued-client", Some("attempt"), &keys)
                .unwrap()
                .sub,
            "user"
        );
        assert!(validate_identity(&token, "other-client", Some("attempt"), &keys).is_err());
        assert!(validate_identity(&token, "issued-client", Some("other-attempt"), &keys).is_err());
        for (field, value) in [
            ("iss", serde_json::json!("https://example.org")),
            ("exp", serde_json::json!(now() - 3600)),
        ] {
            let mut wrong = claims.clone();
            wrong[field] = value;
            let token = jsonwebtoken::encode(&header, &wrong, &encoding).unwrap();
            assert!(validate_identity(&token, "issued-client", Some("attempt"), &keys).is_err());
        }
        let mut parts: Vec<_> = token.split('.').map(str::to_owned).collect();
        parts[1] = URL_SAFE_NO_PAD.encode(br#"{"sub":"forged","iss":"https://auth.openai.com","aud":"issued-client","exp":9999999999,"nonce":"attempt"}"#);
        assert!(
            validate_identity(&parts.join("."), "issued-client", Some("attempt"), &keys).is_err()
        );
    }

    #[test]
    fn callback_is_bound_to_attempt_and_registered_account() {
        let attempt = Attempt {
            state: "expected".into(),
            nonce: "nonce".into(),
            verifier: "verifier".into(),
            redirect: "http://127.0.0.1:1234/auth/callback".into(),
            client_id: "registered-account".into(),
        };
        assert!(
            attempt
                .callback("/auth/callback?state=wrong&code=secret")
                .is_err()
        );
        assert!(
            attempt
                .callback("/auth/callback?state=expected&code=secret&client_id=other")
                .is_err()
        );
        assert!(
            attempt
                .callback("/auth/callback?state=expected&state=expected&code=secret")
                .is_err()
        );
        assert!(attempt.callback("/auth/callback?state=expected&code=secret&client_id=registered-account&client_id=other").is_err());
        assert_eq!(
            attempt.callback("/auth/callback?state=expected&error=access_denied"),
            Err(Error::Cancelled)
        );
        assert_eq!(
            attempt
                .callback("/auth/callback?state=expected&code=secret")
                .unwrap()
                .1,
            "registered-account"
        );
        let url = url::Url::parse(&attempt.url("urn:uuid:test", None)).unwrap();
        assert_eq!(url.host_str(), Some("auth.openai.com"));
        assert!(!url.query_pairs().any(|(k, _)| k == "agent_name_hint"));
    }
}
