//! Sync engine behaviour against an in-memory fake server.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use calendar_core::timing::resolve_local;
use calendar_core::{EventDraft, EventTiming, SyncStatus, TimeRange};
use calendar_provider_api::{
    CalendarProvider, Capabilities, ChangeSet, Precondition, ProviderError, ProviderResult,
    RemoteCalendar, RemoteObject, WriteResult,
};
use calendar_storage::{RemoteCalendarSpec, Store};
use calendar_sync::{CredentialStore, MemoryCredentials, SyncEngine, SyncEvent};
use chrono::{Duration, NaiveDate};

const TZ: chrono_tz::Tz = chrono_tz::Europe::Berlin;
const CAL: &str = "https://dav.example/cal/alice/work/";

#[derive(Default)]
struct Server {
    objects: Mutex<HashMap<String, (String, String)>>,
    version: Mutex<u64>,
    fail: Mutex<Option<ProviderError>>,
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
    async fn list_calendars(&self) -> ProviderResult<Vec<RemoteCalendar>> {
        self.0.check()?;
        Ok(vec![RemoteCalendar {
            href: CAL.into(),
            name: "Work".into(),
            color: None,
            read_only: false,
            ctag: None,
            capabilities: Capabilities {
                incremental_sync: false,
                conditional_writes: true,
            },
        }])
    }
    async fn fetch_changes(
        &self,
        _: &RemoteCalendar,
        _: Option<&str>,
        known: &[(String, Option<String>)],
    ) -> ProviderResult<ChangeSet> {
        self.0.check()?;
        let objects = self.0.objects.lock().unwrap();
        let known: HashMap<_, _> = known.iter().cloned().collect();
        Ok(ChangeSet {
            upserted: objects
                .iter()
                .filter(|(h, (e, _))| known.get(*h).cloned().flatten().as_ref() != Some(e))
                .map(|(h, (e, i))| RemoteObject {
                    href: h.clone(),
                    etag: Some(e.clone()),
                    ical: i.clone(),
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
            etag: Some(etag),
        })
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
        .resolve_operation(&e.calendar_id, &href, true)
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
        .resolve_operation(&e.calendar_id, &href, false)
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
        .resolve_operation(&e.calendar_id, &format!("{CAL}local-1.ics"), true)
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
