//! Private account sessions shared by hosts and inference connectors.
//! Never place these documents in user-config.json or export them with a release.
#![forbid(unsafe_code)]

mod accounts;
pub mod chatgpt;
mod error;
pub mod login;
mod website;
pub use error::ConnectionError;

use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fmt, fs,
    io::{self, Write},
    path::{Path, PathBuf},
};
/// Login identities shared by any number of service connectors.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AccountProvider {
    Doubao,
    Chatgpt,
}

impl AccountProvider {
    pub const ALL: &[Self] = &[Self::Doubao, Self::Chatgpt];

    pub const fn id(self) -> &'static str {
        match self {
            Self::Doubao => "doubao",
            Self::Chatgpt => "chatgpt",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Doubao => "Doubao",
            Self::Chatgpt => "ChatGPT",
        }
    }

    pub const fn browser_oauth(self) -> bool {
        matches!(self, Self::Chatgpt)
    }

    pub const fn sign_in_label(self) -> &'static str {
        match self {
            Self::Chatgpt => "Continue with ChatGPT",
            _ => "Sign in",
        }
    }

    pub const fn requirement(self) -> Option<&'static str> {
        match self {
            Self::Chatgpt => Some("Requires an eligible ChatGPT Plus or Pro plan."),
            _ => None,
        }
    }
}

pub enum AccountOperation {
    SignIn { profile: Option<String> },
    SignOut { profile: String },
    RefreshModels,
    SelectProfile(String),
}

/// Credential-free presentation data. Hosts cache this; painting never does network I/O.
#[derive(Clone, Debug, Default)]
pub struct AccountSnapshot {
    pub status: AccountStatus,
    pub label: Option<String>,
    pub profiles: Vec<AccountProfile>,
    pub active_profile: Option<String>,
    pub models: Option<Vec<AccountModel>>,
}

#[derive(Clone, Debug)]
pub struct AccountProfile {
    pub id: String,
    pub label: String,
    pub connected: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountModel {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AccountStatus {
    #[default]
    SignedOut,
    Connected,
    Expired,
    Unavailable,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountSession {
    schema_version: u32,
    provider: AccountProvider,
    /// Prevents an old failed request from invalidating a newer login.
    generation: String,
    cookie_header: String,
    attributes: BTreeMap<String, String>,
}

impl fmt::Debug for AccountSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccountSession")
            .field("provider", &self.provider)
            .finish_non_exhaustive()
    }
}

impl AccountSession {
    pub fn new(
        provider: AccountProvider,
        cookie_header: String,
        attributes: BTreeMap<String, String>,
    ) -> io::Result<Self> {
        let session = Self {
            schema_version: 1,
            provider,
            generation: uuid::Uuid::new_v4().to_string(),
            cookie_header,
            attributes,
        };
        session.validate()?;
        Ok(session)
    }

    pub fn cookie_header(&self) -> &str {
        &self.cookie_header
    }
    pub fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes.get(name).map(String::as_str)
    }

    fn validate(&self) -> io::Result<()> {
        let valid = self.schema_version == 1
            && uuid::Uuid::parse_str(&self.generation).is_ok()
            && !self.cookie_header.is_empty()
            && self.cookie_header.len() <= 64 * 1024
            && self
                .cookie_header
                .bytes()
                .all(|byte| byte >= 0x20 && byte < 0x7f)
            && login::spec(self.provider).is_some_and(|spec| {
                spec.required_attributes.iter().all(|key| {
                    self.attribute(key).is_some_and(|value| {
                        !value.is_empty()
                            && value.len() <= 128
                            && value.bytes().all(|b| b.is_ascii_digit())
                    })
                })
            });
        if valid {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid account session; sign in again.",
            ))
        }
    }
}

#[derive(Clone, Debug)]
pub struct AccountStore {
    directory: PathBuf,
}

impl AccountStore {
    pub async fn operate(
        &self,
        provider: AccountProvider,
        action: AccountOperation,
        cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
        open_browser: impl FnOnce(String) -> Result<(), ConnectionError>,
    ) -> Result<(), ConnectionError> {
        match action {
            AccountOperation::SignIn { profile } => match provider {
                AccountProvider::Chatgpt => {
                    chatgpt::sign_in(self, profile.as_deref(), cancelled, open_browser).await
                }
                _ => Err(ConnectionError::InvalidAuthorization), // Website login is hosted by the native WebView.
            },
            AccountOperation::SignOut { profile } => match provider {
                AccountProvider::Chatgpt => chatgpt::sign_out(self, &profile).await,
                _ => self
                    .logout_website_profile(provider, &profile)
                    .map_err(|_| ConnectionError::Storage),
            },
            AccountOperation::RefreshModels => match provider {
                AccountProvider::Chatgpt => chatgpt::refresh_models(self).await,
                _ => Ok(()),
            },
            AccountOperation::SelectProfile(id) => match provider {
                AccountProvider::Chatgpt => chatgpt::select_profile(self, &id),
                _ => self
                    .select_website_profile(provider, &id)
                    .map_err(|_| ConnectionError::Storage),
            },
        }
    }
    pub fn for_project_root(root: impl AsRef<Path>) -> Self {
        Self {
            directory: root.as_ref().join("runtime/accounts"),
        }
    }
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    fn path(&self, provider: AccountProvider) -> PathBuf {
        self.directory
            .join(format!("{}.accounts.json", provider.id()))
    }

    pub fn status(&self, provider: AccountProvider) -> AccountStatus {
        self.snapshot(provider).status
    }

    pub fn snapshot(&self, provider: AccountProvider) -> AccountSnapshot {
        let snapshot = match provider {
            AccountProvider::Chatgpt => chatgpt::snapshot(self).map_err(|_| ()),
            _ => self.website_snapshot(provider).map_err(|_| ()),
        };
        snapshot.unwrap_or(AccountSnapshot {
            status: AccountStatus::Unavailable,
            ..Default::default()
        })
    }

    /// Atomically write a private document while the caller holds the store lock.
    fn write_private(&self, provider: AccountProvider, value: &impl Serialize) -> io::Result<()> {
        let mut file = tempfile::NamedTempFile::new_in(&self.directory)?;
        serde_json::to_writer(&mut file, value)
            .map_err(|_| io::Error::other("Cannot save account connection."))?;
        file.flush()?;
        file.as_file().sync_all()?;
        file.persist(self.path(provider))
            .map_err(|error| error.error)?;
        Ok(())
    }

    fn prepare_directory(&self) -> io::Result<()> {
        fs::create_dir_all(&self.directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.directory, fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }

    fn lock(&self, provider: AccountProvider) -> io::Result<fs::File> {
        self.prepare_directory()?;
        let mut options = fs::OpenOptions::new();
        options.create(true).read(true).write(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(self.directory.join(format!(".{}.lock", provider.id())))?;
        file.lock()?;
        Ok(file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn session() -> AccountSession {
        AccountSession::new(
            AccountProvider::Doubao,
            "sessionid=private".into(),
            [
                ("device_id".into(), "123".into()),
                ("web_id".into(), "456".into()),
            ]
            .into(),
        )
        .unwrap()
    }
    #[test]
    fn reject_header_injection() {
        let mut session = session();
        session.cookie_header.push_str("\r\nInjected: secret");
        assert!(session.validate().is_err());
    }
}
