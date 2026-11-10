//! Generic host controller for official-account connections. No provider wire
//! details or passwords cross the UI boundary.
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use xrtranslate_connections::{
    AccountOperation, AccountProvider, AccountSnapshot, AccountStatus, AccountStore,
};

#[derive(Clone)]
pub(crate) enum AccountAction {
    Connect(AccountProvider, Option<String>),
    SignOut(AccountProvider, String),
    SelectProfile(AccountProvider, String),
    RefreshModels(AccountProvider),
    Cancel,
}

impl AccountAction {
    pub fn changes_account(&self, connections: &AccountConnections) -> bool {
        match self {
            Self::SignOut(provider, id) => connections
                .snapshot(*provider)
                .is_some_and(|s| s.active_profile.as_ref() == Some(id)),
            Self::SelectProfile(provider, id) => connections.snapshot(*provider).is_some_and(|s| {
                s.active_profile.as_ref() != Some(id)
                    && s.profiles.iter().any(|p| p.id == *id && p.connected)
            }),
            _ => false,
        }
    }
}

enum ConnectionEvent {
    Open(String),
    Finished(Result<(), xrtranslate_connections::ConnectionError>),
}
struct ConnectionWorker {
    events: mpsc::Receiver<ConnectionEvent>,
    cancelled: Arc<AtomicBool>,
    login: bool,
}

#[derive(Default)]
pub(crate) struct AccountConnections {
    snapshots: BTreeMap<AccountProvider, AccountSnapshot>,
    checked: Option<Instant>,
    busy_provider: Option<AccountProvider>,
    worker: Option<ConnectionWorker>,
    working_label: &'static str,
    #[cfg(not(target_os = "android"))]
    child: Option<std::process::Child>,
    pub error: Option<String>,
}

impl AccountConnections {
    pub fn status(&self, provider: AccountProvider) -> AccountStatus {
        self.snapshots
            .get(&provider)
            .map(|s| s.status)
            .unwrap_or_default()
    }
    pub fn snapshot(&self, provider: AccountProvider) -> Option<&AccountSnapshot> {
        self.snapshots.get(&provider)
    }
    pub fn working_label(&self) -> &'static str {
        if self.worker.is_some() {
            self.working_label
        } else {
            "Complete sign-in in the login window. You’ll return here automatically."
        }
    }
    pub fn can_cancel(&self) -> bool {
        self.worker.as_ref().is_none_or(|worker| worker.login)
    }
    pub fn is_busy(&self) -> bool {
        self.busy_provider.is_some()
    }

    pub fn is_busy_for(&self, provider: AccountProvider) -> bool {
        self.busy_provider == Some(provider)
    }

    /// Report a completed login once, after its saved connection is readable.
    /// Background status refreshes and cancellation never report login success.
    pub fn poll(&mut self, root: &Path, ctx: &eframe::egui::Context) -> Option<AccountProvider> {
        ctx.request_repaint_after(if self.is_busy() {
            Duration::from_millis(250)
        } else {
            Duration::from_secs(1)
        });
        let mut completed = None;
        if let Some(worker) = self.worker.take() {
            let mut finished = false;
            loop {
                match worker.events.try_recv() {
                    Ok(ConnectionEvent::Open(url)) => {
                        #[cfg(not(target_os = "android"))]
                        ctx.open_url(eframe::egui::OpenUrl::new_tab(url));
                        #[cfg(target_os = "android")]
                        if let Err(error) = crate::android::open_account_authorization(&url) {
                            worker.cancelled.store(true, Ordering::Release);
                            self.error = Some(error);
                        }
                    }
                    Ok(ConnectionEvent::Finished(result)) => {
                        finished = true;
                        match result {
                            Ok(()) if worker.login => completed = self.busy_provider,
                            Err(xrtranslate_connections::ConnectionError::Cancelled) | Ok(()) => {}
                            Err(error) => self.error = Some(error.to_string()),
                        }
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.error =
                            Some("Could not complete account connection. Please try again.".into());
                        finished = true;
                        break;
                    }
                }
            }
            if finished {
                self.busy_provider = None;
                self.checked = None;
            } else {
                self.worker = Some(worker);
            }
        }
        #[cfg(not(target_os = "android"))]
        if let Some(child) = self.child.as_mut() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    self.child = None;
                    completed = self.busy_provider.take().filter(|_| status.success());
                    self.checked = None;
                    if !status.success() && status.code() != Some(2) {
                        self.error = Some(match status.code() {
                            #[cfg(target_os = "linux")]
                            Some(127) => "Account login needs WebKitGTK 4.1. Install libwebkit2gtk-4.1-0 and try again.",
                            #[cfg(windows)]
                            Some(5) => "Account login needs Microsoft Edge WebView2. Install or repair WebView2 and try again.",
                            Some(4) => "Could not save account connection.",
                            _ => "Could not open or complete account login. Please try again.",
                        }.into());
                    }
                }
                Ok(None) => {}
                Err(_) => {
                    self.cancel();
                    self.error =
                        Some("Could not open or complete account login. Please try again.".into());
                }
            }
        }
        #[cfg(target_os = "android")]
        if self.worker.is_none() && self.busy_provider.is_some() {
            if let Some(result) = crate::android::poll_account_login(root) {
                self.busy_provider = None;
                self.checked = None;
                match result {
                    Ok(provider) => completed = provider,
                    Err(error) => self.error = Some(error),
                }
            }
        }
        if self
            .checked
            .is_some_and(|at| at.elapsed() < Duration::from_secs(1))
        {
            return None;
        }
        self.checked = Some(Instant::now());
        let store = AccountStore::for_project_root(xrtranslate_config::data_root(root));
        for &provider in AccountProvider::ALL {
            let snapshot = store.snapshot(provider);
            let status = snapshot.status;
            if self
                .snapshots
                .insert(provider, snapshot)
                .is_some_and(|previous| previous.status == AccountStatus::Connected)
                && status == AccountStatus::Expired
            {
                self.error = Some("Session expired. Sign in again.".into());
            }
        }
        completed.filter(|&provider| {
            if self.status(provider) == AccountStatus::Connected {
                true
            } else {
                self.error = Some("Cannot read account connection. Sign in again.".into());
                false
            }
        })
    }

    pub fn perform(&mut self, action: AccountAction, root: &Path) -> Result<(), String> {
        self.error = None;
        match action {
            AccountAction::Cancel => self.cancel(),
            AccountAction::SelectProfile(provider, id) => {
                let connected = self
                    .snapshot(provider)
                    .is_some_and(|s| s.profiles.iter().any(|p| p.id == id && p.connected));
                if !connected {
                    return self.perform(AccountAction::Connect(provider, Some(id)), root);
                }
                self.start_operation(provider, AccountOperation::SelectProfile(id), root)?;
            }
            AccountAction::RefreshModels(provider) => {
                self.start_operation(provider, AccountOperation::RefreshModels, root)?;
            }
            AccountAction::SignOut(provider, profile) => {
                self.start_operation(provider, AccountOperation::SignOut { profile }, root)?;
            }
            AccountAction::Connect(provider, profile) => {
                if self.is_busy() {
                    return Ok(());
                }
                if provider.browser_oauth() {
                    self.start_operation(provider, AccountOperation::SignIn { profile }, root)?;
                    return Ok(());
                }
                #[cfg(target_os = "android")]
                crate::android::start_account_login(provider)?;
                #[cfg(not(target_os = "android"))]
                {
                    let executable = std::env::current_exe()
                        .map_err(|_| "Cannot locate account login window.")?;
                    let name = if cfg!(windows) {
                        "xrtranslate-account-login.exe"
                    } else {
                        "xrtranslate-account-login"
                    };
                    let parent = executable
                        .parent()
                        .ok_or("Cannot locate account login window.")?;
                    let helper = [parent.join(name), root.join("bin").join(name)]
                        .into_iter()
                        .find(|path| path.is_file())
                        .ok_or("Account login component is missing. Reinstall the application.")?;
                    let mut command = std::process::Command::new(helper);
                    command
                        .arg(provider.id())
                        .arg(xrtranslate_config::data_root(root))
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null());
                    #[cfg(windows)]
                    crate::child_process::hide_console(&mut command);
                    self.child = Some(command.spawn().map_err(
                        |_| "Could not open or complete account login. Please try again.",
                    )?);
                }
                self.busy_provider = Some(provider);
            }
        }
        self.checked = None;
        Ok(())
    }

    fn start_operation(
        &mut self,
        provider: AccountProvider,
        action: AccountOperation,
        root: &Path,
    ) -> Result<(), String> {
        if self.is_busy() {
            return Ok(());
        }
        let login = matches!(action, AccountOperation::SignIn { .. });
        self.working_label = if login {
            "Complete sign-in in your browser. You’ll return here automatically."
        } else {
            "Updating account connection…"
        };
        let store = AccountStore::for_project_root(xrtranslate_config::data_root(root));
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancellation = cancelled.clone();
        let (sender, events) = mpsc::channel();
        std::thread::Builder::new()
            .name("account-connection".into())
            .spawn(move || {
                let result = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|_| xrtranslate_connections::ConnectionError::Network)
                    .and_then(|runtime| {
                        runtime.block_on(store.operate(provider, action, cancellation, |url| {
                            sender
                                .send(ConnectionEvent::Open(url))
                                .map_err(|_| xrtranslate_connections::ConnectionError::Cancelled)
                        }))
                    });
                #[cfg(target_os = "android")]
                if login && result.is_ok() {
                    crate::android::return_from_account_authorization();
                }
                let _ = sender.send(ConnectionEvent::Finished(result));
            })
            .map_err(|_| "Could not start account connection.".to_owned())?;
        self.worker = Some(ConnectionWorker {
            events,
            cancelled,
            login,
        });
        self.busy_provider = Some(provider);
        self.checked = None;
        Ok(())
    }

    fn cancel(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.cancelled.store(true, Ordering::Release);
        }
        #[cfg(not(target_os = "android"))]
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        #[cfg(target_os = "android")]
        if self.busy_provider.is_some() {
            crate::android::cancel_account_login();
        }
        self.busy_provider = None;
    }
}

impl Drop for AccountConnections {
    fn drop(&mut self) {
        self.cancel();
    }
}
