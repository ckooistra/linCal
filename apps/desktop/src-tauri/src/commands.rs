//! Tauri commands: argument decoding, one call into `calendar-storage`, and
//! mapping errors to user-facing messages. No calendar logic lives here.

use calendar_core::sim::{self, DatasetOptions};
use calendar_core::wire::{WireDraft, WireEvent, WireRange};
use calendar_core::{Account, Calendar, CoreError, parse_time_zone};
use std::sync::Arc;

use calendar_storage::{AccountStatus, StorageError, Store};
use calendar_sync::{
    Resolution, SchedulerHandle, SyncEngine, SyncError, SyncProblem, user_message,
};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use tauri::State;

pub struct Sync {
    pub engine: Arc<SyncEngine>,
    pub scheduler: SchedulerHandle,
    /// Cancels the account setup in progress, if any.
    pub adding: std::sync::Mutex<Option<Arc<calendar_sync::CancelSignal>>>,
}

/// Database handle (or why it could not be opened) and the sync engine.
pub struct AppState {
    pub store: Result<Store, String>,
    pub sync: Option<Sync>,
}

impl AppState {
    fn store(&self) -> Result<&Store, CommandError> {
        self.store.as_ref().map_err(|reason| CommandError {
            code: "storage-unavailable",
            message: format!("The calendar database could not be opened: {reason}"),
        })
    }

    fn sync(&self) -> Result<&Sync, CommandError> {
        self.sync.as_ref().ok_or_else(|| CommandError {
            code: "storage-unavailable",
            message: "Synchronization is unavailable because the database could not be opened."
                .into(),
        })
    }

    /// Ask for an upload soon if a change touched a synchronized calendar.
    fn changed(&self, status: calendar_core::SyncStatus) {
        if status == calendar_core::SyncStatus::PendingSync
            && let Some(sync) = &self.sync
        {
            sync.scheduler.trigger();
        }
    }
}

impl From<SyncError> for CommandError {
    fn from(error: SyncError) -> Self {
        let code = match &error {
            SyncError::Provider(e) => match e {
                calendar_provider_api::ProviderError::Unauthorized => "unauthorized",
                calendar_provider_api::ProviderError::Network(_) => "offline",
                _ => "provider",
            },
            SyncError::Storage(_) => "storage",
            SyncError::Credentials(_) => "credentials",
            SyncError::Config(_) => "invalid",
            SyncError::TimedOut => "offline",
            SyncError::Task => "internal",
            SyncError::Cancelled => "cancelled",
            SyncError::SignIn(calendar_oauth_error) => match calendar_oauth_error {
                calendar_sync::OAuthError::Cancelled => "cancelled",
                _ => "sign-in",
            },
        };
        Self {
            code,
            message: user_message(&error),
        }
    }
}

/// Error returned to the frontend: a stable code plus a readable message.
#[derive(Debug, Serialize)]
pub struct CommandError {
    code: &'static str,
    message: String,
}

impl From<StorageError> for CommandError {
    fn from(error: StorageError) -> Self {
        let code = match &error {
            StorageError::Invalid(_) => "invalid",
            StorageError::NotFound(_) => "not-found",
            StorageError::ReadOnly(_) => "read-only",
            StorageError::UnknownCalendar(_) => "unknown-calendar",
            StorageError::AlreadyExists(_) => "already-exists",
            StorageError::Unsupported(_) | StorageError::UnsupportedRecurrence(_) => "unsupported",
            StorageError::Database(_)
            | StorageError::Migration(_)
            | StorageError::Corrupt(_)
            | StorageError::Task => "storage",
        };
        let message = match &error {
            StorageError::NotFound(_) => "The event no longer exists.".to_owned(),
            StorageError::Database(_) | StorageError::Task => {
                // Technical detail goes to the log, not the UI.
                eprintln!("[storage] {error}");
                "The change could not be saved to the local database.".to_owned()
            }
            _ => capitalize(&error.to_string()),
        };
        Self { code, message }
    }
}

impl From<CoreError> for CommandError {
    fn from(error: CoreError) -> Self {
        StorageError::Invalid(error).into()
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect::<String>() + ".",
        None => String::new(),
    }
}

fn zone(name: &str) -> Result<Tz, CommandError> {
    Ok(parse_time_zone(name)?)
}

type CommandResult<T> = Result<T, CommandError>;

/// Which stored thing an edit or delete applies to.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// A one-off event (or an override row addressed by its id).
    Event,
    /// One occurrence of a series.
    Occurrence,
    /// The whole series the occurrence belongs to.
    Series,
}

#[tauri::command]
pub async fn list_accounts(state: State<'_, AppState>) -> CommandResult<Vec<Account>> {
    Ok(state.store()?.list_accounts().await?)
}

#[tauri::command]
pub async fn list_calendars(state: State<'_, AppState>) -> CommandResult<Vec<Calendar>> {
    Ok(state.store()?.list_calendars().await?)
}

#[tauri::command]
pub async fn query_range(
    state: State<'_, AppState>,
    range: WireRange,
    time_zone: String,
) -> CommandResult<Vec<WireEvent>> {
    let tz = zone(&time_zone)?;
    let occurrences = state.store()?.query_range(range.to_range()?, tz).await?;
    Ok(occurrences
        .iter()
        .map(|o| WireEvent::from_occurrence(o, &tz))
        .collect())
}

/// Most search results returned per call.
const MAX_SEARCH_RESULTS: usize = 500;

#[tauri::command]
pub async fn search_events(
    state: State<'_, AppState>,
    text: String,
    range: WireRange,
    time_zone: String,
    limit: usize,
    latest: bool,
) -> CommandResult<Vec<WireEvent>> {
    let tz = zone(&time_zone)?;
    let found = state
        .store()?
        .search(
            &text,
            range.to_range()?,
            tz,
            limit.min(MAX_SEARCH_RESULTS),
            latest,
        )
        .await?;
    Ok(found
        .iter()
        .map(|o| WireEvent::from_occurrence(o, &tz))
        .collect())
}

#[tauri::command]
pub async fn create_event(
    state: State<'_, AppState>,
    id: String,
    draft: WireDraft,
    time_zone: String,
) -> CommandResult<WireEvent> {
    let tz = zone(&time_zone)?;
    let event = state
        .store()?
        .create_event(&id, draft.to_draft(&tz)?)
        .await?;
    state.changed(event.sync_status);
    Ok(WireEvent::from_occurrence(&event.as_occurrence(), &tz))
}

#[tauri::command]
pub async fn update_event(
    state: State<'_, AppState>,
    id: String,
    draft: WireDraft,
    scope: Scope,
    time_zone: String,
) -> CommandResult<WireEvent> {
    let tz = zone(&time_zone)?;
    let store = state.store()?;
    let draft = draft.to_draft(&tz)?;
    let occurrence = match scope {
        Scope::Event => store.update_event(&id, draft).await?.as_occurrence(),
        Scope::Occurrence => store.update_occurrence(&id, draft, tz).await?,
        Scope::Series => store.update_series(&id, draft).await?.as_occurrence(),
    };
    state.changed(occurrence.sync_status);
    Ok(WireEvent::from_occurrence(&occurrence, &tz))
}

#[tauri::command]
pub async fn delete_event(
    state: State<'_, AppState>,
    id: String,
    scope: Scope,
) -> CommandResult<()> {
    let store = state.store()?;
    let remote = store.object_of(&id).await?.is_some();
    match scope {
        Scope::Event => store.delete_event(&id).await?,
        Scope::Occurrence => store.delete_occurrence(&id).await?,
        Scope::Series => store.delete_series(&id).await?,
    }
    if remote {
        state.changed(calendar_core::SyncStatus::PendingSync);
    }
    Ok(())
}

#[tauri::command]
pub async fn set_calendar_color(
    state: State<'_, AppState>,
    calendar_id: String,
    color: String,
) -> CommandResult<()> {
    Ok(state
        .store()?
        .set_calendar_color(&calendar_id, &color)
        .await?)
}

#[tauri::command]
pub async fn get_settings(
    state: State<'_, AppState>,
) -> CommandResult<serde_json::Map<String, serde_json::Value>> {
    Ok(state.store()?.settings().await?)
}

#[tauri::command]
pub async fn set_setting(
    state: State<'_, AppState>,
    key: String,
    value: serde_json::Value,
) -> CommandResult<()> {
    Ok(state.store()?.set_setting(&key, &value).await?)
}

/// Replaces all data with a deterministic simulated dataset centred on today.
#[tauri::command]
pub async fn load_simulated_dataset(
    state: State<'_, AppState>,
    size: usize,
    time_zone: String,
    anchor: Option<String>,
) -> CommandResult<()> {
    if !sim::DATASET_SIZES.contains(&size) {
        return Err(CommandError {
            code: "invalid",
            message: "Unsupported dataset size.".into(),
        });
    }
    let tz = zone(&time_zone)?;
    let anchor = match anchor {
        Some(a) => chrono::NaiveDate::parse_from_str(&a, "%Y-%m-%d")
            .map_err(|_| CoreError::InvalidDate(a))?,
        None => chrono::Utc::now().with_timezone(&tz).date_naive(),
    };
    let data = tauri::async_runtime::spawn_blocking(move || {
        sim::generate(&DatasetOptions {
            seed: sim::DEFAULT_SEED,
            event_count: size,
            anchor,
            time_zone: tz,
        })
    })
    .await
    .map_err(|_| StorageError::Task)?;
    Ok(state
        .store()?
        .replace_all(&data.accounts, &data.calendars, &data.events)
        .await?)
}

/// Connects a CalDAV account (discovery + calendar list) and starts syncing.
/// The password goes to the system keyring, never to the database.
#[tauri::command]
pub async fn add_caldav_account(
    state: State<'_, AppState>,
    server_url: String,
    username: String,
    password: String,
) -> CommandResult<String> {
    let sync = state.sync()?;
    if username.trim().is_empty() || password.is_empty() {
        return Err(CommandError {
            code: "invalid",
            message: "Enter a user name and password.".into(),
        });
    }
    let cancel = Arc::new(calendar_sync::CancelSignal::new());
    *sync.adding.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::clone(&cancel));
    let result = sync
        .engine
        .add_caldav_account_cancellable(&server_url, username.trim(), &password, &cancel)
        .await;
    sync.adding.lock().unwrap_or_else(|e| e.into_inner()).take();
    let id = result?;
    sync.scheduler.trigger();
    Ok(id)
}

/// OAuth client ids built into this binary by its distributor
/// (`UC_GOOGLE_CLIENT_ID` / `UC_MICROSOFT_CLIENT_ID` at build time), if any.
/// Client ids are not secrets; no client secret is ever built in.
const BUILT_IN_GOOGLE_CLIENT_ID: Option<&str> = option_env!("UC_GOOGLE_CLIENT_ID");
const BUILT_IN_MICROSOFT_CLIENT_ID: Option<&str> = option_env!("UC_MICROSOFT_CLIENT_ID");

/// The built-in client id for "google" or "microsoft", if any.
#[tauri::command]
pub fn oauth_client_id(provider: String) -> Option<String> {
    match provider.as_str() {
        "google" => BUILT_IN_GOOGLE_CLIENT_ID,
        "microsoft" => BUILT_IN_MICROSOFT_CLIENT_ID,
        _ => None,
    }
    .filter(|id| !id.is_empty())
    .map(str::to_owned)
}

fn browser(app: &tauri::AppHandle) -> calendar_sync::BrowserOpener {
    let app = app.clone();
    Arc::new(move |url: &str| {
        use tauri_plugin_opener::OpenerExt;
        app.opener()
            .open_url(url, None::<&str>)
            .map_err(|e| e.to_string())
    })
}

/// Starts a cancellable account operation (one at a time).
fn begin_adding(sync: &Sync) -> Arc<calendar_sync::CancelSignal> {
    let cancel = Arc::new(calendar_sync::CancelSignal::new());
    *sync.adding.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::clone(&cancel));
    cancel
}

/// Signs in to Google in the system browser and adds the account.
#[tauri::command]
pub async fn add_google_account(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    client_id: String,
    client_secret: Option<String>,
) -> CommandResult<String> {
    let sync = state.sync()?;
    let cancel = begin_adding(sync);
    let result = sync
        .engine
        .add_google_account(&client_id, client_secret.as_deref(), browser(&app), &cancel)
        .await;
    sync.adding.lock().unwrap_or_else(|e| e.into_inner()).take();
    let id = result?;
    sync.scheduler.trigger();
    Ok(id)
}

/// Signs in to Microsoft in the system browser and adds the account.
#[tauri::command]
pub async fn add_microsoft_account(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    client_id: String,
) -> CommandResult<String> {
    let sync = state.sync()?;
    let cancel = begin_adding(sync);
    let result = sync
        .engine
        .add_microsoft_account(&client_id, browser(&app), &cancel)
        .await;
    sync.adding.lock().unwrap_or_else(|e| e.into_inner()).take();
    let id = result?;
    sync.scheduler.trigger();
    Ok(id)
}

/// Renews an expired or revoked OAuth sign-in.
#[tauri::command]
pub async fn sign_in_again(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    account_id: String,
) -> CommandResult<()> {
    let sync = state.sync()?;
    let cancel = begin_adding(sync);
    let result = sync
        .engine
        .sign_in_again(&account_id, browser(&app), &cancel)
        .await;
    sync.adding.lock().unwrap_or_else(|e| e.into_inner()).take();
    result?;
    sync.scheduler.trigger();
    Ok(())
}

/// Stops an account setup that is still connecting.
#[tauri::command]
pub async fn cancel_add_account(state: State<'_, AppState>) -> CommandResult<()> {
    if let Some(cancel) = state
        .sync()?
        .adding
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
    {
        cancel.notify_one();
    }
    Ok(())
}

#[tauri::command]
pub async fn remove_account(state: State<'_, AppState>, account_id: String) -> CommandResult<()> {
    Ok(state.sync()?.engine.remove_account(&account_id).await?)
}

/// Syncs all accounts now, retrying waiting uploads without backoff.
#[tauri::command]
pub async fn sync_now(state: State<'_, AppState>) -> CommandResult<()> {
    state.store()?.retry_now().await?;
    state.sync()?.scheduler.trigger();
    Ok(())
}

#[tauri::command]
pub async fn account_statuses(state: State<'_, AppState>) -> CommandResult<Vec<AccountStatus>> {
    Ok(state.store()?.account_statuses().await?)
}

/// Resolves a sync conflict or failure for the object an event belongs to
/// (from the event editor).
#[tauri::command]
pub async fn resolve_conflict(
    state: State<'_, AppState>,
    event_id: String,
    resolution: Resolution,
) -> CommandResult<()> {
    let store = state.store()?;
    let (calendar_id, href) = store
        .object_of(&event_id)
        .await?
        .ok_or_else(|| CommandError {
            code: "not-found",
            message: "This event is not synchronized.".into(),
        })?;
    store
        .resolve_operation(&calendar_id, &href, resolution)
        .await?;
    state.sync()?.scheduler.trigger();
    Ok(())
}

/// Failed and conflicting changes, including deletions (which have no
/// event left to open).
#[tauri::command]
pub async fn sync_problems(state: State<'_, AppState>) -> CommandResult<Vec<SyncProblem>> {
    Ok(state.sync()?.engine.problems().await?)
}

#[tauri::command]
pub async fn resolve_sync_problem(
    state: State<'_, AppState>,
    calendar_id: String,
    href: String,
    resolution: Resolution,
) -> CommandResult<()> {
    let sync = state.sync()?;
    sync.engine.resolve(&calendar_id, &href, resolution).await?;
    sync.scheduler.trigger();
    Ok(())
}
