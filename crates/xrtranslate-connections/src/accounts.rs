//! Provider-independent account inventory and lifecycle. Credentials remain typed
//! by the authorization adapter and never cross into presentation snapshots.
use crate::{
    AccountModel, AccountProfile, AccountProvider, AccountSnapshot, AccountStatus, AccountStore,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::io;

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct NoContext {}

#[derive(Serialize, Deserialize)]
pub(crate) struct Accounts<C, H = NoContext> {
    pub active: Option<String>,
    pub profiles: Vec<SavedAccount<C>>,
    #[serde(flatten)]
    pub context: H,
}

impl<C, H: Default> Default for Accounts<C, H> {
    fn default() -> Self {
        Self {
            active: None,
            profiles: Vec::new(),
            context: H::default(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct SavedAccount<C> {
    pub id: String,
    pub subject: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub display_name: String,
    pub generation: String,
    pub credentials: Option<C>,
    pub expired: bool,
    pub models: Vec<AccountModel>,
}

impl<C, H> Accounts<C, H> {
    pub fn profile(&self, id: &str) -> Option<&SavedAccount<C>> {
        self.profiles.iter().find(|p| p.id == id)
    }
    pub fn profile_mut(&mut self, id: &str) -> Option<&mut SavedAccount<C>> {
        self.profiles.iter_mut().find(|p| p.id == id)
    }
    pub fn selected(&self) -> Option<&SavedAccount<C>> {
        self.active.as_deref().and_then(|id| self.profile(id))
    }
    pub fn selected_mut(&mut self) -> Option<&mut SavedAccount<C>> {
        let id = self.active.clone()?;
        self.profile_mut(&id)
    }
    pub fn select(&mut self, id: &str) -> io::Result<()> {
        if self.profile(id).is_none() {
            return Err(invalid());
        }
        self.active = Some(id.into());
        Ok(())
    }
    pub fn connect(&mut self, profile: SavedAccount<C>) {
        self.active = Some(profile.id.clone());
        if let Some(saved) = self.profile_mut(&profile.id) {
            *saved = profile;
        } else {
            self.profiles.push(profile);
        }
    }
    /// Disconnect one account without changing the selected account or other sessions.
    pub fn disconnect(&mut self, id: &str, expired: bool) -> Option<C> {
        let profile = self.profile_mut(id)?;
        profile.expired = expired;
        profile.models.clear();
        profile.generation = uuid::Uuid::new_v4().to_string();
        profile.credentials.take()
    }
    pub fn expire(&mut self, id: &str, generation: &str) {
        if self.profile(id).is_some_and(|p| p.generation == generation) {
            self.disconnect(id, true);
        }
    }
    pub fn snapshot(&self, provider: AccountProvider, has_models: bool) -> AccountSnapshot {
        let label = |p: &SavedAccount<C>| {
            let name = if !p.email.is_empty() {
                &p.email
            } else if !p.display_name.is_empty() {
                &p.display_name
            } else {
                provider.label()
            };
            if self
                .profiles
                .iter()
                .filter(|other| other.email == p.email && other.display_name == p.display_name)
                .count()
                <= 1
            {
                return name.to_owned();
            }
            let suffix: String =
                p.id.chars()
                    .rev()
                    .take(6)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect();
            format!("{name} · {suffix}")
        };
        let selected = self.selected();
        AccountSnapshot {
            status: selected.map_or(AccountStatus::SignedOut, |p| {
                if p.expired {
                    AccountStatus::Expired
                } else if p.credentials.is_some() {
                    AccountStatus::Connected
                } else {
                    AccountStatus::SignedOut
                }
            }),
            label: selected.map(label),
            active_profile: self.active.clone(),
            profiles: self
                .profiles
                .iter()
                .map(|p| AccountProfile {
                    id: p.id.clone(),
                    label: label(p),
                    connected: p.credentials.is_some() && !p.expired,
                })
                .collect(),
            models: has_models.then(|| {
                selected
                    .filter(|p| p.credentials.is_some())
                    .map(|p| p.models.clone())
                    .unwrap_or_default()
            }),
        }
    }
}

impl<C: Serialize + DeserializeOwned, H: Serialize + DeserializeOwned + Default> Accounts<C, H> {
    pub fn read(store: &AccountStore, provider: AccountProvider) -> io::Result<Self> {
        let bytes = match std::fs::read(store.path(provider)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(error),
        };
        if bytes.len() > 4 * 1024 * 1024 {
            return Err(invalid());
        }
        serde_json::from_slice(&bytes).map_err(|_| invalid())
    }
    /// Caller holds this provider's cross-process lock.
    pub fn write(&self, store: &AccountStore, provider: AccountProvider) -> io::Result<()> {
        store.write_private(provider, self)
    }
}

pub(crate) fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "Cannot read account connections.",
    )
}
