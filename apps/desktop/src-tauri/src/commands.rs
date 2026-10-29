//! Tauri commands: argument decoding, one call into `calendar-storage`, and
//! mapping errors to user-facing messages. No calendar logic lives here.

use calendar_core::sim::{self, DatasetOptions};
use calendar_core::wire::{WireDraft, WireEvent, WireRange};
use calendar_core::{Account, Calendar, CoreError, parse_time_zone};
use calendar_storage::{StorageError, Store};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use tauri::State;

/// Database handle, or the reason it could not be opened (shown in the UI).
pub struct AppState(pub Result<Store, String>);

impl AppState {
    fn store(&self) -> Result<&Store, CommandError> {
        self.0.as_ref().map_err(|reason| CommandError {
            code: "storage-unavailable",
            message: format!("The calendar database could not be opened: {reason}"),
        })
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
            StorageError::Unsupported(_) => "unsupported",
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
    Ok(WireEvent::from_occurrence(&occurrence, &tz))
}

#[tauri::command]
pub async fn delete_event(
    state: State<'_, AppState>,
    id: String,
    scope: Scope,
) -> CommandResult<()> {
    let store = state.store()?;
    match scope {
        Scope::Event => store.delete_event(&id).await?,
        Scope::Occurrence => store.delete_occurrence(&id).await?,
        Scope::Series => store.delete_series(&id).await?,
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
