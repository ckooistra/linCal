//! Storage side of synchronization: remote accounts and calendars, remote
//! object metadata, the durable operation queue, and applying server
//! changes. Provider- and protocol-agnostic: callers pass parsed content.

use serde_json::Value;
use sqlx::{Row, Sqlite, Transaction};

use calendar_core::{
    Account, Calendar, CalendarEvent, EventContent, ObjectContent, OccurrenceKey, ProviderKind,
    SeriesLink, SyncStatus, occurrence_id, parse_occurrence_id,
};

use crate::codec::{EventRow, decode_event, event_columns};
use crate::{Result, StorageError, Store, now, now_ms};

/// A calendar as reported by a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteCalendarSpec {
    pub remote_url: String,
    pub name: String,
    pub color: Option<String>,
    pub read_only: bool,
    pub supports_sync: bool,
    pub ctag: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RemoteAccount {
    pub account: Account,
    pub config: Value,
    pub sync_error: Option<String>,
    pub last_synced_at: Option<i64>,
}

/// Sync state of one remote calendar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteCalendarState {
    pub calendar_id: String,
    pub remote_url: String,
    pub name: String,
    pub read_only: bool,
    pub supports_sync: bool,
    pub sync_token: Option<String>,
    pub ctag: Option<String>,
    /// Provider reader version the events were last read with.
    pub reader_version: i64,
}

/// A remote object received from the server.
#[derive(Debug, Clone)]
pub struct IncomingObject {
    pub href: String,
    pub etag: Option<String>,
    pub ical: String,
    /// Parsed content, or why it could not be parsed.
    pub content: std::result::Result<ObjectContent, String>,
    /// What the parsed content only approximates.
    pub warnings: Vec<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ApplyReport {
    pub applied: usize,
    pub deleted: usize,
    /// Hrefs left alone because a local change is waiting to be uploaded.
    pub skipped_pending: Vec<String>,
    /// Objects that could not be imported, with the reason.
    pub failed: Vec<(String, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    Put,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingOp {
    pub calendar_id: String,
    pub href: String,
    pub kind: OpKind,
    pub generation: i64,
    pub attempts: i64,
    /// Upload unconditionally (the user chose to keep their version).
    pub force: bool,
}

/// Whether the server may hold an object (see migration 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    /// Created locally and never sent.
    Local,
    /// A create was sent; whether it arrived is unknown.
    Sent,
    /// Exists on the server.
    Present,
}

impl Presence {
    fn parse(value: &str) -> Self {
        match value {
            "local" => Self::Local,
            "sent" => Self::Sent,
            _ => Self::Present,
        }
    }
}

/// What to upload for a `put` operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushObject {
    pub uid: String,
    /// Version last seen on the server (`None` = never seen, or unknown).
    pub etag: Option<String>,
    pub presence: Presence,
    /// Last server copy, as the base for editing ("" if created locally).
    pub base_ical: String,
    pub content: ObjectContent,
    /// Overrides whose recurrence id moved locally: (current, in base copy).
    pub renamed: Vec<(OccurrenceKey, OccurrenceKey)>,
}

/// How the user resolves a failed or conflicting operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Resolution {
    /// Try the same conditional upload again (after a permanent failure).
    Retry,
    /// Upload the local version even though the server copy changed.
    KeepLocal,
    /// Drop the local change and take whatever the server has.
    UseServer,
}

/// A queued operation that needs the user's attention.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProblemOperation {
    pub calendar_id: String,
    pub href: String,
    pub kind: OpKind,
    /// True for a version conflict, false for a permanent failure.
    pub conflict: bool,
    pub error: Option<String>,
    /// Title of the local rows, if any are left (deletions have none).
    pub title: Option<String>,
    /// Last server copy, for describing deleted objects.
    pub base_ical: String,
}

/// Outcome of a failed operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpFailure {
    /// Try again at this time (epoch ms).
    Retry { at_ms: i64 },
    /// Give up until the user intervenes.
    Permanent,
    /// The server copy changed; the user must choose a version.
    Conflict,
}

/// Per-account status for the UI.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountStatus {
    pub account_id: String,
    pub remote: bool,
    pub last_synced_at: Option<i64>,
    pub error: Option<String>,
    pub pending: i64,
    pub failed: i64,
    pub conflicts: i64,
    /// Server objects whose newest version could not be imported.
    pub import_failures: i64,
    /// Imported objects shown only approximately (see `import_warnings`).
    pub approximated: i64,
    /// An OAuth sign-in expired or was revoked.
    pub needs_sign_in: bool,
}

fn segment(url: &str) -> &str {
    url.trim_end_matches('/').rsplit('/').next().unwrap_or(url)
}

/// Stable row id for an object pulled from the server.
fn pulled_id(calendar_id: &str, href: &str) -> String {
    let tail = segment(href);
    let tail = tail.strip_suffix(".ics").unwrap_or(tail);
    format!("{calendar_id}/{tail}").replace(calendar_core::model::OCCURRENCE_SEPARATOR, ":_:")
}

/// 64-bit FNV-1a: a stable, dependency-free hash for disambiguating ids.
fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Href for an object created locally in a remote calendar.
fn new_href(calendar_url: &str, uid: &str) -> String {
    let safe: String = uid
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let base = if calendar_url.ends_with('/') {
        calendar_url.to_owned()
    } else {
        format!("{calendar_url}/")
    };
    format!("{base}{safe}.ics")
}

impl Store {
    // --- hooks used by local mutations --------------------------------------

    /// The remote URL of a calendar, if it is synchronized.
    pub(crate) async fn calendar_remote(
        tx: &mut Transaction<'_, Sqlite>,
        calendar_id: &str,
    ) -> Result<Option<String>> {
        let row = sqlx::query("SELECT remote_url FROM calendars WHERE id = ?")
            .bind(calendar_id)
            .fetch_optional(&mut **tx)
            .await?;
        Ok(row.and_then(|r| r.get::<Option<String>, _>("remote_url")))
    }

    pub(crate) async fn event_href(
        tx: &mut Transaction<'_, Sqlite>,
        event_id: &str,
    ) -> Result<Option<String>> {
        let row = sqlx::query("SELECT remote_href FROM events WHERE id = ?")
            .bind(event_id)
            .fetch_optional(&mut **tx)
            .await?;
        Ok(row.and_then(|r| r.get::<Option<String>, _>("remote_href")))
    }

    /// For an event created in a remote calendar: registers its new object.
    pub(crate) async fn attach_new_object(
        tx: &mut Transaction<'_, Sqlite>,
        calendar_id: &str,
        calendar_url: &str,
        event_id: &str,
    ) -> Result<()> {
        let href = new_href(calendar_url, event_id);
        sqlx::query("INSERT INTO remote_objects (calendar_id, href, uid, etag, ical, presence) VALUES (?, ?, ?, NULL, '', 'local')")
            .bind(calendar_id)
            .bind(&href)
            .bind(event_id)
            .execute(&mut **tx)
            .await?;
        Self::set_href(tx, event_id, &href).await?;
        Self::touch_object(tx, calendar_id, &href).await?;
        Ok(())
    }

    pub(crate) async fn set_href(
        tx: &mut Transaction<'_, Sqlite>,
        event_id: &str,
        href: &str,
    ) -> Result<()> {
        sqlx::query("UPDATE events SET remote_href = ? WHERE id = ? OR series_id = ?")
            .bind(href)
            .bind(event_id)
            .bind(event_id)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    /// Records a local change of an object: queues an upload and marks its
    /// rows as waiting for sync. Repeated changes coalesce into one upload.
    /// An unresolved conflict stays a conflict (the user still has to
    /// choose); returns the status the rows now have.
    pub(crate) async fn touch_object(
        tx: &mut Transaction<'_, Sqlite>,
        calendar_id: &str,
        href: &str,
    ) -> Result<SyncStatus> {
        sqlx::query(
            "INSERT INTO pending_operations (calendar_id, href, kind, created_at) VALUES (?, ?, 'put', ?) \
             ON CONFLICT(calendar_id, href) DO UPDATE SET kind = 'put', state = CASE WHEN state = 'conflict' \
             THEN 'conflict' ELSE 'pending' END, generation = generation + 1, attempts = 0, next_attempt_ms = 0, \
             last_error = NULL",
        )
        .bind(calendar_id)
        .bind(href)
        .bind(now_ms())
        .execute(&mut **tx)
        .await?;
        let state: String = sqlx::query_scalar(
            "SELECT state FROM pending_operations WHERE calendar_id = ? AND href = ?",
        )
        .bind(calendar_id)
        .bind(href)
        .fetch_one(&mut **tx)
        .await?;
        let status = if state == "conflict" {
            SyncStatus::Conflict
        } else {
            SyncStatus::PendingSync
        };
        sqlx::query("UPDATE events SET sync_status = ? WHERE calendar_id = ? AND remote_href = ?")
            .bind(status.as_str())
            .bind(calendar_id)
            .bind(href)
            .execute(&mut **tx)
            .await?;
        Ok(status)
    }

    /// After rows of an object were deleted locally: upload the remainder,
    /// or delete the object remotely if nothing is left.
    pub(crate) async fn object_changed_or_removed(
        tx: &mut Transaction<'_, Sqlite>,
        calendar_id: &str,
        href: &str,
    ) -> Result<()> {
        let left: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM events WHERE calendar_id = ? AND remote_href = ?",
        )
        .bind(calendar_id)
        .bind(href)
        .fetch_one(&mut **tx)
        .await?;
        if left > 0 {
            Self::touch_object(tx, calendar_id, href).await?;
            return Ok(());
        }
        let presence: Option<String> = sqlx::query_scalar(
            "SELECT presence FROM remote_objects WHERE calendar_id = ? AND href = ?",
        )
        .bind(calendar_id)
        .bind(href)
        .fetch_optional(&mut **tx)
        .await?;
        if matches!(presence.as_deref(), Some("local") | None) {
            // Never sent (an upload marks the object 'sent' before it
            // starts, in its own transaction): forget it entirely.
            for sql in [
                "DELETE FROM remote_objects WHERE calendar_id = ? AND href = ?",
                "DELETE FROM pending_operations WHERE calendar_id = ? AND href = ?",
            ] {
                sqlx::query(sql)
                    .bind(calendar_id)
                    .bind(href)
                    .execute(&mut **tx)
                    .await?;
            }
            return Ok(());
        }
        sqlx::query(
            "INSERT INTO pending_operations (calendar_id, href, kind, created_at) VALUES (?, ?, 'delete', ?) \
             ON CONFLICT(calendar_id, href) DO UPDATE SET kind = 'delete', state = 'pending', \
             generation = generation + 1, attempts = 0, next_attempt_ms = 0, last_error = NULL",
        )
        .bind(calendar_id)
        .bind(href)
        .bind(now_ms())
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    /// Runs the one-time fix-ups queued by migrations (`pending_fixups`).
    pub(crate) async fn run_fixups(&self) -> Result<()> {
        let pending: Vec<String> = sqlx::query_scalar("SELECT name FROM pending_fixups")
            .fetch_all(&self.pool)
            .await?;
        for name in pending {
            let mut tx = self.pool.begin().await?;
            if name == "lone-instances" {
                // Before migration 4, lone instances were recognised by an
                // id of the form `<object>@<recurrence id>`.
                let rows = sqlx::query(
                    "SELECT id, calendar_id, remote_href FROM events WHERE remote_href IS NOT NULL \
                     AND series_id IS NULL AND rrule IS NULL AND instr(id, '@') > 0",
                )
                .fetch_all(&mut *tx)
                .await?;
                for row in rows {
                    let id: String = row.get("id");
                    let own_id = pulled_id(
                        &row.get::<String, _>("calendar_id"),
                        &row.get::<String, _>("remote_href"),
                    );
                    // An event whose file name contains '@' has exactly its
                    // object's id; a lone instance has `<that id>@<key>`.
                    let lone = id
                        .rsplit_once('@')
                        .filter(|(object, _)| id != own_id && object.starts_with(own_id.as_str()))
                        .and_then(|(_, k)| k.parse::<OccurrenceKey>().ok());
                    if let Some(key) = lone {
                        sqlx::query("UPDATE events SET lone_recurrence_id = ? WHERE id = ?")
                            .bind(key.to_string())
                            .bind(&id)
                            .execute(&mut *tx)
                            .await?;
                    }
                }
            }
            sqlx::query("DELETE FROM pending_fixups WHERE name = ?")
                .bind(&name)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
        }
        Ok(())
    }

    // --- accounts and calendars ---------------------------------------------

    /// Adds a remote account and its calendars. Calendars are ordered after
    /// existing ones.
    pub async fn create_remote_account(
        &self,
        account: &Account,
        config: &Value,
        calendars: &[RemoteCalendarSpec],
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO accounts (id, name, provider, created_at, config) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&account.id)
        .bind(&account.name)
        .bind(account.provider.as_str())
        .bind(now_ms())
        .bind(config.to_string())
        .execute(&mut *tx)
        .await
        .map_err(|e| crate::map_unique(e, &account.id))?;
        for spec in calendars {
            Self::insert_remote_calendar(&mut tx, &account.id, spec).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn insert_remote_calendar(
        tx: &mut Transaction<'_, Sqlite>,
        account_id: &str,
        spec: &RemoteCalendarSpec,
    ) -> Result<()> {
        let order: i64 =
            sqlx::query_scalar("SELECT COALESCE(MAX(sort_order), 0) + 1 FROM calendars")
                .fetch_one(&mut **tx)
                .await?;
        let mut id = format!("{account_id}/{}", segment(&spec.remote_url));
        let taken: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM calendars WHERE id = ?")
            .bind(&id)
            .fetch_one(&mut **tx)
            .await?;
        if taken > 0 {
            // Another collection of the account ends in the same segment.
            id = format!("{id}~{:016x}", fnv1a(&spec.remote_url));
        }
        sqlx::query(
            "INSERT INTO calendars (id, account_id, name, color, read_only, sort_order, remote_url, ctag, supports_sync) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(account_id)
        .bind(&spec.name)
        .bind(spec.color.as_deref().unwrap_or("#5c7cfa"))
        .bind(i64::from(spec.read_only))
        .bind(order)
        .bind(&spec.remote_url)
        .bind(&spec.ctag)
        .bind(i64::from(spec.supports_sync))
        .execute(&mut **tx)
        .await
        .map_err(|e| crate::map_unique(e, &id))?;
        Ok(())
    }

    /// Reconciles the local calendar list with the server's. New calendars
    /// are added; names and permissions follow the server (the colour stays
    /// local once set). Calendars gone from the server are removed unless
    /// they hold unsynced changes. Returns true if anything changed.
    pub async fn update_remote_calendars(
        &self,
        account_id: &str,
        specs: &[RemoteCalendarSpec],
    ) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let existing = sqlx::query("SELECT id, remote_url, name, read_only, supports_sync FROM calendars WHERE account_id = ?")
            .bind(account_id)
            .fetch_all(&mut *tx)
            .await?;
        let mut changed = false;
        for spec in specs {
            match existing.iter().find(|r| {
                r.get::<Option<String>, _>("remote_url").as_deref() == Some(&spec.remote_url)
            }) {
                Some(row) => {
                    let differs = row.get::<String, _>("name") != spec.name
                        || (row.get::<i64, _>("read_only") != 0) != spec.read_only
                        || (row.get::<i64, _>("supports_sync") != 0) != spec.supports_sync;
                    if differs {
                        changed = true;
                        sqlx::query("UPDATE calendars SET name = ?, read_only = ?, supports_sync = ? WHERE id = ?")
                            .bind(&spec.name)
                            .bind(i64::from(spec.read_only))
                            .bind(i64::from(spec.supports_sync))
                            .bind(row.get::<String, _>("id"))
                            .execute(&mut *tx)
                            .await?;
                    }
                }
                None => {
                    changed = true;
                    Self::insert_remote_calendar(&mut tx, account_id, spec).await?;
                }
            }
        }
        for row in &existing {
            let url: Option<String> = row.get("remote_url");
            if specs.iter().any(|s| Some(&s.remote_url) == url.as_ref()) {
                continue;
            }
            let id: String = row.get("id");
            let pending: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM pending_operations WHERE calendar_id = ?")
                    .bind(&id)
                    .fetch_one(&mut *tx)
                    .await?;
            if pending == 0 {
                changed = true;
                sqlx::query("DELETE FROM calendars WHERE id = ?")
                    .bind(&id)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        tx.commit().await?;
        Ok(changed)
    }

    /// Replaces a remote account's non-secret settings.
    pub async fn set_account_config(&self, account_id: &str, config: &Value) -> Result<()> {
        sqlx::query("UPDATE accounts SET config = ? WHERE id = ?")
            .bind(config.to_string())
            .bind(account_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn remote_accounts(&self) -> Result<Vec<RemoteAccount>> {
        let rows = sqlx::query(
            "SELECT id, name, provider, config, sync_error, last_synced_at FROM accounts \
             WHERE provider NOT IN ('local', 'simulated') ORDER BY created_at",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                let provider: String = r.get("provider");
                Ok(RemoteAccount {
                    account: Account {
                        id: r.get("id"),
                        name: r.get("name"),
                        provider: ProviderKind::parse(&provider).ok_or_else(|| {
                            StorageError::Corrupt(format!("provider: {provider}"))
                        })?,
                    },
                    config: serde_json::from_str(&r.get::<String, _>("config"))
                        .unwrap_or(Value::Null),
                    sync_error: r.get("sync_error"),
                    last_synced_at: r.get("last_synced_at"),
                })
            })
            .collect()
    }

    pub async fn remote_calendars(&self, account_id: &str) -> Result<Vec<RemoteCalendarState>> {
        let rows = sqlx::query(
            "SELECT id, remote_url, name, read_only, supports_sync, sync_token, ctag, reader_version FROM calendars \
             WHERE account_id = ? AND remote_url IS NOT NULL ORDER BY sort_order",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| RemoteCalendarState {
                calendar_id: r.get("id"),
                remote_url: r.get("remote_url"),
                name: r.get("name"),
                read_only: r.get::<i64, _>("read_only") != 0,
                supports_sync: r.get::<i64, _>("supports_sync") != 0,
                sync_token: r.get("sync_token"),
                ctag: r.get("ctag"),
                reader_version: r.get("reader_version"),
            })
            .collect())
    }

    /// Records that a calendar's objects were all read with `version`.
    pub async fn set_reader_version(&self, calendar_id: &str, version: i64) -> Result<()> {
        sqlx::query("UPDATE calendars SET reader_version = ? WHERE id = ?")
            .bind(version)
            .bind(calendar_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_account_status(
        &self,
        account_id: &str,
        error: Option<&str>,
        synced_at: Option<i64>,
        needs_sign_in: bool,
    ) -> Result<()> {
        sqlx::query("UPDATE accounts SET sync_error = ?, last_synced_at = COALESCE(?, last_synced_at), needs_sign_in = ? WHERE id = ?")
            .bind(error)
            .bind(synced_at)
            .bind(needs_sign_in)
            .bind(account_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Removes an account with its calendars, events and queued changes.
    pub async fn remove_account(&self, account_id: &str) -> Result<()> {
        let done = sqlx::query("DELETE FROM accounts WHERE id = ?")
            .bind(account_id)
            .execute(&self.pool)
            .await?;
        if done.rows_affected() == 0 {
            return Err(StorageError::NotFound(account_id.to_owned()));
        }
        Ok(())
    }

    pub async fn account_statuses(&self) -> Result<Vec<AccountStatus>> {
        let rows = sqlx::query(
            "SELECT a.id, a.provider, a.sync_error, a.last_synced_at, a.needs_sign_in, \
               (SELECT COUNT(*) FROM pending_operations p JOIN calendars c ON c.id = p.calendar_id WHERE c.account_id = a.id AND p.state = 'pending') AS pending, \
               (SELECT COUNT(*) FROM pending_operations p JOIN calendars c ON c.id = p.calendar_id WHERE c.account_id = a.id AND p.state = 'failed') AS failed, \
               (SELECT COUNT(*) FROM pending_operations p JOIN calendars c ON c.id = p.calendar_id WHERE c.account_id = a.id AND p.state = 'conflict') AS conflicts, \
               (SELECT COUNT(*) FROM remote_objects o JOIN calendars c ON c.id = o.calendar_id WHERE c.account_id = a.id AND o.import_error IS NOT NULL) AS import_failures, \
               (SELECT COUNT(*) FROM remote_objects o JOIN calendars c ON c.id = o.calendar_id WHERE c.account_id = a.id AND o.import_warnings IS NOT NULL) AS approximated \
             FROM accounts a ORDER BY a.created_at",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                let provider: String = r.get("provider");
                AccountStatus {
                    account_id: r.get("id"),
                    remote: !matches!(provider.as_str(), "local" | "simulated"),
                    last_synced_at: r.get("last_synced_at"),
                    error: r.get("sync_error"),
                    pending: r.get("pending"),
                    failed: r.get("failed"),
                    conflicts: r.get("conflicts"),
                    import_failures: r.get("import_failures"),
                    approximated: r.get("approximated"),
                    needs_sign_in: r.get::<i64, _>("needs_sign_in") != 0,
                }
            })
            .collect())
    }

    // --- pulling -----------------------------------------------------------

    /// Hrefs and ETags of the objects of a calendar the server may hold.
    /// A `None` version means "exists, version unknown": a full listing
    /// fetches it again, or reports it deleted if it is gone.
    pub async fn known_objects(&self, calendar_id: &str) -> Result<Vec<(String, Option<String>)>> {
        let rows = sqlx::query(
            "SELECT href, etag FROM remote_objects WHERE calendar_id = ? AND presence <> 'local'",
        )
        .bind(calendar_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get("href"), r.get("etag")))
            .collect())
    }

    async fn has_pending(
        tx: &mut Transaction<'_, Sqlite>,
        calendar_id: &str,
        href: &str,
    ) -> Result<bool> {
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pending_operations WHERE calendar_id = ? AND href = ?",
        )
        .bind(calendar_id)
        .bind(href)
        .fetch_one(&mut **tx)
        .await?;
        Ok(n > 0)
    }

    async fn insert_object_rows(
        tx: &mut Transaction<'_, Sqlite>,
        calendar_id: &str,
        href: &str,
        master_id: &str,
        content: &ObjectContent,
    ) -> Result<()> {
        let now = now();
        let row = |id: String, e: &EventContent, series: Option<SeriesLink>| CalendarEvent {
            reminders: e.reminders.clone(),
            id,
            calendar_id: calendar_id.to_owned(),
            title: e.title.clone(),
            description: e.description.clone(),
            location: e.location.clone(),
            timing: e.timing.clone(),
            recurrence: e.recurrence.clone(),
            series,
            sync_status: SyncStatus::Synced,
            revision: 1,
            created_at: now,
            updated_at: now,
        };
        // (row id, RECURRENCE-ID of a lone instance)
        let mut ids: Vec<(String, Option<OccurrenceKey>)> = Vec::new();
        if let Some(m) = &content.master {
            Self::insert(tx, &row(master_id.to_owned(), m, None)).await?;
            ids.push((master_id.to_owned(), None));
        }
        for o in &content.overrides {
            let key = o.recurrence_id.unwrap_or_else(|| o.timing.start_key());
            let (event, lone) = match &content.master {
                Some(_) => (
                    row(
                        occurrence_id(master_id, &key),
                        o,
                        Some(SeriesLink {
                            series_id: master_id.to_owned(),
                            recurrence_id: key,
                        }),
                    ),
                    None,
                ),
                // Instances without their series (e.g. a single invitation
                // occurrence) are shown as one-off events.
                None => (row(format!("{master_id}@{key}"), o, None), Some(key)),
            };
            Self::insert(tx, &event).await?;
            ids.push((event.id, lone));
        }
        for (id, lone) in ids {
            sqlx::query("UPDATE events SET remote_href = ?, lone_recurrence_id = ? WHERE id = ?")
                .bind(href)
                .bind(lone.map(|k| k.to_string()))
                .bind(id)
                .execute(&mut **tx)
                .await?;
        }
        Ok(())
    }

    /// Replaces the stored rows of an object with a newly downloaded version.
    async fn replace_object(
        tx: &mut Transaction<'_, Sqlite>,
        calendar_id: &str,
        obj: &IncomingObject,
        content: &ObjectContent,
    ) -> Result<()> {
        let existing: Option<String> = sqlx::query_scalar(
            "SELECT id FROM events WHERE calendar_id = ? AND remote_href = ? AND series_id IS NULL \
             AND lone_recurrence_id IS NULL ORDER BY (rrule IS NULL), id LIMIT 1",
        )
        .bind(calendar_id)
        .bind(&obj.href)
        .fetch_optional(&mut **tx)
        .await?;
        let master_id = match existing {
            Some(id) => id,
            None => Self::new_pulled_id(tx, calendar_id, &obj.href).await?,
        };
        sqlx::query("DELETE FROM events WHERE calendar_id = ? AND remote_href = ?")
            .bind(calendar_id)
            .bind(&obj.href)
            .execute(&mut **tx)
            .await?;
        sqlx::query(
            "INSERT INTO remote_objects (calendar_id, href, uid, etag, ical, presence, import_error, import_warnings) \
             VALUES (?, ?, ?, ?, ?, 'present', NULL, ?) \
             ON CONFLICT(calendar_id, href) DO UPDATE SET uid = excluded.uid, etag = excluded.etag, \
             ical = excluded.ical, presence = 'present', import_error = NULL, \
             import_warnings = excluded.import_warnings",
        )
        .bind(calendar_id)
        .bind(&obj.href)
        .bind(&content.uid)
        .bind(&obj.etag)
        .bind(&obj.ical)
        .bind((!obj.warnings.is_empty()).then(|| obj.warnings.join("\n")))
        .execute(&mut **tx)
        .await?;
        Self::insert_object_rows(tx, calendar_id, &obj.href, &master_id, content).await
    }

    /// Records that the newest version of an object could not be imported.
    /// A previously imported version stays as it was (rows, ETag and base
    /// copy), so edits to it are still checked against the server; an
    /// object never imported is remembered with its version so it is not
    /// downloaded again until it changes.
    async fn quarantine(
        tx: &mut Transaction<'_, Sqlite>,
        calendar_id: &str,
        obj: &IncomingObject,
        reason: &str,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO remote_objects (calendar_id, href, uid, etag, ical, presence, import_error) \
             VALUES (?, ?, '', ?, '', 'present', ?) \
             ON CONFLICT(calendar_id, href) DO UPDATE SET import_error = excluded.import_error",
        )
        .bind(calendar_id)
        .bind(&obj.href)
        .bind(&obj.etag)
        .bind(reason)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    /// A row id for an object seen for the first time. Ids derive from the
    /// href's last segment for readability; distinct hrefs that map to the
    /// same id get a hash of the full href appended.
    async fn new_pulled_id(
        tx: &mut Transaction<'_, Sqlite>,
        calendar_id: &str,
        href: &str,
    ) -> Result<String> {
        let id = pulled_id(calendar_id, href);
        let taken: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM events WHERE (id = ? OR series_id = ?) AND remote_href IS NOT ?",
        )
        .bind(&id)
        .bind(&id)
        .bind(href)
        .fetch_one(&mut **tx)
        .await?;
        Ok(if taken > 0 {
            format!("{id}~{:016x}", fnv1a(href))
        } else {
            id
        })
    }

    /// Applies server changes to one calendar in a single transaction and
    /// stores the new sync cursor. Objects with queued local changes are
    /// left alone (the upload's version check will surface any conflict).
    pub async fn apply_remote_changes(
        &self,
        calendar_id: &str,
        objects: &[IncomingObject],
        deleted: &[String],
        cursor: Option<&str>,
    ) -> Result<ApplyReport> {
        let mut report = ApplyReport::default();
        let mut tx = self.pool.begin().await?;
        for obj in objects {
            if Self::has_pending(&mut tx, calendar_id, &obj.href).await? {
                report.skipped_pending.push(obj.href.clone());
                continue;
            }
            let imported = match &obj.content {
                Ok(content) => {
                    // One bad object must not abort the whole batch, and must
                    // not lose the last version that did import.
                    sqlx::query("SAVEPOINT import_object")
                        .execute(&mut *tx)
                        .await?;
                    match Self::replace_object(&mut tx, calendar_id, obj, content).await {
                        Ok(()) => {
                            sqlx::query("RELEASE import_object")
                                .execute(&mut *tx)
                                .await?;
                            Ok(())
                        }
                        Err(e) => {
                            sqlx::query("ROLLBACK TO import_object")
                                .execute(&mut *tx)
                                .await?;
                            sqlx::query("RELEASE import_object")
                                .execute(&mut *tx)
                                .await?;
                            Err(e.to_string())
                        }
                    }
                }
                Err(reason) => Err(reason.clone()),
            };
            match imported {
                Ok(()) => report.applied += 1,
                Err(reason) => {
                    Self::quarantine(&mut tx, calendar_id, obj, &reason).await?;
                    report.failed.push((obj.href.clone(), reason));
                }
            }
        }
        for href in deleted {
            if Self::has_pending(&mut tx, calendar_id, href).await? {
                report.skipped_pending.push(href.clone());
                continue;
            }
            for sql in [
                "DELETE FROM events WHERE calendar_id = ? AND remote_href = ?",
                "DELETE FROM remote_objects WHERE calendar_id = ? AND href = ?",
            ] {
                sqlx::query(sql)
                    .bind(calendar_id)
                    .bind(href)
                    .execute(&mut *tx)
                    .await?;
            }
            report.deleted += 1;
        }
        if let Some(c) = cursor {
            sqlx::query("UPDATE calendars SET sync_token = ? WHERE id = ?")
                .bind(c)
                .bind(calendar_id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(report)
    }

    // --- pushing -----------------------------------------------------------

    /// Queued operations of an account that are due now.
    pub async fn due_operations(&self, account_id: &str, now_ms: i64) -> Result<Vec<PendingOp>> {
        let rows = sqlx::query(
            "SELECT p.calendar_id, p.href, p.kind, p.generation, p.attempts, p.force FROM pending_operations p \
             JOIN calendars c ON c.id = p.calendar_id \
             WHERE c.account_id = ? AND p.state = 'pending' AND p.next_attempt_ms <= ? ORDER BY p.created_at",
        )
        .bind(account_id)
        .bind(now_ms)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| PendingOp {
                calendar_id: r.get("calendar_id"),
                href: r.get("href"),
                kind: if r.get::<String, _>("kind") == "delete" {
                    OpKind::Delete
                } else {
                    OpKind::Put
                },
                generation: r.get("generation"),
                attempts: r.get("attempts"),
                force: r.get::<i64, _>("force") != 0,
            })
            .collect())
    }

    /// Makes every waiting retry due immediately ("Sync now").
    pub async fn retry_now(&self) -> Result<()> {
        sqlx::query("UPDATE pending_operations SET next_attempt_ms = 0 WHERE state = 'pending'")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Earliest retry time of queued operations for an account, if any.
    pub async fn next_retry_ms(&self, account_id: &str) -> Result<Option<i64>> {
        Ok(sqlx::query_scalar(
            "SELECT MIN(p.next_attempt_ms) FROM pending_operations p JOIN calendars c ON c.id = p.calendar_id \
             WHERE c.account_id = ? AND p.state = 'pending'",
        )
        .bind(account_id)
        .fetch_one(&self.pool)
        .await?)
    }

    /// The current local content of an object, for upload.
    pub async fn object_for_push(
        &self,
        calendar_id: &str,
        href: &str,
    ) -> Result<Option<PushObject>> {
        let obj = sqlx::query(
            "SELECT uid, etag, ical, presence FROM remote_objects WHERE calendar_id = ? AND href = ?",
        )
        .bind(calendar_id)
        .bind(href)
        .fetch_optional(&self.pool)
        .await?;
        let Some(obj) = obj else { return Ok(None) };
        let rows: Vec<EventRow> = sqlx::query_as(concat!(
            "SELECT ",
            event_columns!(),
            " FROM events WHERE calendar_id = ? AND remote_href = ? ORDER BY series_id IS NOT NULL, id"
        ))
        .bind(calendar_id)
        .bind(href)
        .fetch_all(&self.pool)
        .await?;
        let mut lone = std::collections::HashMap::new();
        for r in sqlx::query(
            "SELECT id, lone_recurrence_id FROM events \
             WHERE calendar_id = ? AND remote_href = ? AND lone_recurrence_id IS NOT NULL",
        )
        .bind(calendar_id)
        .bind(href)
        .fetch_all(&self.pool)
        .await?
        {
            let value: String = r.get("lone_recurrence_id");
            let key: OccurrenceKey = value
                .parse()
                .map_err(|_| StorageError::Corrupt(format!("recurrence id: {value}")))?;
            lone.insert(r.get::<String, _>("id"), key);
        }
        let mut master = None;
        let mut overrides = Vec::new();
        for row in rows {
            let event = decode_event(row)?;
            let lone_key = lone.get(&event.id).copied();
            let content = EventContent {
                reminders: event.reminders,
                title: event.title,
                description: event.description,
                location: event.location,
                timing: event.timing,
                recurrence: event.recurrence,
                recurrence_id: event.series.as_ref().map(|s| s.recurrence_id),
            };
            if event.series.is_some() {
                overrides.push(content);
            } else if let Some(key) = lone_key {
                // A lone instance imported without its series.
                overrides.push(EventContent {
                    recurrence_id: Some(key),
                    ..content
                });
            } else {
                master = Some(content);
            }
        }
        let mut renamed = Vec::new();
        for r in sqlx::query(
            "SELECT recurrence_id, base_recurrence_id FROM events \
             WHERE calendar_id = ? AND remote_href = ? AND base_recurrence_id IS NOT NULL",
        )
        .bind(calendar_id)
        .bind(href)
        .fetch_all(&self.pool)
        .await?
        {
            let parse = |column: &str| -> Result<OccurrenceKey> {
                let value: String = r.get(column);
                value
                    .parse()
                    .map_err(|_| StorageError::Corrupt(format!("recurrence id: {value}")))
            };
            renamed.push((parse("recurrence_id")?, parse("base_recurrence_id")?));
        }
        Ok(Some(PushObject {
            uid: obj.get("uid"),
            etag: obj.get("etag"),
            presence: Presence::parse(&obj.get::<String, _>("presence")),
            base_ical: obj.get("ical"),
            content: ObjectContent {
                uid: obj.get("uid"),
                master,
                overrides,
            },
            renamed,
        }))
    }

    /// Called before an object that was never sent is uploaded: from now on
    /// a local deletion must be sent to the server too, whatever happens to
    /// the request. Returns false if the object was deleted meanwhile (then
    /// nothing must be uploaded).
    pub async fn begin_upload(&self, calendar_id: &str, href: &str) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let exists: Option<String> = sqlx::query_scalar(
            "SELECT presence FROM remote_objects WHERE calendar_id = ? AND href = ?",
        )
        .bind(calendar_id)
        .bind(href)
        .fetch_optional(&mut *tx)
        .await?;
        if exists.is_none() {
            return Ok(false);
        }
        sqlx::query(
            "UPDATE remote_objects SET presence = 'sent' WHERE calendar_id = ? AND href = ? AND presence = 'local'",
        )
        .bind(calendar_id)
        .bind(href)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Moves an object to the `href` the provider assigned on create.
    pub async fn rename_object(&self, calendar_id: &str, from: &str, to: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for sql in [
            "UPDATE remote_objects SET href = ? WHERE calendar_id = ? AND href = ?",
            "UPDATE pending_operations SET href = ? WHERE calendar_id = ? AND href = ?",
            "UPDATE events SET remote_href = ? WHERE calendar_id = ? AND remote_href = ?",
        ] {
            sqlx::query(sql)
                .bind(to)
                .bind(calendar_id)
                .bind(from)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Records a successful upload of `ical`, whose server version is now
    /// `etag` (`None` if the server did not say). Rows become `synced`
    /// unless they were changed again while the upload was running; then
    /// the previous base copy is kept, because those rows were edited
    /// against it (moved override ids still refer to it).
    pub async fn complete_put(&self, op: &PendingOp, etag: Option<&str>, ical: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let done = sqlx::query(
            "DELETE FROM pending_operations WHERE calendar_id = ? AND href = ? AND generation = ?",
        )
        .bind(&op.calendar_id)
        .bind(&op.href)
        .bind(op.generation)
        .execute(&mut *tx)
        .await?;
        if done.rows_affected() > 0 {
            sqlx::query(
                "UPDATE remote_objects SET etag = ?, ical = ?, presence = 'present', import_error = NULL \
                 WHERE calendar_id = ? AND href = ?",
            )
            .bind(etag)
            .bind(ical)
            .bind(&op.calendar_id)
            .bind(&op.href)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE events SET sync_status = 'synced', base_recurrence_id = NULL \
                 WHERE calendar_id = ? AND remote_href = ?",
            )
            .bind(&op.calendar_id)
            .bind(&op.href)
            .execute(&mut *tx)
            .await?;
        } else {
            sqlx::query(
                "UPDATE remote_objects SET etag = ?, presence = 'present' WHERE calendar_id = ? AND href = ?",
            )
            .bind(etag)
            .bind(&op.calendar_id)
            .bind(&op.href)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn complete_delete(&self, op: &PendingOp) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let done = sqlx::query(
            "DELETE FROM pending_operations WHERE calendar_id = ? AND href = ? AND generation = ?",
        )
        .bind(&op.calendar_id)
        .bind(&op.href)
        .bind(op.generation)
        .execute(&mut *tx)
        .await?;
        if done.rows_affected() > 0 {
            sqlx::query("DELETE FROM remote_objects WHERE calendar_id = ? AND href = ?")
                .bind(&op.calendar_id)
                .bind(&op.href)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Records a failed upload and updates the affected rows' status.
    pub async fn fail_operation(
        &self,
        op: &PendingOp,
        error: &str,
        failure: OpFailure,
    ) -> Result<()> {
        let (state, next, status) = match failure {
            OpFailure::Retry { at_ms } => ("pending", at_ms, "pending-sync"),
            OpFailure::Permanent => ("failed", 0, "sync-failed"),
            OpFailure::Conflict => ("conflict", 0, "conflict"),
        };
        let mut tx = self.pool.begin().await?;
        let updated = sqlx::query(
            "UPDATE pending_operations SET state = ?, next_attempt_ms = ?, attempts = attempts + 1, last_error = ? \
             WHERE calendar_id = ? AND href = ? AND generation = ?",
        )
        .bind(state)
        .bind(next)
        .bind(error)
        .bind(&op.calendar_id)
        .bind(&op.href)
        .bind(op.generation)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() == 0 {
            // Changed again meanwhile: the newer change gets its own attempt.
            return Ok(());
        }
        sqlx::query("UPDATE events SET sync_status = ? WHERE calendar_id = ? AND remote_href = ?")
            .bind(status)
            .bind(&op.calendar_id)
            .bind(&op.href)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// The (calendar, href) of the remote object an event or occurrence belongs to.
    pub async fn object_of(
        &self,
        event_or_occurrence_id: &str,
    ) -> Result<Option<(String, String)>> {
        let id = parse_occurrence_id(event_or_occurrence_id)
            .map(|(series, _)| series)
            .unwrap_or(event_or_occurrence_id);
        let row = sqlx::query(
            "SELECT calendar_id, remote_href FROM events WHERE (id = ? OR id = ?) AND remote_href IS NOT NULL LIMIT 1",
        )
        .bind(event_or_occurrence_id)
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| (r.get("calendar_id"), r.get("remote_href"))))
    }

    /// Resolves a conflict or permanent failure of the operation on one
    /// object. Works whether or not local rows are left (a failed deletion
    /// has none).
    pub async fn resolve_operation(
        &self,
        calendar_id: &str,
        href: &str,
        resolution: Resolution,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let queued: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pending_operations WHERE calendar_id = ? AND href = ?",
        )
        .bind(calendar_id)
        .bind(href)
        .fetch_one(&mut *tx)
        .await?;
        if queued == 0 {
            return Err(StorageError::NotFound(href.to_owned()));
        }
        match resolution {
            Resolution::Retry | Resolution::KeepLocal => {
                sqlx::query(
                    "UPDATE pending_operations SET state = 'pending', force = (force OR ?), attempts = 0, \
                     next_attempt_ms = 0, last_error = NULL, generation = generation + 1 \
                     WHERE calendar_id = ? AND href = ?",
                )
                .bind(resolution == Resolution::KeepLocal)
                .bind(calendar_id)
                .bind(href)
                .execute(&mut *tx)
                .await?;
                sqlx::query("UPDATE events SET sync_status = 'pending-sync' WHERE calendar_id = ? AND remote_href = ?")
                    .bind(calendar_id)
                    .bind(href)
                    .execute(&mut *tx)
                    .await?;
            }
            Resolution::UseServer => {
                let presence: Option<String> = sqlx::query_scalar(
                    "SELECT presence FROM remote_objects WHERE calendar_id = ? AND href = ?",
                )
                .bind(calendar_id)
                .bind(href)
                .fetch_optional(&mut *tx)
                .await?;
                let statements: &[&'static str] = if matches!(
                    presence.as_deref(),
                    Some("local") | None
                ) {
                    // The server never saw it: dropping the change drops the event.
                    &[
                        "DELETE FROM pending_operations WHERE calendar_id = ? AND href = ?",
                        "DELETE FROM events WHERE calendar_id = ? AND remote_href = ?",
                        "DELETE FROM remote_objects WHERE calendar_id = ? AND href = ?",
                    ]
                } else {
                    // Forget the version so the next (full) listing downloads
                    // the server copy, or reports it deleted, and replaces
                    // the local rows either way.
                    &[
                        "DELETE FROM pending_operations WHERE calendar_id = ? AND href = ?",
                        "UPDATE remote_objects SET etag = NULL WHERE calendar_id = ? AND href = ?",
                    ]
                };
                for &sql in statements {
                    sqlx::query(sql)
                        .bind(calendar_id)
                        .bind(href)
                        .execute(&mut *tx)
                        .await?;
                }
                // Server changes to this object may have been skipped while the
                // local change was pending, and the sync token has moved past
                // them: force a full (ETag-compared) listing next time.
                sqlx::query("UPDATE calendars SET sync_token = NULL WHERE id = ?")
                    .bind(calendar_id)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }

    /// Failed and conflicting operations of an account, oldest first.
    pub async fn problem_operations(&self, account_id: &str) -> Result<Vec<ProblemOperation>> {
        let rows = sqlx::query(
            "SELECT p.calendar_id, p.href, p.kind, p.state, p.last_error, o.ical, \
               (SELECT e.title FROM events e WHERE e.calendar_id = p.calendar_id AND e.remote_href = p.href \
                ORDER BY e.series_id IS NOT NULL, e.id LIMIT 1) AS title \
             FROM pending_operations p JOIN calendars c ON c.id = p.calendar_id \
             LEFT JOIN remote_objects o ON o.calendar_id = p.calendar_id AND o.href = p.href \
             WHERE c.account_id = ? AND p.state IN ('failed', 'conflict') ORDER BY p.created_at",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| ProblemOperation {
                calendar_id: r.get("calendar_id"),
                href: r.get("href"),
                kind: if r.get::<String, _>("kind") == "delete" {
                    OpKind::Delete
                } else {
                    OpKind::Put
                },
                conflict: r.get::<String, _>("state") == "conflict",
                error: r.get("last_error"),
                title: r.get("title"),
                base_ical: r.get::<Option<String>, _>("ical").unwrap_or_default(),
            })
            .collect())
    }

    /// Objects whose newest server version could not be imported.
    pub async fn import_failures(&self, account_id: &str) -> Result<Vec<(String, String)>> {
        let rows = sqlx::query(
            "SELECT o.href, o.import_error FROM remote_objects o JOIN calendars c ON c.id = o.calendar_id \
             WHERE c.account_id = ? AND o.import_error IS NOT NULL ORDER BY o.href",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get("href"), r.get("import_error")))
            .collect())
    }

    /// Account id of a calendar.
    pub async fn account_of_calendar(&self, calendar_id: &str) -> Result<Option<String>> {
        Ok(
            sqlx::query_scalar("SELECT account_id FROM calendars WHERE id = ?")
                .bind(calendar_id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    /// Calendars of an account, for the UI after adding it.
    pub async fn calendars_of(&self, account_id: &str) -> Result<Vec<Calendar>> {
        Ok(self
            .list_calendars()
            .await?
            .into_iter()
            .filter(|c| c.account_id == account_id)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_hrefs() {
        assert_eq!(
            pulled_id("acct/cal", "https://h/dav/cal/abc-1.ics"),
            "acct/cal/abc-1"
        );
        assert_eq!(pulled_id("c", "https://h/x/odd::name.ics"), "c/odd:_:name");
        assert_eq!(
            new_href("https://h/dav/cal/", "local-1"),
            "https://h/dav/cal/local-1.ics"
        );
        assert_eq!(
            new_href("https://h/dav/cal", "a b/c"),
            "https://h/dav/cal/a_b_c.ics"
        );
    }
}
