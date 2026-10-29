//! One synchronization pass per account: upload queued local changes,
//! refresh the calendar list, then download server changes.

use std::sync::Arc;
use std::time::Duration;

use calendar_core::{Account, ProviderKind};
use calendar_ical::{parse, read_events, write_events};
use calendar_provider_api::{
    CalendarProvider, Capabilities, Precondition, ProviderError, RemoteCalendar,
};
use calendar_storage::{
    IncomingObject, OpFailure, OpKind, PendingOp, RemoteAccount, RemoteCalendarSpec, StorageError,
    Store,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
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
    pub calendars_changed: bool,
}

impl SyncReport {
    pub fn changed(&self) -> bool {
        self.uploaded + self.downloaded + self.deleted + self.conflicts > 0
            || self.calendars_changed
    }
}

pub type ProviderFactory = Arc<
    dyn Fn(&RemoteAccount, &str) -> Result<Box<dyn CalendarProvider>, ProviderError> + Send + Sync,
>;
pub type Listener = Arc<dyn Fn(SyncEvent) + Send + Sync>;

/// Default factory: CalDAV accounts from their stored config.
pub fn caldav_factory() -> ProviderFactory {
    Arc::new(|account: &RemoteAccount, password: &str| {
        let config: CalDavConfig = serde_json::from_value(account.config.clone())
            .map_err(|_| ProviderError::InvalidResponse("account settings are damaged".into()))?;
        let home = Url::parse(&config.calendar_home_url)
            .map_err(|_| ProviderError::InvalidResponse("bad calendar home URL".into()))?;
        let provider = calendar_caldav::CalDavProvider::new(home, &config.username, password)?;
        Ok(Box::new(provider) as Box<dyn CalendarProvider>)
    })
}

/// Retry delay after `attempts` failures: 30 s doubling up to 1 h.
pub fn backoff(attempts: i64) -> Duration {
    let exp = attempts.clamp(0, 7) as u32;
    Duration::from_secs((30u64 << exp).min(3600))
}

pub struct SyncEngine {
    store: Store,
    credentials: Arc<dyn CredentialStore>,
    providers: ProviderFactory,
    listener: Listener,
    /// One pass at a time per engine (accounts are synced sequentially).
    lock: tokio::sync::Mutex<()>,
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

fn new_account_id() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!(
        "caldav-{nanos:x}-{}",
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
        Self {
            store,
            credentials,
            providers,
            listener,
            lock: tokio::sync::Mutex::new(()),
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
        if !server.path().ends_with('/') {
            let path = format!("{}/", server.path());
            server.set_path(&path);
        }
        let found = calendar_caldav::discover(&server, username, password).await?;
        let provider =
            calendar_caldav::CalDavProvider::new(found.calendar_home.clone(), username, password)?;
        let calendars = provider.list_calendars().await?;

        let id = new_account_id();
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
        self.set_secret(&id, password).await?;
        let specs: Vec<_> = calendars.iter().map(spec).collect();
        let config = serde_json::to_value(&config).map_err(|e| SyncError::Config(e.to_string()))?;
        if let Err(e) = self
            .store
            .create_remote_account(&account, &config, &specs)
            .await
        {
            let creds = Arc::clone(&self.credentials);
            let cleanup = id.clone();
            let _ = tokio::task::spawn_blocking(move || creds.delete(&cleanup)).await;
            return Err(e.into());
        }
        Ok(id)
    }

    /// Removes an account, its local data and its stored password.
    pub async fn remove_account(&self, account_id: &str) -> Result<(), SyncError> {
        let _guard = self.lock.lock().await;
        self.store.remove_account(account_id).await?;
        let creds = Arc::clone(&self.credentials);
        let id = account_id.to_owned();
        tokio::task::spawn_blocking(move || creds.delete(&id))
            .await
            .map_err(|_| SyncError::Credentials("keyring task failed".into()))?
            .map_err(SyncError::Credentials)
    }

    /// Syncs every remote account. Failures are isolated per account.
    pub async fn sync_all(&self) -> Result<(), SyncError> {
        for account in self.store.remote_accounts().await? {
            let _ = self.sync_account(&account.account.id).await;
        }
        Ok(())
    }

    /// Earliest time (epoch ms) a queued retry becomes due, across accounts.
    pub async fn next_retry_ms(&self) -> Result<Option<i64>, SyncError> {
        let mut next: Option<i64> = None;
        for account in self.store.remote_accounts().await? {
            if let Some(t) = self.store.next_retry_ms(&account.account.id).await? {
                next = Some(next.map_or(t, |n| n.min(t)));
            }
        }
        Ok(next)
    }

    pub async fn sync_account(&self, account_id: &str) -> Result<SyncReport, SyncError> {
        let _guard = self.lock.lock().await;
        (self.listener)(SyncEvent::Syncing {
            account_id: account_id.to_owned(),
        });
        let result = self.run(account_id).await;
        match &result {
            Ok(report) => {
                self.store
                    .set_account_status(account_id, None, Some(now_ms()))
                    .await?;
                (self.listener)(SyncEvent::Synced {
                    account_id: account_id.to_owned(),
                    changed: report.changed(),
                });
            }
            Err(e) => {
                let offline = matches!(e, SyncError::Provider(p) if p.is_transient());
                let message = user_message(e);
                self.store
                    .set_account_status(account_id, Some(&message), None)
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
        let mut report = SyncReport::default();

        // 1. Upload first, so server changes never silently replace queued edits.
        for op in self.store.due_operations(account_id, now_ms()).await? {
            self.push(provider.as_ref(), &op, &mut report).await?;
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
            let objects: Vec<IncomingObject> = changes
                .upserted
                .into_iter()
                .map(|o| {
                    let content = parse(&o.ical)
                        .map_err(|e| e.to_string())
                        .and_then(|cal| read_events(&cal).map_err(|e| e.to_string()))
                        .map(|events| events.into_content());
                    IncomingObject {
                        href: o.href,
                        etag: o.etag,
                        ical: o.ical,
                        content,
                    }
                })
                .collect();
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
            OpKind::Put => {
                let base = if obj.base_ical.trim().is_empty() {
                    None
                } else {
                    parse(&obj.base_ical).ok()
                };
                let ical = write_events(
                    base.as_ref(),
                    &obj.uid,
                    obj.content.master.as_ref(),
                    &obj.content.overrides,
                    Utc::now(),
                )
                .to_ics();
                let precondition = match (&obj.etag, op.force) {
                    (_, true) => Precondition::Overwrite,
                    (Some(etag), false) => Precondition::Version(etag.clone()),
                    (None, false) => Precondition::Create,
                };
                match provider.put_object(&op.href, &ical, &precondition).await {
                    Ok(w) => {
                        self.store
                            .complete_put(op, w.etag.as_deref(), &ical)
                            .await?;
                        Ok(())
                    }
                    Err(e) => Err(e),
                }
            }
            OpKind::Delete => {
                let version = if op.force { None } else { obj.etag.as_deref() };
                match provider.delete_object(&op.href, version).await {
                    Ok(()) => {
                        self.store.complete_delete(op).await?;
                        Ok(())
                    }
                    Err(e) => Err(e),
                }
            }
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
                self.store
                    .fail_operation(op, &e.to_string(), OpFailure::Permanent)
                    .await?;
            }
        }
        Ok(())
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
    }
}
