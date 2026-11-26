//! Sync engine behaviour against an in-memory fake server.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

use async_trait::async_trait;
use calendar_core::timing::resolve_local;
use calendar_core::{EventDraft, EventTiming, SyncStatus, TimeRange};
use calendar_provider_api::{
    CalendarProvider, Capabilities, ChangeSet, Precondition, ProviderError, ProviderResult,
    RemoteCalendar, RemoteObject, WriteResult,
};
use calendar_storage::{RemoteCalendarSpec, Resolution, Store};
use calendar_sync::{CredentialStore, MemoryCredentials, SyncEngine, SyncEvent};
use chrono::{Duration, NaiveDate};

const TZ: chrono_tz::Tz = chrono_tz::Europe::Berlin;
const CAL: &str = "https://dav.example/cal/alice/work/";
/// A second calendar of the same account, listed when `Server::home` is set.
const HOME: &str = "https://dav.example/cal/alice/home/";

#[derive(Default)]
struct Server {
    objects: Mutex<HashMap<String, (String, String)>>,
    version: Mutex<u64>,
    fail: Mutex<Option<ProviderError>>,
    /// Answer writes without an ETag (allowed by RFC 4791 §5.3.4).
    no_etag: Mutex<bool>,
    /// Pause writes: signal the first Notify on entry, wait for the second.
    gate: Mutex<Option<(Arc<Notify>, Arc<Notify>)>>,
    /// Also list the calendar `HOME`.
    home: Mutex<bool>,
    /// Refuse to move objects between calendars (as some servers do).
    no_moves: Mutex<bool>,
    /// Requests made, as "MOVE <href>" (moves only).
    moves: Mutex<Vec<String>>,
    /// Refuse writes under this prefix (as Google refuses some events).
    refuse_under: Mutex<Option<String>>,
}

impl Server {
    fn bump(&self) -> String {
        let mut v = self.version.lock().unwrap();
        *v += 1;
        format!("\"{}\"", *v)
    }
    fn put_raw(&self, href: &str, ical: &str) {
        let etag = self.bump();
        self.objects
            .lock()
            .unwrap()
            .insert(href.into(), (etag, ical.into()));
    }
    fn check(&self) -> ProviderResult<()> {
        match self.fail.lock().unwrap().clone() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

struct Fake(Arc<Server>);

#[async_trait]
impl CalendarProvider for Fake {
    fn codec(&self) -> Arc<dyn calendar_provider_api::ObjectCodec> {
        Arc::new(calendar_caldav::ICalendarCodec)
    }
    async fn list_calendars(&self) -> ProviderResult<Vec<RemoteCalendar>> {
        self.0.check()?;
        let mut calendars = vec![(CAL, "Work")];
        if *self.0.home.lock().unwrap() {
            calendars.push((HOME, "Home"));
        }
        Ok(calendars
            .into_iter()
            .map(|(href, name)| RemoteCalendar {
                href: href.into(),
                name: name.into(),
                color: None,
                read_only: false,
                ctag: None,
                capabilities: Capabilities {
                    incremental_sync: false,
                    conditional_writes: true,
                },
            })
            .collect())
    }
    async fn fetch_changes(
        &self,
        calendar: &RemoteCalendar,
        _: Option<&str>,
        known: &[(String, Option<String>)],
    ) -> ProviderResult<ChangeSet> {
        self.0.check()?;
        let all = self.0.objects.lock().unwrap();
        let objects: HashMap<String, (String, String)> = all
            .iter()
            .filter(|(href, _)| href.starts_with(&calendar.href))
            .map(|(href, object)| (href.clone(), object.clone()))
            .collect();
        let known: HashMap<_, _> = known.iter().cloned().collect();
        Ok(ChangeSet {
            upserted: objects
                .iter()
                .filter(|(h, (e, _))| known.get(*h).cloned().flatten().as_ref() != Some(e))
                .map(|(h, (e, i))| RemoteObject {
                    href: h.clone(),
                    etag: Some(e.clone()),
                    data: i.clone(),
                })
                .collect(),
            deleted: known
                .keys()
                .filter(|h| !objects.contains_key(*h))
                .cloned()
                .collect(),
            cursor: None,
        })
    }
    async fn put_object(
        &self,
        href: &str,
        ical: &str,
        pre: &Precondition,
    ) -> ProviderResult<WriteResult> {
        self.0.check()?;
        if let Some(prefix) = self.0.refuse_under.lock().unwrap().as_deref()
            && href.starts_with(prefix)
        {
            return Err(ProviderError::InvalidResponse("HTTP 400 invalid".into()));
        }
        let gate = self.0.gate.lock().unwrap().clone();
        if let Some((entered, release)) = gate {
            entered.notify_one();
            release.notified().await;
        }
        let current = self
            .0
            .objects
            .lock()
            .unwrap()
            .get(href)
            .map(|(e, _)| e.clone());
        match (pre, &current) {
            (Precondition::Create, Some(_)) => return Err(ProviderError::PreconditionFailed),
            (Precondition::Version(v), Some(c)) if v != c => {
                return Err(ProviderError::PreconditionFailed);
            }
            (Precondition::Version(_), None) => return Err(ProviderError::PreconditionFailed),
            _ => {}
        }
        self.0.put_raw(href, ical);
        let etag = self.0.objects.lock().unwrap()[href].0.clone();
        Ok(WriteResult {
            href: href.into(),
            data: None,
            etag: (!*self.0.no_etag.lock().unwrap()).then_some(etag),
        })
    }
    async fn get_object(&self, href: &str) -> ProviderResult<Option<RemoteObject>> {
        self.0.check()?;
        Ok(self
            .0
            .objects
            .lock()
            .unwrap()
            .get(href)
            .map(|(e, i)| RemoteObject {
                href: href.into(),
                etag: Some(e.clone()),
                data: i.clone(),
            }))
    }
    async fn delete_object(&self, href: &str, version: Option<&str>) -> ProviderResult<()> {
        self.0.check()?;
        let mut objects = self.0.objects.lock().unwrap();
        if let (Some(v), Some((cur, _))) = (version, objects.get(href))
            && v != cur
        {
            return Err(ProviderError::PreconditionFailed);
        }
        objects.remove(href);
        Ok(())
    }
    async fn move_object(
        &self,
        href: &str,
        to_calendar: &str,
        version: Option<&str>,
    ) -> ProviderResult<Option<WriteResult>> {
        self.0.check()?;
        if *self.0.no_moves.lock().unwrap() {
            return Ok(None);
        }
        self.0.moves.lock().unwrap().push(format!("MOVE {href}"));
        let target = format!("{to_calendar}{}", href.rsplit('/').next().unwrap());
        let etag = self.0.bump();
        let mut objects = self.0.objects.lock().unwrap();
        let Some((current, data)) = objects.get(href).cloned() else {
            return Err(ProviderError::NotFound(href.into()));
        };
        if version.is_some_and(|v| v != current) || objects.contains_key(&target) {
            return Err(ProviderError::PreconditionFailed);
        }
        objects.remove(href);
        objects.insert(target.clone(), (etag, data));
        // Like most CalDAV servers: no ETag in the answer.
        Ok(Some(WriteResult {
            href: target,
            etag: None,
            data: None,
        }))
    }
}

struct Env {
    store: Store,
    server: Arc<Server>,
    engine: SyncEngine,
    events: Arc<Mutex<Vec<SyncEvent>>>,
    calendar_id: String,
}

async fn env_with(store: Store, server: Arc<Server>) -> Env {
    let creds = Arc::new(MemoryCredentials::default());
    if store.remote_accounts().await.unwrap().is_empty() {
        let account = calendar_core::Account {
            id: "acct".into(),
            name: "alice".into(),
            provider: calendar_core::ProviderKind::Caldav,
        };
        let spec = RemoteCalendarSpec {
            remote_url: CAL.into(),
            name: "Work".into(),
            color: None,
            read_only: false,
            supports_sync: false,
            ctag: None,
        };
        store
            .create_remote_account(&account, &serde_json::json!({}), &[spec])
            .await
            .unwrap();
    }
    creds.set("acct", "secret").unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    let s = Arc::clone(&server);
    let engine = SyncEngine::new(
        store.clone(),
        creds,
        Arc::new(move |_, password| {
            assert_eq!(password, "secret");
            Ok(Box::new(Fake(Arc::clone(&s))) as Box<dyn CalendarProvider>)
        }),
        Arc::new(move |e| sink.lock().unwrap().push(e)),
    );
    let calendar_id = store.calendars_of("acct").await.unwrap()[0].id.clone();
    Env {
        store,
        server,
        engine,
        events,
        calendar_id,
    }
}

async fn env() -> Env {
    env_with(
        Store::open_in_memory().await.unwrap(),
        Arc::new(Server::default()),
    )
    .await
}

fn ics(uid: &str, summary: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:{uid}\r\nDTSTART;TZID=Europe/Berlin:20260610T090000\r\nDTEND;TZID=Europe/Berlin:20260610T100000\r\nSUMMARY:{summary}\r\nX-KEEP:me\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

fn june() -> TimeRange {
    TimeRange::new(
        resolve_local(
            &TZ,
            NaiveDate::from_ymd_opt(2026, 6, 1)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap(),
        ),
        resolve_local(
            &TZ,
            NaiveDate::from_ymd_opt(2026, 7, 1)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap(),
        ),
    )
}

fn draft(calendar: &str, title: &str) -> EventDraft {
    let start = resolve_local(
        &TZ,
        NaiveDate::from_ymd_opt(2026, 6, 12)
            .unwrap()
            .and_hms_opt(14, 0, 0)
            .unwrap(),
    );
    EventDraft {
        reminders: None,
        calendar_id: calendar.into(),
        title: title.into(),
        description: String::new(),
        location: String::new(),
        timing: EventTiming::Zoned {
            start,
            end: start + Duration::hours(1),
            time_zone: TZ,
        },
        rrule: None,
    }
}

async fn titles(store: &Store) -> Vec<(String, SyncStatus)> {
    let mut v: Vec<_> = store
        .query_range(june(), TZ)
        .await
        .unwrap()
        .into_iter()
        .map(|o| (o.title, o.sync_status))
        .collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

#[tokio::test]
async fn pulls_then_pushes_local_creates_edits_and_deletes() {
    let e = env().await;
    e.server
        .put_raw(&format!("{CAL}remote.ics"), &ics("remote", "From server"));
    let report = e.engine.sync_account("acct").await.unwrap();
    assert_eq!(report.downloaded, 1);
    assert_eq!(
        titles(&e.store).await,
        vec![("From server".into(), SyncStatus::Synced)]
    );

    // Local create → uploaded.
    e.store
        .create_event("local-1", draft(&e.calendar_id, "Mine"))
        .await
        .unwrap();
    assert_eq!(e.engine.sync_account("acct").await.unwrap().uploaded, 1);
    let uploaded = e.server.objects.lock().unwrap()[&format!("{CAL}local-1.ics")]
        .1
        .clone();
    assert!(uploaded.contains("SUMMARY:Mine") && uploaded.contains("UID:local-1"));
    assert!(
        titles(&e.store)
            .await
            .iter()
            .all(|(_, s)| *s == SyncStatus::Synced)
    );

    // Local edit of the server event keeps its unknown properties.
    let id = format!("{}/remote", e.calendar_id);
    let mut d = draft(&e.calendar_id, "Edited here");
    d.timing = e.store.get_event(&id).await.unwrap().unwrap().timing;
    e.store.update_event(&id, d).await.unwrap();
    e.engine.sync_account("acct").await.unwrap();
    let edited = e.server.objects.lock().unwrap()[&format!("{CAL}remote.ics")]
        .1
        .clone();
    assert!(edited.contains("SUMMARY:Edited here"));
    assert!(
        edited.contains("X-KEEP:me"),
        "unknown properties survive: {edited}"
    );

    // Local delete → removed on the server.
    e.store.delete_event("local-1").await.unwrap();
    e.engine.sync_account("acct").await.unwrap();
    assert!(
        !e.server
            .objects
            .lock()
            .unwrap()
            .contains_key(&format!("{CAL}local-1.ics"))
    );

    // Remote delete → removed locally.
    e.server.objects.lock().unwrap().clear();
    e.engine.sync_account("acct").await.unwrap();
    assert!(titles(&e.store).await.is_empty());
    assert!(matches!(
        e.events.lock().unwrap().last(),
        Some(SyncEvent::Synced { changed: true, .. })
    ));
}

#[tokio::test]
async fn calendars_read_by_older_readers_are_read_again_once() {
    let e = env().await;
    e.server.put_raw(&format!("{CAL}a.ics"), &ics("a", "One"));
    e.server.put_raw(&format!("{CAL}b.ics"), &ics("b", "Two"));
    assert_eq!(e.engine.sync_account("acct").await.unwrap().downloaded, 2);
    assert_eq!(e.engine.sync_account("acct").await.unwrap().downloaded, 0);
    // As after an update that taught the readers something new.
    e.store.set_reader_version(&e.calendar_id, 0).await.unwrap();
    e.server
        .objects
        .lock()
        .unwrap()
        .remove(&format!("{CAL}b.ics"));
    let report = e.engine.sync_account("acct").await.unwrap();
    assert_eq!((report.downloaded, report.deleted), (1, 1));
    assert_eq!(e.engine.sync_account("acct").await.unwrap().downloaded, 0);
}

#[tokio::test]
async fn edits_to_different_fields_merge_instead_of_conflicting() {
    let e = env().await;
    let href = format!("{CAL}remote.ics");
    e.server.put_raw(&href, &ics("remote", "Original"));
    e.engine.sync_account("acct").await.unwrap();
    let id = format!("{}/remote", e.calendar_id);
    // Here: a new location. On the server: a new title.
    let mut d = e.store.get_event(&id).await.unwrap().unwrap().draft();
    d.location = "Room 4".into();
    e.store.update_event(&id, d).await.unwrap();
    e.server.put_raw(
        &href,
        &ics("remote", "Renamed elsewhere").replace("X-KEEP:me", "X-KEEP:me\r\nX-OTHER:theirs"),
    );

    let report = e.engine.sync_account("acct").await.unwrap();
    assert_eq!((report.conflicts, report.uploaded), (0, 1));
    let on_server = e.server.objects.lock().unwrap()[&href].1.clone();
    assert!(on_server.contains("SUMMARY:Renamed elsewhere"));
    assert!(on_server.contains("LOCATION:Room 4"));
    // Built on the server's copy: its unknown properties survive too.
    assert!(on_server.contains("X-OTHER:theirs"), "{on_server}");
    let local = e.store.get_event(&id).await.unwrap().unwrap();
    assert_eq!(
        (
            local.title.as_str(),
            local.location.as_str(),
            local.sync_status
        ),
        ("Renamed elsewhere", "Room 4", SyncStatus::Synced)
    );
    // Nothing left to do.
    let again = e.engine.sync_account("acct").await.unwrap();
    assert_eq!((again.uploaded, again.downloaded), (0, 0));

    // The same field changed on both sides stays a conflict, and the
    // server's version is kept for comparison.
    let mut d = e.store.get_event(&id).await.unwrap().unwrap().draft();
    d.title = "Mine".into();
    e.store.update_event(&id, d).await.unwrap();
    e.server.put_raw(&href, &ics("remote", "Theirs"));
    assert_eq!(e.engine.sync_account("acct").await.unwrap().conflicts, 1);
    let copy = e
        .store
        .conflict_copy(&e.calendar_id, &href)
        .await
        .unwrap()
        .unwrap();
    assert!(copy.contains("SUMMARY:Theirs"));
    let view = e.engine.conflict_view(&id).await.unwrap().unwrap();
    assert_eq!(view.local.unwrap().title, "Mine");
    assert_eq!(view.server.unwrap().title, "Theirs");
    e.store
        .resolve_operation(&e.calendar_id, &href, Resolution::UseServer)
        .await
        .unwrap();
    e.engine.sync_account("acct").await.unwrap();
    assert_eq!(
        e.store.conflict_copy(&e.calendar_id, &href).await.unwrap(),
        None
    );
}

#[tokio::test]
async fn concurrent_edits_become_conflicts_not_overwrites() {
    let e = env().await;
    let href = format!("{CAL}remote.ics");
    e.server.put_raw(&href, &ics("remote", "Original"));
    e.engine.sync_account("acct").await.unwrap();
    let id = format!("{}/remote", e.calendar_id);
    let mut d = draft(&e.calendar_id, "Local edit");
    d.timing = e.store.get_event(&id).await.unwrap().unwrap().timing;
    e.store.update_event(&id, d).await.unwrap();
    // Someone else edits on the server meanwhile.
    e.server.put_raw(&href, &ics("remote", "Server edit"));

    let report = e.engine.sync_account("acct").await.unwrap();
    assert_eq!(report.conflicts, 1);
    assert!(
        e.server.objects.lock().unwrap()[&href]
            .1
            .contains("Server edit"),
        "server copy not overwritten"
    );
    assert_eq!(
        titles(&e.store).await,
        vec![("Local edit".into(), SyncStatus::Conflict)]
    );

    // Keep mine → forced upload.
    e.store
        .resolve_operation(&e.calendar_id, &href, Resolution::KeepLocal)
        .await
        .unwrap();
    e.engine.sync_account("acct").await.unwrap();
    assert!(
        e.server.objects.lock().unwrap()[&href]
            .1
            .contains("Local edit")
    );
    assert_eq!(
        titles(&e.store).await,
        vec![("Local edit".into(), SyncStatus::Synced)]
    );

    // Another conflict, resolved in favour of the server.
    let mut d = draft(&e.calendar_id, "Local again");
    d.timing = e.store.get_event(&id).await.unwrap().unwrap().timing;
    e.store.update_event(&id, d).await.unwrap();
    e.server.put_raw(&href, &ics("remote", "Server wins"));
    e.engine.sync_account("acct").await.unwrap();
    e.store
        .resolve_operation(&e.calendar_id, &href, Resolution::UseServer)
        .await
        .unwrap();
    e.engine.sync_account("acct").await.unwrap();
    assert_eq!(
        titles(&e.store).await,
        vec![("Server wins".into(), SyncStatus::Synced)]
    );
}

#[tokio::test]
async fn offline_changes_are_kept_and_retried() {
    let e = env().await;
    e.store
        .create_event("local-1", draft(&e.calendar_id, "Offline edit"))
        .await
        .unwrap();
    *e.server.fail.lock().unwrap() = Some(ProviderError::Network("connection failed".into()));
    let err = e.engine.sync_account("acct").await.unwrap_err();
    assert!(calendar_sync::user_message(&err).starts_with("Offline"));
    assert!(matches!(
        e.events.lock().unwrap().last(),
        Some(SyncEvent::Failed { offline: true, .. })
    ));
    assert_eq!(
        titles(&e.store).await,
        vec![("Offline edit".into(), SyncStatus::PendingSync)]
    );
    // A retry is scheduled with backoff rather than hammering the server.
    let next = e.engine.next_retry_ms().await.unwrap().unwrap();
    assert!(next > chrono::Utc::now().timestamp_millis() + 20_000);
    let status = e
        .store
        .account_statuses()
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.account_id == "acct")
        .unwrap();
    assert!(status.error.unwrap().starts_with("Offline"));

    // Back online, once the retry is due.
    *e.server.fail.lock().unwrap() = None;
    assert_eq!(
        e.engine.sync_account("acct").await.unwrap().uploaded,
        0,
        "not due yet"
    );
    e.store
        .resolve_operation(
            &e.calendar_id,
            &format!("{CAL}local-1.ics"),
            Resolution::KeepLocal,
        )
        .await
        .unwrap();
    assert_eq!(e.engine.sync_account("acct").await.unwrap().uploaded, 1);
    assert_eq!(
        titles(&e.store).await,
        vec![("Offline edit".into(), SyncStatus::Synced)]
    );
}

#[tokio::test]
async fn offline_changes_survive_a_restart() {
    let dir = std::env::temp_dir().join(format!("uc-sync-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("db.sqlite");
    let server = Arc::new(Server::default());
    {
        let store = Store::open(&path).await.unwrap();
        let e = env_with(store, Arc::clone(&server)).await;
        e.store
            .create_event("local-1", draft(&e.calendar_id, "Before crash"))
            .await
            .unwrap();
        e.store.close().await;
    }
    let store = Store::open(&path).await.unwrap();
    let e = env_with(store, server).await;
    assert_eq!(e.engine.sync_account("acct").await.unwrap().uploaded, 1);
    assert!(
        e.server.objects.lock().unwrap()[&format!("{CAL}local-1.ics")]
            .1
            .contains("Before crash")
    );
    e.store.close().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn wrong_passwords_are_reported_and_nothing_is_lost() {
    let e = env().await;
    e.store
        .create_event("local-1", draft(&e.calendar_id, "Keep"))
        .await
        .unwrap();
    *e.server.fail.lock().unwrap() = Some(ProviderError::Unauthorized);
    let err = e.engine.sync_account("acct").await.unwrap_err();
    assert!(calendar_sync::user_message(&err).contains("password"));
    assert_eq!(
        e.store
            .due_operations("acct", i64::MAX)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn backoff_grows_and_is_capped() {
    assert_eq!(calendar_sync::backoff(0).as_secs(), 30);
    assert_eq!(calendar_sync::backoff(1).as_secs(), 60);
    assert_eq!(calendar_sync::backoff(50).as_secs(), 3600);
}

// --- audit regressions -------------------------------------------------------

/// F01: a delete made while the first upload is in flight reaches the server.
#[tokio::test]
async fn deleting_during_the_first_upload_deletes_on_the_server() {
    let e = env().await;
    e.store
        .create_event("local-1", draft(&e.calendar_id, "Short-lived"))
        .await
        .unwrap();
    let (entered, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    *e.server.gate.lock().unwrap() = Some((Arc::clone(&entered), Arc::clone(&release)));
    let user = async {
        entered.notified().await;
        e.store.delete_event("local-1").await.unwrap();
        *e.server.gate.lock().unwrap() = None;
        release.notify_one();
    };
    let (first, ()) = tokio::join!(e.engine.sync_account("acct"), user);
    first.unwrap();
    // The PUT landed; the queued delete follows on the next pass.
    e.engine.sync_account("acct").await.unwrap();
    assert!(e.server.objects.lock().unwrap().is_empty());
    assert!(
        e.store
            .due_operations("acct", i64::MAX)
            .await
            .unwrap()
            .is_empty()
    );
}

/// F11: after a write answered without an ETag, the next edit is checked
/// against the server's real version instead of being sent as a create.
#[tokio::test]
async fn writes_without_etags_are_followed_by_a_version_check() {
    let e = env().await;
    *e.server.no_etag.lock().unwrap() = true;
    e.store
        .create_event("local-1", draft(&e.calendar_id, "v1"))
        .await
        .unwrap();
    assert_eq!(e.engine.sync_account("acct").await.unwrap().uploaded, 1);
    e.store
        .update_event("local-1", draft(&e.calendar_id, "v2"))
        .await
        .unwrap();
    let report = e.engine.sync_account("acct").await.unwrap();
    assert_eq!((report.uploaded, report.conflicts), (1, 0));
    assert!(
        e.server.objects.lock().unwrap()[&format!("{CAL}local-1.ics")]
            .1
            .contains("SUMMARY:v2")
    );
    assert_eq!(
        titles(&e.store).await,
        vec![("v2".into(), SyncStatus::Synced)]
    );
}

/// F06: hostile data fails one object, not the pass or the process.
#[tokio::test]
async fn malformed_objects_do_not_stop_synchronization() {
    let e = env().await;
    e.server.put_raw(
        &format!("{CAL}bad.ics"),
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:bad\r\nDTSTART:20260610T090000Z\r\nDURATION:P999999999999D\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
    );
    e.server
        .put_raw(&format!("{CAL}good.ics"), &ics("good", "Good"));
    let report = e.engine.sync_account("acct").await.unwrap();
    assert_eq!((report.downloaded, report.failed_objects), (1, 1));
    assert_eq!(
        titles(&e.store).await,
        vec![("Good".into(), SyncStatus::Synced)]
    );
    assert_eq!(e.store.import_failures("acct").await.unwrap().len(), 1);
    // Not downloaded again until it changes.
    let report = e.engine.sync_account("acct").await.unwrap();
    assert_eq!(report.failed_objects, 0);
}

/// F15: a failing account backs off instead of retrying every second.
#[tokio::test]
async fn failing_accounts_back_off() {
    let Env {
        store,
        server,
        engine,
        events,
        calendar_id,
    } = env().await;
    let engine = Arc::new(engine);
    store
        .create_event("local-1", draft(&calendar_id, "Waiting"))
        .await
        .unwrap();
    *server.fail.lock().unwrap() = Some(ProviderError::Unauthorized);
    engine.sync_all(true).await.unwrap();
    let passes = || {
        events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, SyncEvent::Syncing { .. }))
            .count()
    };
    assert_eq!(passes(), 1);
    // A timed pass skips the account; the next retry is not "now".
    engine.sync_all(false).await.unwrap();
    assert_eq!(passes(), 1);
    let next = engine.next_retry_ms().await.unwrap().unwrap();
    assert!(next > chrono::Utc::now().timestamp_millis() + 20_000);
    // A user-requested pass still tries, and success clears the backoff.
    *server.fail.lock().unwrap() = None;
    engine.sync_all(true).await.unwrap();
    assert_eq!(passes(), 2);
    assert!(engine.next_retry_ms().await.unwrap().is_none());
}

/// F10: a conflicting deletion is listed and can be resolved without an event.
#[tokio::test]
async fn conflicting_deletions_are_listed_and_resolvable() {
    let e = env().await;
    let href = format!("{CAL}remote.ics");
    e.server.put_raw(&href, &ics("remote", "Shared"));
    e.engine.sync_account("acct").await.unwrap();
    e.store
        .delete_event(&format!("{}/remote", e.calendar_id))
        .await
        .unwrap();
    // Someone else edits it before our delete arrives.
    e.server.put_raw(&href, &ics("remote", "Shared, edited"));
    assert_eq!(e.engine.sync_account("acct").await.unwrap().conflicts, 1);
    let problems = e.engine.problems().await.unwrap();
    assert_eq!(problems.len(), 1);
    assert!(problems[0].deletion && problems[0].conflict);
    assert_eq!(problems[0].title, "Shared");
    e.engine
        .resolve(&e.calendar_id, &href, Resolution::UseServer)
        .await
        .unwrap();
    e.engine.sync_account("acct").await.unwrap();
    assert_eq!(
        titles(&e.store).await,
        vec![("Shared, edited".into(), SyncStatus::Synced)]
    );
}

#[tokio::test]
async fn imported_files_are_uploaded_once() {
    let e = env().await;
    let file = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:weekly@elsewhere\r\nDTSTART;TZID=Europe/Berlin:20260601T090000\r\n\
DTEND;TZID=Europe/Berlin:20260601T093000\r\nRRULE:FREQ=WEEKLY;COUNT=3\r\nSUMMARY:Weekly\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:weekly@elsewhere\r\nRECURRENCE-ID;TZID=Europe/Berlin:20260608T090000\r\n\
DTSTART;TZID=Europe/Berlin:20260608T110000\r\nDTEND;TZID=Europe/Berlin:20260608T113000\r\n\
SUMMARY:Weekly (moved)\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let report = calendar_sync::import_ics(&e.store, &e.calendar_id, file.into())
        .await
        .unwrap();
    assert_eq!(report.imported, 1);
    assert_eq!(e.engine.sync_account("acct").await.unwrap().uploaded, 1);
    let objects = e.server.objects.lock().unwrap().clone();
    assert_eq!(objects.len(), 1);
    let (_, uploaded) = objects.values().next().unwrap();
    assert!(uploaded.contains("RRULE:FREQ=WEEKLY;COUNT=3"), "{uploaded}");
    assert!(uploaded.contains("RECURRENCE-ID") && uploaded.contains("SUMMARY:Weekly (moved)"));
    assert!(
        titles(&e.store)
            .await
            .iter()
            .all(|(_, s)| *s == SyncStatus::Synced)
    );
    // Importing again finds the events in the calendar.
    let again = calendar_sync::import_ics(&e.store, &e.calendar_id, file.into())
        .await
        .unwrap();
    assert_eq!((again.imported, again.already_present), (0, 1));
    assert_eq!(e.engine.sync_account("acct").await.unwrap().uploaded, 0);
}

#[tokio::test]
async fn two_programs_never_sync_the_same_account_at_once() {
    // The desktop app and `ucal`: two stores on one database file, each
    // with its own engine, sharing the lock directory.
    let dir = std::env::temp_dir().join(format!("uc-two-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("db.sqlite");
    let server = Arc::new(Server::default());
    let mut desktop = env_with(Store::open(&path).await.unwrap(), Arc::clone(&server)).await;
    let mut terminal = env_with(Store::open(&path).await.unwrap(), Arc::clone(&server)).await;
    desktop.engine = desktop.engine.with_lock_dir(dir.clone());
    terminal.engine = terminal.engine.with_lock_dir(dir.clone());
    terminal
        .store
        .create_event("local-1", draft(&terminal.calendar_id, "From the terminal"))
        .await
        .unwrap();

    // The desktop app's pass stops in the middle of the upload.
    let (entered, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    *server.gate.lock().unwrap() = Some((Arc::clone(&entered), Arc::clone(&release)));
    let first = desktop.engine.sync_account("acct");
    let second = async {
        entered.notified().await;
        *server.gate.lock().unwrap() = None;
        let pass = terminal.engine.sync_account("acct");
        tokio::pin!(pass);
        let waited = tokio::time::timeout(std::time::Duration::from_millis(300), &mut pass).await;
        assert!(waited.is_err(), "the second pass waits for the first");
        release.notify_one();
        pass.await
    };
    let (first, second) = tokio::join!(first, second);
    assert_eq!(first.unwrap().uploaded, 1);
    assert_eq!(second.unwrap().uploaded, 0, "nothing left to upload");
    assert_eq!(*server.version.lock().unwrap(), 1, "uploaded once");
    desktop.store.close().await;
    terminal.store.close().await;
    std::fs::remove_dir_all(&dir).ok();
}

/// The calendar of the account named `name`.
async fn calendar_named(store: &Store, name: &str) -> String {
    store
        .calendars_of("acct")
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.name == name)
        .unwrap()
        .id
}

#[tokio::test]
async fn the_server_moves_an_event_between_its_calendars() {
    let e = env().await;
    *e.server.home.lock().unwrap() = true;
    let href = format!("{CAL}remote.ics");
    e.server.put_raw(&href, &ics("remote", "Planning"));
    e.engine.sync_account("acct").await.unwrap();
    let home = calendar_named(&e.store, "Home").await;
    let id = format!("{}/remote", e.calendar_id);

    let moved = e.store.move_event(&id, "unused", &home).await.unwrap();
    assert_eq!(
        (moved.id.as_str(), moved.sync_status),
        (id.as_str(), SyncStatus::PendingSync)
    );
    let report = e.engine.sync_account("acct").await.unwrap();
    assert_eq!(report.uploaded, 1, "one request");
    assert_eq!(*e.server.moves.lock().unwrap(), [format!("MOVE {href}")]);
    {
        let objects = e.server.objects.lock().unwrap();
        assert_eq!(objects.len(), 1);
        let kept = &objects[&format!("{HOME}remote.ics")].1;
        assert_eq!(
            kept,
            &ics("remote", "Planning"),
            "untouched, X-KEEP included"
        );
    }
    // Further passes change nothing: no duplicates, no ping-pong.
    let again = e.engine.sync_account("acct").await.unwrap();
    assert_eq!(
        (again.uploaded, again.downloaded),
        (0, 0),
        "the new version was read right after the move"
    );
    e.engine.sync_account("acct").await.unwrap();
    let shown = e.store.query_range(june(), TZ).await.unwrap();
    assert_eq!(shown.len(), 1);
    assert_eq!(
        (
            shown[0].series_id.as_deref().unwrap_or(&shown[0].id),
            shown[0].calendar_id.as_str(),
            shown[0].sync_status
        ),
        (id.as_str(), home.as_str(), SyncStatus::Synced)
    );
}

#[tokio::test]
async fn an_edit_made_with_a_move_follows_it() {
    let e = env().await;
    *e.server.home.lock().unwrap() = true;
    e.server
        .put_raw(&format!("{CAL}remote.ics"), &ics("remote", "Planning"));
    e.engine.sync_account("acct").await.unwrap();
    let home = calendar_named(&e.store, "Home").await;
    let shown = e.store.query_range(june(), TZ).await.unwrap();
    let mut edit = draft(&home, "Planning (Home)");
    edit.timing = shown[0].timing.clone();
    e.store
        .update_and_move(&shown[0].id, "unused", edit)
        .await
        .unwrap();
    let report = e.engine.sync_account("acct").await.unwrap();
    assert_eq!(report.uploaded, 2, "the move, then the edit");
    let objects = e.server.objects.lock().unwrap().clone();
    assert_eq!(objects.len(), 1);
    let data = &objects[&format!("{HOME}remote.ics")].1;
    assert!(data.contains("SUMMARY:Planning (Home)") && data.contains("X-KEEP:me"));
    assert!(data.contains("UID:remote"), "the same event");
    assert_eq!(
        titles(&e.store).await,
        [("Planning (Home)".into(), SyncStatus::Synced)]
    );
}

#[tokio::test]
async fn a_move_conflicting_with_a_server_change_waits_for_the_user() {
    let e = env().await;
    *e.server.home.lock().unwrap() = true;
    let href = format!("{CAL}remote.ics");
    e.server.put_raw(&href, &ics("remote", "Planning"));
    e.engine.sync_account("acct").await.unwrap();
    let home = calendar_named(&e.store, "Home").await;
    let id = format!("{}/remote", e.calendar_id);
    e.store.move_event(&id, "unused", &home).await.unwrap();
    e.server.put_raw(&href, &ics("remote", "Planning, edited"));

    let report = e.engine.sync_account("acct").await.unwrap();
    assert_eq!(report.conflicts, 1);
    // Work's listing still has it: it isn't imported there a second time.
    assert_eq!(
        titles(&e.store).await,
        [("Planning".into(), SyncStatus::Conflict)]
    );
    let problems = e.engine.problems().await.unwrap();
    assert!(
        problems[0].message.contains("moved"),
        "{}",
        problems[0].message
    );

    // Keeping the local version moves the server's edited copy.
    e.engine
        .resolve(&home, &href, Resolution::KeepLocal)
        .await
        .unwrap();
    e.engine.sync_account("acct").await.unwrap();
    e.engine.sync_account("acct").await.unwrap();
    assert!(
        e.server
            .objects
            .lock()
            .unwrap()
            .contains_key(&format!("{HOME}remote.ics"))
    );
    assert_eq!(
        titles(&e.store).await,
        [("Planning, edited".into(), SyncStatus::Synced)]
    );
}

#[tokio::test]
async fn using_the_servers_version_of_a_move_leaves_the_event_where_it_was() {
    let e = env().await;
    *e.server.home.lock().unwrap() = true;
    let href = format!("{CAL}remote.ics");
    e.server.put_raw(&href, &ics("remote", "Planning"));
    e.engine.sync_account("acct").await.unwrap();
    let home = calendar_named(&e.store, "Home").await;
    e.store
        .move_event(&format!("{}/remote", e.calendar_id), "unused", &home)
        .await
        .unwrap();
    e.server.put_raw(&href, &ics("remote", "Planning, edited"));
    e.engine.sync_account("acct").await.unwrap();
    e.engine
        .resolve(&home, &href, Resolution::UseServer)
        .await
        .unwrap();
    e.engine.sync_account("acct").await.unwrap();
    let shown = e.store.query_range(june(), TZ).await.unwrap();
    assert_eq!(shown.len(), 1);
    assert_eq!(
        (
            shown[0].title.as_str(),
            shown[0].calendar_id.as_str(),
            shown[0].sync_status
        ),
        (
            "Planning, edited",
            e.calendar_id.as_str(),
            SyncStatus::Synced
        )
    );
    assert!(
        e.server.moves.lock().unwrap().len() == 1,
        "nothing tried again"
    );
}

#[tokio::test]
async fn a_server_that_cannot_move_gets_a_copy_and_a_deletion() {
    let e = env().await;
    *e.server.home.lock().unwrap() = true;
    *e.server.no_moves.lock().unwrap() = true;
    let href = format!("{CAL}remote.ics");
    e.server.put_raw(&href, &ics("remote", "Planning"));
    e.engine.sync_account("acct").await.unwrap();
    let home = calendar_named(&e.store, "Home").await;

    let moved = e
        .store
        .move_event(&format!("{}/remote", e.calendar_id), "moved", &home)
        .await
        .unwrap();
    assert_eq!(moved.sync_status, SyncStatus::PendingSync);
    let report = e.engine.sync_account("acct").await.unwrap();
    assert_eq!(
        report.uploaded, 2,
        "one deletion, one creation, in the same pass"
    );
    {
        let objects = e.server.objects.lock().unwrap();
        assert!(!objects.contains_key(&href), "gone from Work");
        assert_eq!(objects.len(), 1);
        let (copy_href, (_, copy)) = objects.iter().next().unwrap();
        assert!(copy_href.starts_with(HOME));
        assert!(copy.contains("SUMMARY:Planning") && !copy.contains("UID:remote"));
    }
    // Further passes change nothing: no duplicates, no ping-pong.
    e.engine.sync_account("acct").await.unwrap();
    let shown = e.store.query_range(june(), TZ).await.unwrap();
    assert_eq!(shown.len(), 1);
    assert_eq!(
        (shown[0].calendar_id.as_str(), shown[0].sync_status),
        (home.as_str(), SyncStatus::Synced)
    );
}

#[tokio::test]
async fn a_copy_the_server_refuses_leaves_the_original_where_it_was() {
    let e = env().await;
    *e.server.home.lock().unwrap() = true;
    *e.server.no_moves.lock().unwrap() = true;
    *e.server.refuse_under.lock().unwrap() = Some(HOME.into());
    let href = format!("{CAL}remote.ics");
    e.server.put_raw(&href, &ics("remote", "Planning"));
    e.engine.sync_account("acct").await.unwrap();
    let home = calendar_named(&e.store, "Home").await;
    e.store
        .move_event(&format!("{}/remote", e.calendar_id), "moved", &home)
        .await
        .unwrap();

    let report = e.engine.sync_account("acct").await.unwrap();
    assert_eq!((report.uploaded, report.failed_uploads), (0, 1));
    assert!(
        e.server.objects.lock().unwrap().contains_key(&href),
        "not deleted while the copy is refused"
    );
    let problems = e.engine.problems().await.unwrap();
    assert_eq!(problems.len(), 1, "only the copy: {problems:?}");
    assert!(!problems[0].deletion);
    // Another pass doesn't delete it either.
    e.engine.sync_account("acct").await.unwrap();
    assert!(e.server.objects.lock().unwrap().contains_key(&href));

    // Giving the copy up brings the original back, unchanged on the server.
    e.engine
        .resolve(
            &problems[0].calendar_id,
            &problems[0].href,
            Resolution::UseServer,
        )
        .await
        .unwrap();
    e.engine.sync_account("acct").await.unwrap();
    let shown = e.store.query_range(june(), TZ).await.unwrap();
    assert_eq!(
        shown
            .iter()
            .map(|o| (o.title.as_str(), o.calendar_id.as_str(), o.sync_status))
            .collect::<Vec<_>>(),
        [("Planning", e.calendar_id.as_str(), SyncStatus::Synced)]
    );
    assert_eq!(e.server.objects.lock().unwrap().len(), 1);
    assert!(e.engine.problems().await.unwrap().is_empty());
}

#[tokio::test]
async fn once_a_refused_copy_is_accepted_the_original_is_deleted() {
    let e = env().await;
    *e.server.home.lock().unwrap() = true;
    *e.server.no_moves.lock().unwrap() = true;
    *e.server.refuse_under.lock().unwrap() = Some(HOME.into());
    let href = format!("{CAL}remote.ics");
    e.server.put_raw(&href, &ics("remote", "Planning"));
    e.engine.sync_account("acct").await.unwrap();
    let home = calendar_named(&e.store, "Home").await;
    e.store
        .move_event(&format!("{}/remote", e.calendar_id), "moved", &home)
        .await
        .unwrap();
    e.engine.sync_account("acct").await.unwrap();

    *e.server.refuse_under.lock().unwrap() = None;
    let problem = e.engine.problems().await.unwrap().remove(0);
    e.engine
        .resolve(&problem.calendar_id, &problem.href, Resolution::Retry)
        .await
        .unwrap();
    let report = e.engine.sync_account("acct").await.unwrap();
    assert_eq!(report.uploaded, 2, "the copy, then the deletion");
    let objects = e.server.objects.lock().unwrap();
    assert!(!objects.contains_key(&href));
    assert!(objects.keys().all(|h| h.starts_with(HOME)));
}

#[tokio::test]
async fn a_move_never_deletes_a_change_made_on_the_server() {
    let e = env().await;
    let href = format!("{CAL}remote.ics");
    e.server.put_raw(&href, &ics("remote", "Shared"));
    e.engine.sync_account("acct").await.unwrap();
    e.store.ensure_local_calendar().await.unwrap();
    e.store
        .move_event(
            &format!("{}/remote", e.calendar_id),
            "moved",
            "local-personal",
        )
        .await
        .unwrap();
    // Someone edits the old copy before our deletion arrives.
    e.server.put_raw(&href, &ics("remote", "Shared, edited"));
    assert_eq!(e.engine.sync_account("acct").await.unwrap().conflicts, 1);
    assert!(e.server.objects.lock().unwrap().contains_key(&href), "kept");
    // Keeping the server's version brings it back beside the moved copy.
    e.engine
        .resolve(&e.calendar_id, &href, Resolution::UseServer)
        .await
        .unwrap();
    e.engine.sync_account("acct").await.unwrap();
    assert_eq!(
        titles(&e.store).await,
        vec![
            ("Shared".into(), SyncStatus::Saved),
            ("Shared, edited".into(), SyncStatus::Synced),
        ]
    );
}

#[tokio::test]
async fn moving_while_an_edit_uploads_is_not_a_conflict() {
    let e = env().await;
    *e.server.home.lock().unwrap() = true;
    e.server
        .put_raw(&format!("{CAL}remote.ics"), &ics("remote", "Planning"));
    e.engine.sync_account("acct").await.unwrap();
    let home = calendar_named(&e.store, "Home").await;
    let id = format!("{}/remote", e.calendar_id);
    let shown = e.store.query_range(june(), TZ).await.unwrap();
    let mut edit = draft(&e.calendar_id, "Planning (edited)");
    edit.timing = shown[0].timing.clone();
    e.store.update_event(&id, edit).await.unwrap();

    // The edit's upload is under way when the user moves the event.
    let (entered, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    *e.server.gate.lock().unwrap() = Some((Arc::clone(&entered), Arc::clone(&release)));
    let user = async {
        entered.notified().await;
        e.store.move_event(&id, "unused", &home).await.unwrap();
        *e.server.gate.lock().unwrap() = None;
        release.notify_one();
    };
    let (first, ()) = tokio::join!(e.engine.sync_account("acct"), user);
    first.unwrap();
    let report = e.engine.sync_account("acct").await.unwrap();
    assert_eq!(report.conflicts, 0, "the upload's own version is known");
    let objects = e.server.objects.lock().unwrap().clone();
    assert_eq!(objects.len(), 1);
    assert!(
        objects[&format!("{HOME}remote.ics")]
            .1
            .contains("SUMMARY:Planning (edited)")
    );
    assert_eq!(
        titles(&e.store).await,
        [("Planning (edited)".into(), SyncStatus::Synced)]
    );
}
