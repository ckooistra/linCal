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
}

/// A remote object received from the server.
#[derive(Debug, Clone)]
pub struct IncomingObject {
    pub href: String,
    pub etag: Option<String>,
    pub ical: String,
    /// Parsed content, or why it could not be parsed.
    pub content: std::result::Result<ObjectContent, String>,
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

/// What to upload for a `put` operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushObject {
    pub uid: String,
    /// Version last seen on the server (`None` = create).
    pub etag: Option<String>,
    /// Last server copy, as the base for editing ("" if created locally).
    pub base_ical: String,
    pub content: ObjectContent,
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
        sqlx::query("INSERT INTO remote_objects (calendar_id, href, uid, etag, ical) VALUES (?, ?, ?, NULL, '')")
            .bind(calendar_id)
            .bind(&href)
            .bind(event_id)
            .execute(&mut **tx)
            .await?;
        Self::set_href(tx, event_id, &href).await?;
        Self::touch_object(tx, calendar_id, &href).await
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
    pub(crate) async fn touch_object(
        tx: &mut Transaction<'_, Sqlite>,
        calendar_id: &str,
        href: &str,
    ) -> Result<()> {
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
        sqlx::query("UPDATE events SET sync_status = 'pending-sync' WHERE calendar_id = ? AND remote_href = ?")
            .bind(calendar_id)
            .bind(href)
            .execute(&mut **tx)
            .await?;
        Ok(())
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
            return Self::touch_object(tx, calendar_id, href).await;
        }
        let uploaded: Option<Option<String>> = sqlx::query_scalar(
            "SELECT etag FROM remote_objects WHERE calendar_id = ? AND href = ?",
        )
        .bind(calendar_id)
        .bind(href)
        .fetch_optional(&mut **tx)
        .await?;
        if matches!(uploaded, Some(None) | None) {
            // Never reached the server: forget it entirely.
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
        let id = format!("{account_id}/{}", segment(&spec.remote_url));
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
            "SELECT id, remote_url, name, read_only, supports_sync, sync_token, ctag FROM calendars \
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
            })
            .collect())
    }

    pub async fn set_account_status(
        &self,
        account_id: &str,
        error: Option<&str>,
        synced_at: Option<i64>,
    ) -> Result<()> {
        sqlx::query("UPDATE accounts SET sync_error = ?, last_synced_at = COALESCE(?, last_synced_at) WHERE id = ?")
            .bind(error)
            .bind(synced_at)
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
            "SELECT a.id, a.provider, a.sync_error, a.last_synced_at, \
               (SELECT COUNT(*) FROM pending_operations p JOIN calendars c ON c.id = p.calendar_id WHERE c.account_id = a.id AND p.state = 'pending') AS pending, \
               (SELECT COUNT(*) FROM pending_operations p JOIN calendars c ON c.id = p.calendar_id WHERE c.account_id = a.id AND p.state = 'failed') AS failed, \
               (SELECT COUNT(*) FROM pending_operations p JOIN calendars c ON c.id = p.calendar_id WHERE c.account_id = a.id AND p.state = 'conflict') AS conflicts \
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
                }
            })
            .collect())
    }

    // --- pulling -----------------------------------------------------------

    /// Hrefs and ETags of the objects stored for a calendar.
    pub async fn known_objects(&self, calendar_id: &str) -> Result<Vec<(String, Option<String>)>> {
        let rows = sqlx::query(
            "SELECT href, etag FROM remote_objects WHERE calendar_id = ? AND etag IS NOT NULL",
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
        if let Some(m) = &content.master {
            Self::insert(tx, &row(master_id.to_owned(), m, None)).await?;
        }
        for o in &content.overrides {
            let key = o.recurrence_id.unwrap_or_else(|| o.timing.start_key());
            let event = match &content.master {
                Some(_) => row(
                    occurrence_id(master_id, &key),
                    o,
                    Some(SeriesLink {
                        series_id: master_id.to_owned(),
                        recurrence_id: key,
                    }),
                ),
                // Instances without their series (e.g. a single invitation
                // occurrence) are shown as one-off events.
                None => row(format!("{master_id}@{key}"), o, None),
            };
            Self::insert(tx, &event).await?;
        }
        sqlx::query("UPDATE events SET remote_href = ? WHERE calendar_id = ? AND (id = ? OR series_id = ? OR id LIKE ?)")
            .bind(href)
            .bind(calendar_id)
            .bind(master_id)
            .bind(master_id)
            .bind(format!("{master_id}@%"))
            .execute(&mut **tx)
            .await?;
        Ok(())
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
            let existing: Option<String> = sqlx::query_scalar(
                "SELECT id FROM events WHERE calendar_id = ? AND remote_href = ? AND series_id IS NULL \
                 ORDER BY (rrule IS NULL), id LIMIT 1",
            )
            .bind(calendar_id)
            .bind(&obj.href)
            .fetch_optional(&mut *tx)
            .await?;
            let master_id = existing
                .filter(|id| !id.contains('@'))
                .unwrap_or_else(|| pulled_id(calendar_id, &obj.href));
            sqlx::query("DELETE FROM events WHERE calendar_id = ? AND remote_href = ?")
                .bind(calendar_id)
                .bind(&obj.href)
                .execute(&mut *tx)
                .await?;
            let uid = obj
                .content
                .as_ref()
                .map(|c| c.uid.clone())
                .unwrap_or_default();
            sqlx::query(
                "INSERT INTO remote_objects (calendar_id, href, uid, etag, ical) VALUES (?, ?, ?, ?, ?) \
                 ON CONFLICT(calendar_id, href) DO UPDATE SET uid = excluded.uid, etag = excluded.etag, ical = excluded.ical",
            )
            .bind(calendar_id)
            .bind(&obj.href)
            .bind(&uid)
            .bind(&obj.etag)
            .bind(&obj.ical)
            .execute(&mut *tx)
            .await?;
            match &obj.content {
                Ok(content) => {
                    // One bad object must not abort the whole batch.
                    sqlx::query("SAVEPOINT import_object")
                        .execute(&mut *tx)
                        .await?;
                    match Self::insert_object_rows(
                        &mut tx,
                        calendar_id,
                        &obj.href,
                        &master_id,
                        content,
                    )
                    .await
                    {
                        Ok(()) => {
                            sqlx::query("RELEASE import_object")
                                .execute(&mut *tx)
                                .await?;
                            report.applied += 1;
                        }
                        Err(e) => {
                            sqlx::query("ROLLBACK TO import_object")
                                .execute(&mut *tx)
                                .await?;
                            sqlx::query("RELEASE import_object")
                                .execute(&mut *tx)
                                .await?;
                            report.failed.push((obj.href.clone(), e.to_string()));
                        }
                    }
                }
                Err(reason) => report.failed.push((obj.href.clone(), reason.clone())),
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
            "SELECT uid, etag, ical FROM remote_objects WHERE calendar_id = ? AND href = ?",
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
        let mut master = None;
        let mut overrides = Vec::new();
        for row in rows {
            let event = decode_event(row)?;
            let content = EventContent {
                title: event.title,
                description: event.description,
                location: event.location,
                timing: event.timing,
                recurrence: event.recurrence,
                recurrence_id: event.series.as_ref().map(|s| s.recurrence_id),
            };
            if event.series.is_some() {
                overrides.push(content);
            } else if let Some((_, key)) = event
                .id
                .rsplit_once('@')
                .and_then(|(s, k)| Some((s, k.parse::<OccurrenceKey>().ok()?)))
            {
                // A lone instance imported without its series.
                overrides.push(EventContent {
                    recurrence_id: Some(key),
                    ..content
                });
            } else {
                master = Some(content);
            }
        }
        Ok(Some(PushObject {
            uid: obj.get("uid"),
            etag: obj.get("etag"),
            base_ical: obj.get("ical"),
            content: ObjectContent {
                uid: obj.get("uid"),
                master,
                overrides,
            },
        }))
    }

    /// Records a successful upload. Rows become `synced` unless they were
    /// changed again while the upload was running.
    pub async fn complete_put(&self, op: &PendingOp, etag: Option<&str>, ical: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE remote_objects SET etag = ?, ical = ? WHERE calendar_id = ? AND href = ?",
        )
        .bind(etag)
        .bind(ical)
        .bind(&op.calendar_id)
        .bind(&op.href)
        .execute(&mut *tx)
        .await?;
        let done = sqlx::query(
            "DELETE FROM pending_operations WHERE calendar_id = ? AND href = ? AND generation = ?",
        )
        .bind(&op.calendar_id)
        .bind(&op.href)
        .bind(op.generation)
        .execute(&mut *tx)
        .await?;
        if done.rows_affected() > 0 {
            sqlx::query("UPDATE events SET sync_status = 'synced' WHERE calendar_id = ? AND remote_href = ?")
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
        sqlx::query(
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

    /// Resolves a conflict or permanent failure. `keep_local` re-uploads the
    /// local version unconditionally; otherwise the local change is dropped
    /// and the server version is fetched on the next sync.
    pub async fn resolve_operation(
        &self,
        calendar_id: &str,
        href: &str,
        keep_local: bool,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        if keep_local {
            sqlx::query(
                "UPDATE pending_operations SET state = 'pending', force = 1, attempts = 0, next_attempt_ms = 0, \
                 last_error = NULL, generation = generation + 1 WHERE calendar_id = ? AND href = ?",
            )
            .bind(calendar_id)
            .bind(href)
            .execute(&mut *tx)
            .await?;
            sqlx::query("UPDATE events SET sync_status = 'pending-sync' WHERE calendar_id = ? AND remote_href = ?")
                .bind(calendar_id)
                .bind(href)
                .execute(&mut *tx)
                .await?;
        } else {
            for sql in [
                "DELETE FROM pending_operations WHERE calendar_id = ? AND href = ?",
                // Forget the ETag so the next sync downloads the server copy.
                "UPDATE remote_objects SET etag = NULL WHERE calendar_id = ? AND href = ?",
                "UPDATE events SET sync_status = 'synced' WHERE calendar_id = ? AND remote_href = ?",
            ] {
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
            // Nothing local remains of a never-uploaded object.
            sqlx::query(
                "DELETE FROM remote_objects WHERE calendar_id = ? AND href = ? AND ical = ''",
            )
            .bind(calendar_id)
            .bind(href)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
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
