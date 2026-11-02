use super::*;
use crate::{
    AccountProvider, AccountSnapshot,
    accounts::{Accounts, SavedAccount},
};

pub(super) type State = Accounts<Credentials, Host>;
pub(super) type Profile = SavedAccount<Credentials>;

#[derive(Default, Serialize, Deserialize)]
pub(super) struct Host {
    pub host_id: String,
    #[serde(default)]
    pub pending_client: Option<String>,
}

// Deliberately no Debug: only AccountSnapshot can cross the presentation boundary.
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Credentials {
    pub access_token: String,
    pub refresh_token: String,
    pub id_token: String,
    pub expires_at: u64,
    pub scopes: Vec<String>,
}

impl State {
    pub fn load(store: &AccountStore) -> Result<Self> {
        let state = Self::read(store, AccountProvider::Chatgpt).map_err(|_| Error::Storage)?;
        if (!state.context.host_id.is_empty()
            && (!state.context.host_id.starts_with("urn:uuid:")
                || uuid::Uuid::parse_str(&state.context.host_id[9..]).is_err()))
            || state
                .profiles
                .iter()
                .any(|p| p.id.is_empty() || p.id == REGISTRATION_CLIENT || p.subject.is_empty())
        {
            return Err(Error::Storage);
        }
        Ok(state)
    }
    pub fn save(&self, store: &AccountStore) -> Result<()> {
        self.write(store, AccountProvider::Chatgpt)
            .map_err(|_| Error::Storage)
    }
}

pub fn snapshot(store: &AccountStore) -> Result<AccountSnapshot> {
    Ok(State::load(store)?.snapshot(AccountProvider::Chatgpt, true))
}

pub fn select_profile(store: &AccountStore, id: &str) -> Result<()> {
    let _lock = store
        .lock(AccountProvider::Chatgpt)
        .map_err(|_| Error::Storage)?;
    let mut state = State::load(store)?;
    state.select(id).map_err(|_| Error::Storage)?;
    state.save(store)
}

pub(super) async fn lock(store: &AccountStore) -> Result<std::fs::File> {
    let store = store.clone();
    tokio::task::spawn_blocking(move || store.lock(AccountProvider::Chatgpt))
        .await
        .map_err(|_| Error::Storage)?
        .map_err(|_| Error::Storage)
}

pub(super) fn expire(state: &mut State) {
    if let Some(id) = state.active.clone() {
        state.disconnect(&id, true);
    }
}
