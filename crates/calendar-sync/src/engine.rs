//! One synchronization pass per account: upload queued local changes,
//! refresh the calendar list, then download server changes.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use calendar_core::{Account, ProviderKind};
use calendar_provider_api::{
    CalendarProvider, Capabilities, ObjectCodec, Precondition, ProviderError, RemoteCalendar,
    RemoteObject,
};
use calendar_storage::{
    IncomingObject, OpFailure, OpKind, PendingOp, Presence, ProblemOperation, PushObject,
    RemoteAccount, RemoteCalendarSpec, Resolution, StorageError, Store,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use url::Url;

use crate::credentials::CredentialStore;

/// Non-secret CalDAV account settings, stored as the account's config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalDavConfig {
    pub server_url: String,
    pub username: String,
    pub principal_url: String,
    pub calendar_home_url: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("{0}")]
    Provider(#[from] ProviderError),
    #[error("{0}")]
    Storage(#[from] StorageError),
    #[error("credentials: {0}")]
    Credentials(String),
    #[error("{0}")]
    Config(String),
    #[error("sign-in: {0}")]
    SignIn(#[from] calendar_oauth::OAuthError),
    #[error("synchronization took too long and was stopped")]
    TimedOut,
    #[error("a background task failed")]
    Task,
    #[error("cancelled")]
    Cancelled,
}

/// Status changes reported to the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum SyncEvent {
    #[serde(rename_all = "camelCase")]
    Syncing { account_id: String },
    #[serde(rename_all = "camelCase")]
    Synced { account_id: String, changed: bool },
    #[serde(rename_all = "camelCase")]
    Failed {
        account_id: String,
        message: String,
        offline: bool,
    },
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncReport {
    pub uploaded: usize,
    pub conflicts: usize,
    pub downloaded: usize,
    pub deleted: usize,
    pub failed_objects: usize,
    /// Uploads the server refused for good (rows are now `sync-failed`).
    pub failed_uploads: usize,
    pub calendars_changed: bool,
}

impl SyncReport {
    /// Whether anything the UI shows (events or their sync status) changed.
    pub fn changed(&self) -> bool {
        self.uploaded + self.downloaded + self.deleted + self.conflicts + self.failed_uploads > 0
            || self.calendars_changed
    }
}

pub type ProviderFactory = Arc<
    dyn Fn(&RemoteAccount, &str) -> Result<Box<dyn CalendarProvider>, ProviderError> + Send + Sync,
>;
pub type Listener = Arc<dyn Fn(SyncEvent) + Send + Sync>;

/// Non-secret settings of an OAuth account (Google, Microsoft), stored as
/// the account's config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthConfig {
    /// OAuth client registration used for this account.
    pub client_id: String,
    pub email: String,
}

fn damaged() -> ProviderError {
    ProviderError::InvalidResponse("account settings are damaged".into())
}

/// Default factory: providers for CalDAV and Google accounts from their
/// stored config and secret.
pub fn default_factory() -> ProviderFactory {
    Arc::new(
        |account: &RemoteAccount, secret: &str| match account.account.provider {
            ProviderKind::Google => {
                let config: OAuthConfig =
                    serde_json::from_value(account.config.clone()).map_err(|_| damaged())?;
                let api = Url::parse(calendar_google::API_BASE).expect("valid");
                let provider =
                    calendar_google::GoogleProvider::connect(&config.client_id, secret, api)?;
                Ok(Box::new(provider) as Box<dyn CalendarProvider>)
            }
            ProviderKind::Microsoft => {
                let config: OAuthConfig =
                    serde_json::from_value(account.config.clone()).map_err(|_| damaged())?;
                let api = Url::parse(calendar_microsoft::API_BASE).expect("valid");
                let provider =
                    calendar_microsoft::GraphProvider::connect(&config.client_id, secret, api)?;
                Ok(Box::new(provider) as Box<dyn CalendarProvider>)
            }
            ProviderKind::Caldav => {
                let config: CalDavConfig =
                    serde_json::from_value(account.config.clone()).map_err(|_| damaged())?;
                let home = Url::parse(&config.calendar_home_url)
                    .map_err(|_| ProviderError::InvalidResponse("bad calendar home URL".into()))?;
                let provider =
                    calendar_caldav::CalDavProvider::new(home, &config.username, secret)?;
                Ok(Box::new(provider) as Box<dyn CalendarProvider>)
            }
            other => Err(ProviderError::InvalidResponse(format!(
                "{} accounts are not supported yet",
                other.as_str()
            ))),
        },
    )
}

/// Opens a URL in the user's browser (supplied by the desktop shell).
pub type BrowserOpener = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

/// Retry delay after `attempts` failures: 30 s doubling up to 1 h.
pub fn backoff(attempts: i64) -> Duration {
    let exp = attempts.clamp(0, 7) as u32;
    Duration::from_secs((30u64 << exp).min(3600))
}

/// Accounts synced at the same time.
const MAX_PARALLEL_ACCOUNTS: usize = 3;

/// Upper bound on one account's pass; a server that keeps answering slowly
/// must not hold its account (or app shutdown) forever.
const PASS_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Failed passes of one account, for account-level backoff.
#[derive(Debug, Clone, Copy)]
struct Backoff {
    failures: i64,
    until_ms: i64,
}

pub struct SyncEngine {
    store: Store,
    credentials: Arc<dyn CredentialStore>,
    providers: ProviderFactory,
    listener: Listener,
    /// One pass at a time per account; accounts sync independently.
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Accounts whose last pass failed, and when to try them again.
    backoff: Mutex<HashMap<String, Backoff>>,
}

/// One problem operation, described for the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncProblem {
    pub account_id: String,
    pub calendar_id: String,
    pub href: String,
    pub title: String,
    pub deletion: bool,
    pub conflict: bool,
    pub message: String,
}

/// The codec for objects of an account kind (also needed without a
/// connected provider, e.g. to describe a deleted object).
pub fn codec_for(kind: ProviderKind) -> Arc<dyn ObjectCodec> {
    match kind {
        ProviderKind::Google => Arc::new(calendar_google::GoogleCodec),
        ProviderKind::Microsoft => Arc::new(calendar_microsoft::GraphCodec),
        _ => Arc::new(calendar_caldav::ICalendarCodec),
    }
}

/// Reads downloaded objects. Input is untrusted: a codec panic (a bug, but
/// possible) fails that object only instead of the sync task.
fn read_objects(codec: &dyn ObjectCodec, objects: Vec<RemoteObject>) -> Vec<IncomingObject> {
    objects
        .into_iter()
        .map(|o| {
            let read =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| codec.read(&o.data)))
                    .unwrap_or_else(|_| Err("the event data could not be read".into()));
            let (content, warnings) = match read {
                Ok(r) => (Ok(r.content), r.warnings),
                Err(e) => (Err(e), Vec::new()),
            };
            IncomingObject {
                href: o.href,
                etag: o.etag,
                ical: o.data,
                content,
                warnings,
            }
        })
        .collect()
}

/// Title of an object's first event, for describing objects that have no
/// local rows (deletions).
fn summary(codec: &dyn ObjectCodec, data: &str) -> Option<String> {
    let content = codec.read(data).ok()?.content;
    content
        .master
        .map(|m| m.title)
        .or_else(|| content.overrides.into_iter().next().map(|o| o.title))
        .filter(|t| !t.is_empty())
}

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

fn spec(c: &RemoteCalendar) -> RemoteCalendarSpec {
    RemoteCalendarSpec {
        remote_url: c.href.clone(),
        name: c.name.clone(),
        color: c.color.clone(),
        read_only: c.read_only,
        supports_sync: c.capabilities.incremental_sync,
        ctag: c.ctag.clone(),
    }
}

fn new_account_id(kind: &str) -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!(
        "{kind}-{nanos:x}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

impl SyncEngine {
    pub fn new(
        store: Store,
        credentials: Arc<dyn CredentialStore>,
        providers: ProviderFactory,
        listener: Listener,
    ) -> Self {
        // Outlook can hold fewer repeat rules than iCalendar.
        store.set_recurrence_check(ProviderKind::Microsoft, calendar_microsoft::pattern::check);
        Self {
            store,
            credentials,
            providers,
            listener,
            locks: Mutex::new(HashMap::new()),
            backoff: Mutex::new(HashMap::new()),
        }
    }

    fn account_lock(&self, account_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.locks.lock().unwrap_or_else(|e| e.into_inner());
        Arc::clone(locks.entry(account_id.to_owned()).or_default())
    }

    fn backoff_until(&self, account_id: &str) -> Option<i64> {
        let map = self.backoff.lock().unwrap_or_else(|e| e.into_inner());
        map.get(account_id).map(|b| b.until_ms)
    }

    fn record_pass(&self, account_id: &str, ok: bool) {
        let mut map = self.backoff.lock().unwrap_or_else(|e| e.into_inner());
        if ok {
            map.remove(account_id);
        } else {
            let failures = map.get(account_id).map_or(0, |b| b.failures) + 1;
            let delay = i64::try_from(backoff(failures - 1).as_millis()).unwrap_or(i64::MAX / 4);
            map.insert(
                account_id.to_owned(),
                Backoff {
                    failures,
                    until_ms: now_ms() + delay,
                },
            );
        }
    }

    async fn secret(&self, account_id: &str) -> Result<Option<String>, SyncError> {
        let creds = Arc::clone(&self.credentials);
        let id = account_id.to_owned();
        tokio::task::spawn_blocking(move || creds.get(&id))
            .await
            .map_err(|_| SyncError::Credentials("keyring task failed".into()))?
            .map_err(SyncError::Credentials)
    }

    async fn set_secret(&self, account_id: &str, secret: &str) -> Result<(), SyncError> {
        let creds = Arc::clone(&self.credentials);
        let (id, secret) = (account_id.to_owned(), secret.to_owned());
        tokio::task::spawn_blocking(move || creds.set(&id, &secret))
            .await
            .map_err(|_| SyncError::Credentials("keyring task failed".into()))?
            .map_err(SyncError::Credentials)
    }

    /// Discovers a CalDAV account, stores it (password in the credential
    /// store only) and returns its id. Nothing is saved if any step fails.
    pub async fn add_caldav_account(
        &self,
        server_url: &str,
        username: &str,
        password: &str,
    ) -> Result<String, SyncError> {
        self.add_caldav_account_cancellable(
            server_url,
            username,
            password,
            &tokio::sync::Notify::new(),
        )
        .await
    }

    /// [`Self::add_caldav_account`] that gives up when `cancel` is notified
    /// during discovery (the network part). Nothing is stored before
    /// discovery has finished, so cancelling never leaves anything behind.
    pub async fn add_caldav_account_cancellable(
        &self,
        server_url: &str,
        username: &str,
        password: &str,
        cancel: &tokio::sync::Notify,
    ) -> Result<String, SyncError> {
        let mut server = Url::parse(server_url.trim())
            .or_else(|_| Url::parse(&format!("https://{}", server_url.trim())))
            .map_err(|_| {
                SyncError::Config("enter a server address such as https://cloud.example.org".into())
            })?;
        if !matches!(server.scheme(), "https" | "http") {
            return Err(SyncError::Config(
                "the server address must start with https://".into(),
            ));
        }
        // The address is stored as plain configuration: never a password.
        if !server.username().is_empty() || server.password().is_some() {
            return Err(SyncError::Config(
                "enter the user name and password in their own fields, not in the address".into(),
            ));
        }
        if !server.path().ends_with('/') {
            let path = format!("{}/", server.path());
            server.set_path(&path);
        }
        let discovery = async {
            let found = calendar_caldav::discover(&server, username, password).await?;
            let provider = calendar_caldav::CalDavProvider::new(
                found.calendar_home.clone(),
                username,
                password,
            )?;
            let calendars = provider.list_calendars().await?;
            Ok::<_, SyncError>((found, calendars))
        };
        let (found, calendars) = tokio::select! {
            result = discovery => result?,
            () = cancel.notified() => return Err(SyncError::Cancelled),
        };

        let id = new_account_id("caldav");
        let config = CalDavConfig {
            server_url: server.to_string(),
            username: username.to_owned(),
            principal_url: found.principal.to_string(),
            calendar_home_url: found.calendar_home.to_string(),
        };
        let account = Account {
            id: id.clone(),
            name: format!("{username} @ {}", server.host_str().unwrap_or("server")),
            provider: ProviderKind::Caldav,
        };
        let config = serde_json::to_value(&config).map_err(|e| SyncError::Config(e.to_string()))?;
        self.save_new_account(&account, &config, &calendars, password)
            .await?;
        Ok(id)
    }

    /// Signs in to Google in the browser and adds the account with its
    /// calendars. Tokens go to the credential store only.
    pub async fn add_google_account(
        &self,
        client_id: &str,
        client_secret: Option<&str>,
        open_browser: BrowserOpener,
        cancel: &tokio::sync::Notify,
    ) -> Result<String, SyncError> {
        self.add_oauth_account(
            ProviderKind::Google,
            client_id,
            client_secret,
            open_browser,
            cancel,
            None,
        )
        .await
    }

    /// Signs in to Microsoft (personal or work/school) and adds the account.
    pub async fn add_microsoft_account(
        &self,
        client_id: &str,
        open_browser: BrowserOpener,
        cancel: &tokio::sync::Notify,
    ) -> Result<String, SyncError> {
        self.add_oauth_account(
            ProviderKind::Microsoft,
            client_id,
            None,
            open_browser,
            cancel,
            None,
        )
        .await
    }

    /// OAuth client settings for an account kind.
    fn oauth_client_for(
        kind: ProviderKind,
        client_id: &str,
        client_secret: Option<&str>,
    ) -> Result<(calendar_oauth::OAuthClient, Url), SyncError> {
        Ok(match kind {
            ProviderKind::Google => (
                calendar_google::oauth_client(client_id, client_secret),
                Url::parse(calendar_google::API_BASE).expect("valid"),
            ),
            ProviderKind::Microsoft => (
                calendar_microsoft::oauth_client(client_id),
                Url::parse(calendar_microsoft::API_BASE).expect("valid"),
            ),
            other => {
                return Err(SyncError::Config(format!(
                    "{} accounts do not sign in with OAuth",
                    other.as_str()
                )));
            }
        })
    }

    /// The stored secret of an OAuth account kind.
    fn oauth_secret(
        kind: ProviderKind,
        tokens: calendar_oauth::Tokens,
        client_secret: Option<String>,
    ) -> Result<String, SyncError> {
        let json = match kind {
            ProviderKind::Microsoft => {
                serde_json::to_string(&calendar_microsoft::GraphSecret { tokens })
            }
            _ => serde_json::to_string(&calendar_google::GoogleSecret {
                tokens,
                client_secret,
            }),
        };
        json.map_err(|e| SyncError::Config(e.to_string()))
    }

    /// Adds an OAuth account (Google or Microsoft): browser sign-in, then
    /// the account's address and calendars. `endpoints` replaces the OAuth
    /// and API endpoints (tests).
    pub async fn add_oauth_account(
        &self,
        kind: ProviderKind,
        client_id: &str,
        client_secret: Option<&str>,
        open_browser: BrowserOpener,
        cancel: &tokio::sync::Notify,
        endpoints: Option<(calendar_oauth::OAuthClient, Url)>,
    ) -> Result<String, SyncError> {
        let client_id = client_id.trim();
        if client_id.is_empty() {
            return Err(SyncError::Config("enter the OAuth client ID".into()));
        }
        let client_secret = client_secret.map(str::trim).filter(|s| !s.is_empty());
        let (client, api) = match endpoints {
            Some(e) => e,
            None => Self::oauth_client_for(kind, client_id, client_secret)?,
        };
        let tokens = calendar_oauth::authorize(&client, &*open_browser, cancel).await?;
        let source = Arc::new(calendar_oauth::TokenSource::new(client, tokens.clone()));
        let (email, calendars, label) = match kind {
            ProviderKind::Microsoft => {
                let provider = calendar_microsoft::GraphProvider::new(source, api)?;
                (
                    provider.account_email().await?,
                    provider.list_calendars().await?,
                    "Microsoft",
                )
            }
            _ => {
                let provider = calendar_google::GoogleProvider::new(source, api)?;
                (
                    provider.account_email().await?,
                    provider.list_calendars().await?,
                    "Google",
                )
            }
        };
        if self
            .store
            .remote_accounts()
            .await?
            .iter()
            .any(|a| a.account.provider == kind && a.config["email"] == email)
        {
            return Err(SyncError::Config(format!("{email} is already added")));
        }
        let id = new_account_id(kind.as_str());
        let account = Account {
            id: id.clone(),
            name: format!("{email} ({label})"),
            provider: kind,
        };
        let config = serde_json::to_value(OAuthConfig {
            client_id: client_id.to_owned(),
            email,
        })
        .map_err(|e| SyncError::Config(e.to_string()))?;
        let secret = Self::oauth_secret(kind, tokens, client_secret.map(str::to_owned))?;
        self.save_new_account(&account, &config, &calendars, &secret)
            .await?;
        Ok(id)
    }

    /// Signs in again to an OAuth account whose sign-in expired or was
    /// revoked. Queued changes are kept and sync resumes.
    pub async fn sign_in_again(
        &self,
        account_id: &str,
        open_browser: BrowserOpener,
        cancel: &tokio::sync::Notify,
    ) -> Result<(), SyncError> {
        let account = self
            .store
            .remote_accounts()
            .await?
            .into_iter()
            .find(|a| a.account.id == account_id)
            .ok_or_else(|| SyncError::Config("the account no longer exists".into()))?;
        let kind = account.account.provider;
        let config: OAuthConfig = serde_json::from_value(account.config.clone())
            .map_err(|_| SyncError::Provider(damaged()))?;
        let client_secret = match kind {
            ProviderKind::Google => self
                .secret(account_id)
                .await?
                .and_then(|s| serde_json::from_str::<calendar_google::GoogleSecret>(&s).ok())
                .and_then(|s| s.client_secret),
            _ => None,
        };
        let (client, _) =
            Self::oauth_client_for(kind, &config.client_id, client_secret.as_deref())?;
        let tokens = calendar_oauth::authorize(&client, &*open_browser, cancel).await?;
        let secret = Self::oauth_secret(kind, tokens, client_secret)?;
        self.set_secret(account_id, &secret).await?;
        self.store
            .set_account_status(account_id, None, None, false)
            .await?;
        self.record_pass(account_id, true);
        Ok(())
    }

    /// Stores a new account: secret first, then the database rows; the
    /// secret is removed again if the rows cannot be written.
    async fn save_new_account(
        &self,
        account: &Account,
        config: &serde_json::Value,
        calendars: &[RemoteCalendar],
        secret: &str,
    ) -> Result<(), SyncError> {
        self.set_secret(&account.id, secret).await?;
        let specs: Vec<_> = calendars.iter().map(spec).collect();
        if let Err(e) = self
            .store
            .create_remote_account(account, config, &specs)
            .await
        {
            let creds = Arc::clone(&self.credentials);
            let cleanup = account.id.clone();
            let _ = tokio::task::spawn_blocking(move || creds.delete(&cleanup)).await;
            return Err(e.into());
        }
        Ok(())
    }

    /// Removes an account, its local data and its stored password.
    pub async fn remove_account(&self, account_id: &str) -> Result<(), SyncError> {
        let lock = self.account_lock(account_id);
        let _guard = lock.lock().await;
        self.store.remove_account(account_id).await?;
        let creds = Arc::clone(&self.credentials);
        let id = account_id.to_owned();
        tokio::task::spawn_blocking(move || creds.delete(&id))
            .await
            .map_err(|_| SyncError::Credentials("keyring task failed".into()))?
            .map_err(SyncError::Credentials)
    }

    /// Syncs every remote account, a few at a time. Failures, slowness and
    /// panics are isolated per account. Unless `forced` (a local change or
    /// "Sync now"), accounts whose last pass failed wait out their backoff.
    pub async fn sync_all(self: &Arc<Self>, forced: bool) -> Result<(), SyncError> {
        let permits = Arc::new(Semaphore::new(MAX_PARALLEL_ACCOUNTS));
        let mut tasks = tokio::task::JoinSet::new();
        for account in self.store.remote_accounts().await? {
            let id = account.account.id;
            if !forced && self.backoff_until(&id).is_some_and(|t| t > now_ms()) {
                continue;
            }
            let engine = Arc::clone(self);
            let permits = Arc::clone(&permits);
            tasks.spawn(async move {
                let _permit = permits.acquire().await;
                let _ = engine.sync_account(&id).await;
            });
        }
        while let Some(done) = tasks.join_next().await {
            if let Err(e) = done
                && e.is_panic()
            {
                eprintln!("[sync] an account sync task panicked");
            }
        }
        Ok(())
    }

    /// Earliest time (epoch ms) a pass is needed for queued retries, across
    /// accounts, respecting account backoff.
    pub async fn next_retry_ms(&self) -> Result<Option<i64>, SyncError> {
        let mut next: Option<i64> = None;
        for account in self.store.remote_accounts().await? {
            let id = &account.account.id;
            if let Some(t) = self.store.next_retry_ms(id).await? {
                let t = t.max(self.backoff_until(id).unwrap_or(0));
                next = Some(next.map_or(t, |n| n.min(t)));
            }
        }
        Ok(next)
    }

    pub async fn sync_account(&self, account_id: &str) -> Result<SyncReport, SyncError> {
        let lock = self.account_lock(account_id);
        let _guard = lock.lock().await;
        (self.listener)(SyncEvent::Syncing {
            account_id: account_id.to_owned(),
        });
        let result = match tokio::time::timeout(PASS_TIMEOUT, self.run(account_id)).await {
            Ok(result) => result,
            Err(_) => Err(SyncError::TimedOut),
        };
        self.record_pass(account_id, result.is_ok());
        match &result {
            Ok(report) => {
                self.store
                    .set_account_status(account_id, None, Some(now_ms()), false)
                    .await?;
                (self.listener)(SyncEvent::Synced {
                    account_id: account_id.to_owned(),
                    changed: report.changed(),
                });
            }
            Err(e) => {
                let offline = matches!(e, SyncError::Provider(p) if p.is_transient())
                    || matches!(e, SyncError::TimedOut);
                let oauth = self.is_oauth(account_id).await;
                let needs_sign_in =
                    oauth && matches!(e, SyncError::Provider(ProviderError::Unauthorized));
                let message = if needs_sign_in {
                    "The sign-in has expired or was revoked. Sign in again to resume syncing; nothing is lost.".to_owned()
                } else {
                    user_message(e)
                };
                self.store
                    .set_account_status(account_id, Some(&message), None, needs_sign_in)
                    .await?;
                (self.listener)(SyncEvent::Failed {
                    account_id: account_id.to_owned(),
                    message,
                    offline,
                });
            }
        }
        result
    }

    async fn is_oauth(&self, account_id: &str) -> bool {
        self.store.remote_accounts().await.is_ok_and(|accounts| {
            accounts.iter().any(|a| {
                a.account.id == account_id
                    && matches!(
                        a.account.provider,
                        ProviderKind::Google | ProviderKind::Microsoft
                    )
            })
        })
    }

    async fn run(&self, account_id: &str) -> Result<SyncReport, SyncError> {
        let account = self
            .store
            .remote_accounts()
            .await?
            .into_iter()
            .find(|a| a.account.id == account_id)
            .ok_or_else(|| SyncError::Config("the account no longer exists".into()))?;
        let password = self.secret(account_id).await?.ok_or_else(|| {
            SyncError::Credentials("the password is missing from the keyring".into())
        })?;
        let provider = (self.providers)(&account, &password)?;
        let result = self.pass(account_id, provider.as_ref()).await;
        // Rotated OAuth tokens must be kept even if the pass failed.
        if let Some(secret) = provider.take_updated_secret() {
            self.set_secret(account_id, &secret).await?;
        }
        result
    }

    async fn pass(
        &self,
        account_id: &str,
        provider: &dyn CalendarProvider,
    ) -> Result<SyncReport, SyncError> {
        let mut report = SyncReport::default();

        // 1. Upload first, so server changes never silently replace queued edits.
        for op in self.store.due_operations(account_id, now_ms()).await? {
            self.push(provider, &op, &mut report).await?;
        }

        // 2. Calendar list (new, renamed, removed, permissions).
        let calendars = provider.list_calendars().await?;
        let specs: Vec<_> = calendars.iter().map(spec).collect();
        report.calendars_changed = self
            .store
            .update_remote_calendars(account_id, &specs)
            .await?;

        // 3. Download changes per calendar.
        for state in self.store.remote_calendars(account_id).await? {
            let remote = RemoteCalendar {
                href: state.remote_url.clone(),
                name: state.name.clone(),
                color: None,
                read_only: state.read_only,
                ctag: state.ctag.clone(),
                capabilities: Capabilities {
                    incremental_sync: state.supports_sync,
                    conditional_writes: true,
                },
            };
            let known = self.store.known_objects(&state.calendar_id).await?;
            let changes = match provider
                .fetch_changes(&remote, state.sync_token.as_deref(), &known)
                .await
            {
                Ok(c) => c,
                // A calendar we lost access to should not stop the others.
                Err(ProviderError::Forbidden(_) | ProviderError::NotFound(_)) => continue,
                Err(e) => return Err(e.into()),
            };
            // Parsing is CPU work on untrusted input: off the async workers.
            let upserted = changes.upserted;
            let codec = provider.codec();
            let objects = tokio::task::spawn_blocking(move || read_objects(&*codec, upserted))
                .await
                .map_err(|_| SyncError::Task)?;
            let applied = self
                .store
                .apply_remote_changes(
                    &state.calendar_id,
                    &objects,
                    &changes.deleted,
                    changes.cursor.as_deref(),
                )
                .await?;
            for (href, reason) in &applied.failed {
                eprintln!("[sync] skipped {href}: {reason}");
            }
            report.downloaded += applied.applied;
            report.deleted += applied.deleted;
            report.failed_objects += applied.failed.len();
        }
        Ok(report)
    }

    async fn push(
        &self,
        provider: &dyn CalendarProvider,
        op: &PendingOp,
        report: &mut SyncReport,
    ) -> Result<(), SyncError> {
        let Some(obj) = self
            .store
            .object_for_push(&op.calendar_id, &op.href)
            .await?
        else {
            // The object is gone locally and never existed remotely.
            self.store.complete_delete(op).await?;
            return Ok(());
        };
        let outcome = match op.kind {
            OpKind::Put => self.upload(provider, op, &obj).await?,
            OpKind::Delete => self.delete(provider, op, &obj).await?,
        };
        match outcome {
            Ok(()) => report.uploaded += 1,
            Err(ProviderError::PreconditionFailed) => {
                report.conflicts += 1;
                self.store
                    .fail_operation(
                        op,
                        "the event was changed on the server",
                        OpFailure::Conflict,
                    )
                    .await?;
            }
            Err(e @ ProviderError::Unauthorized) => return Err(e.into()),
            Err(ProviderError::RateLimited { retry_after }) => {
                let delay = retry_after.unwrap_or_else(|| backoff(op.attempts));
                let at = now_ms() + i64::try_from(delay.as_millis()).unwrap_or(i64::MAX / 2);
                self.store
                    .fail_operation(
                        op,
                        "the server asked to slow down",
                        OpFailure::Retry { at_ms: at },
                    )
                    .await?;
                return Err(ProviderError::RateLimited { retry_after }.into());
            }
            Err(e) if e.is_transient() => {
                let at = now_ms()
                    + i64::try_from(backoff(op.attempts).as_millis()).unwrap_or(i64::MAX / 2);
                self.store
                    .fail_operation(op, &e.to_string(), OpFailure::Retry { at_ms: at })
                    .await?;
                // Offline or server down: stop this pass, keep the queue.
                return Err(e.into());
            }
            Err(e) => {
                report.failed_uploads += 1;
                self.store
                    .fail_operation(op, &e.to_string(), OpFailure::Permanent)
                    .await?;
            }
        }
        Ok(())
    }
}

impl SyncEngine {
    /// Uploads the local version of an object. The outer error is local
    /// (storage); the inner one is the provider's answer.
    async fn upload(
        &self,
        provider: &dyn CalendarProvider,
        op: &PendingOp,
        obj: &PushObject,
    ) -> Result<Result<(), ProviderError>, SyncError> {
        if obj.presence == Presence::Local
            && !self.store.begin_upload(&op.calendar_id, &op.href).await?
        {
            // Deleted locally before anything was sent.
            return Ok(Ok(()));
        }
        let base = Some(obj.base_ical.as_str()).filter(|b| !b.trim().is_empty());
        let payload = provider
            .codec()
            .write(base, &obj.uid, &obj.content, &obj.renamed);
        let precondition = if op.force {
            Precondition::Overwrite
        } else {
            match (&obj.etag, obj.presence) {
                (Some(etag), _) => Precondition::Version(etag.clone()),
                (None, Presence::Local) => Precondition::Create,
                // The server may hold it, in a version we do not know (a
                // create whose answer was lost, or a write answered without
                // an ETag): ask before writing.
                (None, presence) => match provider.get_object(&op.href).await {
                    Ok(Some(RemoteObject {
                        etag: Some(etag), ..
                    })) => Precondition::Version(etag),
                    // A server without ETags cannot check versions.
                    Ok(Some(_)) => Precondition::Overwrite,
                    Ok(None) if presence == Presence::Sent => Precondition::Create,
                    // Deleted on the server while changed here.
                    Ok(None) => return Ok(Err(ProviderError::PreconditionFailed)),
                    Err(e) => return Ok(Err(e)),
                },
            }
        };
        let written = match provider.put_object(&op.href, &payload, &precondition).await {
            Ok(w) => w,
            Err(e) => return Ok(Err(e)),
        };
        // Providers that assign ids on create report where the object is.
        let op = if written.href != op.href {
            self.store
                .rename_object(&op.calendar_id, &op.href, &written.href)
                .await?;
            PendingOp {
                href: written.href.clone(),
                ..op.clone()
            }
        } else {
            op.clone()
        };
        let (etag, stored) = match (written.etag, written.data) {
            (etag, Some(data)) => (etag, data),
            // Without an ETag the server may have changed the data it
            // stored (RFC 4791 §5.3.4): read back its version and copy.
            (None, None) => match provider.get_object(&op.href).await {
                Ok(Some(current)) => (current.etag, current.data),
                _ => (None, payload),
            },
            (etag, None) => (etag, payload),
        };
        self.store
            .complete_put(&op, etag.as_deref(), &stored)
            .await?;
        Ok(Ok(()))
    }

    async fn delete(
        &self,
        provider: &dyn CalendarProvider,
        op: &PendingOp,
        obj: &PushObject,
    ) -> Result<Result<(), ProviderError>, SyncError> {
        let version = match (&obj.etag, op.force) {
            (_, true) => None,
            (Some(etag), false) => Some(etag.clone()),
            (None, false) => match provider.get_object(&op.href).await {
                Ok(Some(current)) => current.etag,
                Ok(None) => {
                    // Never arrived, or already gone.
                    self.store.complete_delete(op).await?;
                    return Ok(Ok(()));
                }
                Err(e) => return Ok(Err(e)),
            },
        };
        match provider.delete_object(&op.href, version.as_deref()).await {
            Ok(()) => {
                self.store.complete_delete(op).await?;
                Ok(Ok(()))
            }
            Err(e) => Ok(Err(e)),
        }
    }

    /// Failed and conflicting changes of all accounts, for the UI.
    pub async fn problems(&self) -> Result<Vec<SyncProblem>, SyncError> {
        let mut out = Vec::new();
        for account in self.store.remote_accounts().await? {
            let account_id = account.account.id;
            let codec = codec_for(account.account.provider);
            for p in self.store.problem_operations(&account_id).await? {
                out.push(describe(&*codec, &account_id, p));
            }
        }
        Ok(out)
    }

    /// Resolves a failed or conflicting change.
    pub async fn resolve(
        &self,
        calendar_id: &str,
        href: &str,
        resolution: Resolution,
    ) -> Result<(), SyncError> {
        Ok(self
            .store
            .resolve_operation(calendar_id, href, resolution)
            .await?)
    }
}

fn describe(codec: &dyn ObjectCodec, account_id: &str, p: ProblemOperation) -> SyncProblem {
    let deletion = p.kind == OpKind::Delete;
    let title = p
        .title
        .clone()
        .filter(|t| !t.is_empty())
        .or_else(|| summary(codec, &p.base_ical))
        .unwrap_or_else(|| "Untitled event".into());
    let message = match (p.conflict, deletion) {
        (true, true) => "You deleted this event, but it was changed on the server.".to_owned(),
        (true, false) => "This event was changed on the server and here.".to_owned(),
        (false, _) => format!(
            "The server refused the change: {}.",
            p.error.as_deref().unwrap_or("unknown error")
        ),
    };
    SyncProblem {
        account_id: account_id.to_owned(),
        calendar_id: p.calendar_id,
        href: p.href,
        title,
        deletion,
        conflict: p.conflict,
        message,
    }
}

/// Readable message for the UI (never contains secrets).
pub fn user_message(e: &SyncError) -> String {
    match e {
        SyncError::Provider(ProviderError::Unauthorized) => {
            "The server rejected the user name or password. Check the account (an app password may be required).".into()
        }
        SyncError::Provider(ProviderError::Network(_)) => "Offline: the server could not be reached. Changes are kept and will sync later.".into(),
        SyncError::Provider(ProviderError::RateLimited { .. }) => "The server is busy; syncing will resume shortly.".into(),
        SyncError::Provider(ProviderError::Server { status }) => format!("The server reported an error (HTTP {status}). Will retry."),
        SyncError::Provider(other) => format!("Sync problem: {other}."),
        SyncError::Storage(_) => "The local database could not be updated.".into(),
        SyncError::Credentials(m) => format!("Could not read the account password: {m}."),
        SyncError::Config(m) => m.clone(),
        SyncError::TimedOut => "Synchronization took too long and will be retried.".into(),
        SyncError::Task => "Synchronization failed unexpectedly and will be retried.".into(),
        SyncError::Cancelled => "Cancelled.".into(),
        SyncError::SignIn(calendar_oauth::OAuthError::Cancelled) => "Cancelled.".into(),
        SyncError::SignIn(e) => format!("Sign-in failed: {e}."),
    }
}
