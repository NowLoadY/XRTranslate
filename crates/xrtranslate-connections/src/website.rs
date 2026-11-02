//! Website authorization contributes credentials to the shared account inventory.
use crate::{
    AccountProvider, AccountSession, AccountSnapshot, AccountStore,
    accounts::{Accounts, SavedAccount},
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io};

#[derive(Clone, Serialize, Deserialize)]
struct Credentials {
    cookie_header: String,
    attributes: BTreeMap<String, String>,
}
type WebsiteAccounts = Accounts<Credentials>;

impl AccountStore {
    fn website_accounts(&self, provider: AccountProvider) -> io::Result<WebsiteAccounts> {
        if provider.browser_oauth() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "This account uses browser authorization.",
            ));
        }
        let accounts = WebsiteAccounts::read(self, provider)?;
        for account in &accounts.profiles {
            if let Some(credentials) = &account.credentials {
                session(provider, account, credentials).validate()?;
            }
        }
        Ok(accounts)
    }

    pub(crate) fn website_snapshot(
        &self,
        provider: AccountProvider,
    ) -> io::Result<AccountSnapshot> {
        let accounts = self.website_accounts(provider)?;
        Ok(accounts.snapshot(provider, false))
    }

    pub fn load(&self, provider: AccountProvider) -> io::Result<Option<AccountSession>> {
        let accounts = self.website_accounts(provider)?;
        Ok(accounts.selected().and_then(|account| {
            account
                .credentials
                .as_ref()
                .map(|credentials| session(provider, account, credentials))
        }))
    }

    pub fn save(&self, session: &AccountSession) -> io::Result<()> {
        session.validate()?;
        let _lock = self.lock(session.provider)?;
        let mut accounts = self.website_accounts(session.provider)?;
        let subject = session.attribute("account_id").unwrap_or_default();
        let id = accounts
            .profiles
            .iter()
            .find(|p| {
                (!subject.is_empty() && p.subject == subject)
                    || p.credentials
                        .as_ref()
                        .is_some_and(|c| c.cookie_header == session.cookie_header)
            })
            .map(|p| p.id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        accounts.connect(profile(session, id));
        accounts.write(self, session.provider)
    }

    pub(crate) fn select_website_profile(
        &self,
        provider: AccountProvider,
        id: &str,
    ) -> io::Result<()> {
        let _lock = self.lock(provider)?;
        let mut accounts = self.website_accounts(provider)?;
        accounts.select(id)?;
        accounts.write(self, provider)
    }

    pub(crate) fn logout_website_profile(
        &self,
        provider: AccountProvider,
        id: &str,
    ) -> io::Result<()> {
        let _lock = self.lock(provider)?;
        let mut accounts = self.website_accounts(provider)?;
        accounts.disconnect(id, false);
        accounts.write(self, provider)
    }

    pub fn expire(&self, session: &AccountSession) -> io::Result<()> {
        let _lock = self.lock(session.provider)?;
        let mut accounts = self.website_accounts(session.provider)?;
        if let Some(id) = accounts
            .profiles
            .iter()
            .find(|p| p.generation == session.generation)
            .map(|p| p.id.clone())
        {
            accounts.expire(&id, &session.generation);
            accounts.write(self, session.provider)?;
        }
        Ok(())
    }
}

fn session(
    provider: AccountProvider,
    account: &SavedAccount<Credentials>,
    credentials: &Credentials,
) -> AccountSession {
    AccountSession {
        schema_version: 1,
        provider,
        generation: account.generation.clone(),
        cookie_header: credentials.cookie_header.clone(),
        attributes: credentials.attributes.clone(),
    }
}

fn profile(session: &AccountSession, id: String) -> SavedAccount<Credentials> {
    SavedAccount {
        id,
        subject: session.attribute("account_id").unwrap_or_default().into(),
        email: String::new(),
        display_name: session.attribute("display_name").unwrap_or_default().into(),
        generation: session.generation.clone(),
        expired: false,
        models: Vec::new(),
        credentials: Some(Credentials {
            cookie_header: session.cookie_header.clone(),
            attributes: session.attributes.clone(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switching_accounts_keeps_credentials_and_expiry_separate() {
        let directory = tempfile::tempdir().unwrap();
        let store = AccountStore::for_project_root(directory.path());
        let provider = AccountProvider::Doubao;
        let login = |subject: &str, cookie: &str| {
            AccountSession::new(
                provider,
                cookie.into(),
                [
                    ("device_id".into(), "123".into()),
                    ("web_id".into(), "456".into()),
                    ("account_id".into(), subject.into()),
                ]
                .into(),
            )
            .unwrap()
        };
        let first = login("first", "sessionid=first");
        store.save(&first).unwrap();
        let first_id = store.snapshot(provider).active_profile.unwrap();
        let second = login("second", "sessionid=second");
        store.save(&second).unwrap();
        let second_id = store.snapshot(provider).active_profile.unwrap();

        store.select_website_profile(provider, &first_id).unwrap();
        assert_eq!(
            store.load(provider).unwrap().unwrap().cookie_header(),
            first.cookie_header()
        );
        let renewed = login("first", "sessionid=renewed");
        store.save(&renewed).unwrap();
        store.expire(&first).unwrap();
        let snapshot = store.snapshot(provider);
        assert_eq!(snapshot.profiles.len(), 2);
        assert_eq!(snapshot.active_profile.as_deref(), Some(first_id.as_str()));
        assert_eq!(
            store.load(provider).unwrap().unwrap().cookie_header(),
            renewed.cookie_header()
        );

        store.logout_website_profile(provider, &second_id).unwrap();
        assert_eq!(
            store.load(provider).unwrap().unwrap().cookie_header(),
            renewed.cookie_header()
        );
        store.select_website_profile(provider, &second_id).unwrap();
        assert!(store.load(provider).unwrap().is_none());
        store.select_website_profile(provider, &first_id).unwrap();
        store.expire(&renewed).unwrap();
        assert_eq!(store.status(provider), crate::AccountStatus::Expired);
    }
}
