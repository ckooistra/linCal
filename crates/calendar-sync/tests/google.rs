//! A Google account through the whole engine: OAuth sign-in, download,
//! local edits, creates and deletes, against the fake Calendar API.

use std::sync::Arc;

use calendar_core::timing::resolve_local;
use calendar_core::{EventDraft, EventTiming, ProviderKind, TimeRange};
use calendar_google::testing::FakeGoogle;
use calendar_google::{GoogleProvider, oauth_client};
use calendar_provider_api::{
    CalendarProvider, Capabilities, ChangeSet, Precondition, ProviderResult, RemoteCalendar,
    RemoteObject, WriteResult,
};
use calendar_storage::{RemoteCalendarSpec, Store};
use calendar_sync::{BrowserOpener, CredentialStore, MemoryCredentials, SyncEngine};
use chrono::NaiveDate;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Notify;

const TZ: chrono_tz::Tz = chrono_tz::Europe::Berlin;

/// Plays the browser: "approves" by calling the loopback redirect.
fn browser() -> BrowserOpener {
    Arc::new(|auth: &str| {
        let auth = url::Url::parse(auth).unwrap();
        let q = |k: &str| {
            auth.query_pairs()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.into_owned())
                .unwrap()
        };
        let port = url::Url::parse(&q("redirect_uri")).unwrap().port().unwrap();
        let state = q("state");
        tokio::spawn(async move {
            let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .unwrap();
            s.write_all(format!("GET /?code=c&state={state} HTTP/1.1\r\n\r\n").as_bytes())
                .await
                .unwrap();
            let mut reply = String::new();
            let _ = s.read_to_string(&mut reply).await;
        });
        Ok(())
    })
}

fn june() -> TimeRange {
    let day = |d: u32| {
        resolve_local(
            &TZ,
            NaiveDate::from_ymd_opt(2026, 6, d)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap(),
        )
    };
    TimeRange::new(day(1), day(30))
}

async fn titles(store: &Store) -> Vec<String> {
    let mut v: Vec<_> = store
        .query_range(june(), TZ)
        .await
        .unwrap()
        .into_iter()
        .map(|o| o.title)
        .collect();
    v.sort();
    v
}

/// A CalDAV calendar in memory: href → (etag, iCalendar).
#[derive(Default)]
struct Dav(std::sync::Mutex<std::collections::HashMap<String, (String, String)>>);

const DAV_CAL: &str = "https://dav.example/cal/alice/personal/";

#[async_trait::async_trait]
impl CalendarProvider for Dav {
    fn codec(&self) -> Arc<dyn calendar_provider_api::ObjectCodec> {
        Arc::new(calendar_caldav::ICalendarCodec)
    }
    async fn list_calendars(&self) -> ProviderResult<Vec<RemoteCalendar>> {
        Ok(vec![RemoteCalendar {
            href: DAV_CAL.into(),
            name: "Personal".into(),
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
        let objects = self.0.lock().unwrap();
        Ok(ChangeSet {
            upserted: objects
                .iter()
                .filter(|(h, (e, _))| !known.iter().any(|(k, v)| k == *h && v.as_ref() == Some(e)))
                .map(|(h, (e, d))| RemoteObject {
                    href: h.clone(),
                    etag: Some(e.clone()),
                    data: d.clone(),
                })
                .collect(),
            deleted: known
                .iter()
                .filter(|(k, _)| !objects.contains_key(k))
                .map(|(k, _)| k.clone())
                .collect(),
            cursor: None,
        })
    }
    async fn put_object(
        &self,
        href: &str,
        data: &str,
        _: &Precondition,
    ) -> ProviderResult<WriteResult> {
        let etag = format!("\"{}\"", data.len());
        self.0
            .lock()
            .unwrap()
            .insert(href.into(), (etag.clone(), data.into()));
        Ok(WriteResult {
            href: href.into(),
            etag: Some(etag),
            data: None,
        })
    }
    async fn get_object(&self, href: &str) -> ProviderResult<Option<RemoteObject>> {
        Ok(self.0.lock().unwrap().get(href).map(|(e, d)| RemoteObject {
            href: href.into(),
            etag: Some(e.clone()),
            data: d.clone(),
        }))
    }
    async fn delete_object(&self, href: &str, _: Option<&str>) -> ProviderResult<()> {
        self.0.lock().unwrap().remove(href);
        Ok(())
    }
}

/// A store and engine with the fake's Google account signed in.
async fn signed_in(fake: &FakeGoogle) -> (Store, SyncEngine, String) {
    let (store, engine, id, _) = signed_in_with_dav(fake).await;
    (store, engine, id)
}

/// The same, with a CalDAV account "dav" beside it.
async fn signed_in_with_dav(fake: &FakeGoogle) -> (Store, SyncEngine, String, Arc<Dav>) {
    let store = Store::open_in_memory().await.unwrap();
    let api = fake.api();
    let factory_api = api.clone();
    let dav = Arc::new(Dav::default());
    let factory_dav = Arc::clone(&dav);
    let credentials = Arc::new(MemoryCredentials::default());
    credentials.set("dav", "secret").unwrap();
    let engine = SyncEngine::new(
        store.clone(),
        credentials,
        Arc::new(move |account, secret| {
            if account.account.provider == ProviderKind::Caldav {
                return Ok(
                    Box::new(DavHandle(Arc::clone(&factory_dav))) as Box<dyn CalendarProvider>
                );
            }
            let config: calendar_sync::OAuthConfig =
                serde_json::from_value(account.config.clone()).unwrap();
            Ok(Box::new(GoogleProvider::connect(
                &config.client_id,
                secret,
                factory_api.clone(),
            )?) as Box<dyn CalendarProvider>)
        }),
        Arc::new(|_| {}),
    );

    // Sign in (the fake token endpoint issues the valid token).
    let mut client = oauth_client("cid", None);
    client.auth_url = url::Url::parse(&format!("{}/auth", fake.server.uri())).unwrap();
    client.token_url = fake.token_url();
    let id = engine
        .add_oauth_account(
            ProviderKind::Google,
            "cid",
            None,
            browser(),
            &Notify::new(),
            Some((client, api)),
        )
        .await
        .unwrap();
    store
        .create_remote_account(
            &calendar_core::Account {
                id: "dav".into(),
                name: "Nextcloud".into(),
                provider: ProviderKind::Caldav,
            },
            &json!({}),
            &[RemoteCalendarSpec {
                remote_url: DAV_CAL.into(),
                name: "Personal".into(),
                color: None,
                read_only: false,
                supports_sync: false,
                ctag: None,
            }],
        )
        .await
        .unwrap();
    (store, engine, id, dav)
}

/// The provider the engine gets for the CalDAV account (shares the data).
struct DavHandle(Arc<Dav>);

#[async_trait::async_trait]
impl CalendarProvider for DavHandle {
    fn codec(&self) -> Arc<dyn calendar_provider_api::ObjectCodec> {
        self.0.codec()
    }
    async fn list_calendars(&self) -> ProviderResult<Vec<RemoteCalendar>> {
        self.0.list_calendars().await
    }
    async fn fetch_changes(
        &self,
        c: &RemoteCalendar,
        cursor: Option<&str>,
        known: &[(String, Option<String>)],
    ) -> ProviderResult<ChangeSet> {
        self.0.fetch_changes(c, cursor, known).await
    }
    async fn put_object(
        &self,
        href: &str,
        data: &str,
        p: &Precondition,
    ) -> ProviderResult<WriteResult> {
        self.0.put_object(href, data, p).await
    }
    async fn get_object(&self, href: &str) -> ProviderResult<Option<RemoteObject>> {
        self.0.get_object(href).await
    }
    async fn delete_object(&self, href: &str, v: Option<&str>) -> ProviderResult<()> {
        self.0.delete_object(href, v).await
    }
}

#[tokio::test]
async fn events_move_to_nextcloud_and_back() {
    let fake = FakeGoogle::start().await;
    fake.state.lock().unwrap().put(
        "alice@example.com",
        json!({"id": "marci0123", "iCalUID": "marci0123@google.com", "summary": "Marci",
               "start": {"dateTime": "2026-06-12T21:00:00-04:00", "timeZone": "America/Toronto"},
               "end": {"dateTime": "2026-06-12T22:00:00-04:00", "timeZone": "America/Toronto"}}),
    );
    let (store, engine, google, dav) = signed_in_with_dav(&fake).await;
    engine.sync_account(&google).await.unwrap();
    engine.sync_account("dav").await.unwrap();
    let google_cal = store.calendars_of(&google).await.unwrap()[0].id.clone();
    let dav_cal = store.calendars_of("dav").await.unwrap()[0].id.clone();

    // To Nextcloud: the copy arrives, then the original goes.
    let shown = store.query_range(june(), TZ).await.unwrap();
    let moved = store
        .move_event(
            &shown[0].id,
            "local-17b2cabd-0134-42a0-86c3-59f217bf86d0",
            &dav_cal,
        )
        .await
        .unwrap();
    engine.sync_account("dav").await.unwrap();
    engine.sync_account(&google).await.unwrap();
    let problems = engine.problems().await.unwrap();
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(dav.0.lock().unwrap().len(), 1);
    assert_eq!(
        fake.state.lock().unwrap().events["alice@example.com"]["marci0123"].1["status"],
        "cancelled"
    );

    // And back to Google.
    store
        .move_event(
            &moved.id,
            "local-5e1d0c3a-9f7b-4c2e-8a61-0b9e2d4f7a13",
            &google_cal,
        )
        .await
        .unwrap();
    engine.sync_account(&google).await.unwrap();
    engine.sync_account("dav").await.unwrap();
    let problems = engine.problems().await.unwrap();
    assert!(problems.is_empty(), "{problems:?}");
    assert!(dav.0.lock().unwrap().is_empty(), "gone from Nextcloud");
    let shown = store.query_range(june(), TZ).await.unwrap();
    assert_eq!(
        shown
            .iter()
            .map(|o| (o.title.as_str(), o.calendar_id.as_str(), o.sync_status))
            .collect::<Vec<_>>(),
        [(
            "Marci",
            google_cal.as_str(),
            calendar_core::SyncStatus::Synced
        )]
    );
}

#[tokio::test]
async fn google_account_round_trip() {
    let fake = FakeGoogle::start().await;
    fake.state.lock().unwrap().put(
        "alice@example.com",
        json!({
            "id": "abc", "iCalUID": "abc@google.com", "summary": "Stand-up",
            "start": {"dateTime": "2026-06-01T09:00:00+02:00", "timeZone": "Europe/Berlin"},
            "end": {"dateTime": "2026-06-01T09:15:00+02:00", "timeZone": "Europe/Berlin"},
            "recurrence": ["RRULE:FREQ=DAILY;COUNT=3"],
            "attendees": [{"email": "bob@example.com"}]
        }),
    );
    let (store, engine, id) = signed_in(&fake).await;
    let accounts = store.remote_accounts().await.unwrap();
    assert_eq!(accounts[0].account.provider, ProviderKind::Google);
    assert_eq!(accounts[0].account.name, "alice@example.com (Google)");
    let calendars = store.calendars_of(&id).await.unwrap();
    assert_eq!(calendars.len(), 2);
    assert!(calendars[1].read_only);
    let cal = calendars[0].id.clone();

    // Download: three occurrences of the series.
    let report = engine.sync_account(&id).await.unwrap();
    assert_eq!(report.downloaded, 1);
    assert_eq!(titles(&store).await, vec!["Stand-up"; 3]);
    // The series uses the calendar's default reminders (popup ones only).
    assert!(
        store
            .query_range(june(), TZ)
            .await
            .unwrap()
            .iter()
            .all(|o| o.reminders == vec![10])
    );

    // Rename the series locally: only the title travels, attendees stay.
    let master_id = store.query_range(june(), TZ).await.unwrap()[0]
        .series_id
        .clone()
        .unwrap();
    let master = store.get_event(&master_id).await.unwrap().unwrap();
    let mut draft = master.draft();
    draft.title = "Daily".into();
    store.update_event(&master_id, draft.clone()).await.unwrap();
    assert_eq!(engine.sync_account(&id).await.unwrap().uploaded, 1);
    {
        let s = fake.state.lock().unwrap();
        let e = &s.events["alice@example.com"]["abc"].1;
        assert_eq!(e["summary"], "Daily");
        assert_eq!(e["attendees"][0]["email"], "bob@example.com");
        // Unchanged reminders are not sent: the defaults stay in use.
        assert!(e.get("reminders").is_none());
    }

    // Own reminders replace the defaults on the server.
    draft.reminders = Some(vec![0, 30]);
    store.update_event(&master_id, draft).await.unwrap();
    assert_eq!(engine.sync_account(&id).await.unwrap().uploaded, 1);
    assert_eq!(
        fake.state.lock().unwrap().events["alice@example.com"]["abc"].1["reminders"],
        json!({"useDefault": false, "overrides": [
            {"method": "popup", "minutes": 0}, {"method": "popup", "minutes": 30}
        ]})
    );
    engine.sync_account(&id).await.unwrap();
    assert_eq!(
        store
            .get_event(&master_id)
            .await
            .unwrap()
            .unwrap()
            .reminders,
        vec![0, 30]
    );

    // Create locally: uploaded once, not duplicated by the next pass.
    let start = resolve_local(
        &TZ,
        NaiveDate::from_ymd_opt(2026, 6, 12)
            .unwrap()
            .and_hms_opt(14, 0, 0)
            .unwrap(),
    );
    store
        .create_event(
            "local-1",
            EventDraft {
                reminders: None,
                calendar_id: cal.clone(),
                title: "Dentist".into(),
                description: String::new(),
                location: String::new(),
                timing: EventTiming::Zoned {
                    start,
                    end: start + chrono::Duration::hours(1),
                    time_zone: TZ,
                },
                rrule: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(engine.sync_account(&id).await.unwrap().uploaded, 1);
    let again = engine.sync_account(&id).await.unwrap();
    assert_eq!((again.uploaded, again.downloaded), (0, 0));
    assert_eq!(
        fake.state.lock().unwrap().events["alice@example.com"].len(),
        2
    );
    assert_eq!(
        titles(&store).await,
        vec!["Daily", "Daily", "Daily", "Dentist"]
    );

    // Delete locally: cancelled on the server.
    store.delete_event("local-1").await.unwrap();
    engine.sync_account(&id).await.unwrap();
    let cancelled = fake.state.lock().unwrap().events["alice@example.com"]
        .values()
        .filter(|(_, e)| e["status"] == "cancelled")
        .count();
    assert_eq!(cancelled, 1);
    assert_eq!(titles(&store).await, vec!["Daily"; 3]);

    // A change made on the server arrives incrementally.
    {
        let mut s = fake.state.lock().unwrap();
        let mut e = s.events["alice@example.com"]["abc"].1.clone();
        e["summary"] = json!("Daily (moved)");
        s.put("alice@example.com", e);
    }
    assert_eq!(engine.sync_account(&id).await.unwrap().downloaded, 1);
    assert_eq!(titles(&store).await, vec!["Daily (moved)"; 3]);

    // The third occurrence gets its own title; then "this and following"
    // from the second: the series is cut short, the later part (with that
    // exception) becomes a new series, and nothing is duplicated.
    let occ = store.query_range(june(), TZ).await.unwrap();
    let mut third = occ[2].clone();
    third.title = "Last one".into();
    store
        .update_occurrence(
            &third.id,
            EventDraft {
                reminders: None,
                calendar_id: cal.clone(),
                title: third.title.clone(),
                description: String::new(),
                location: String::new(),
                timing: third.timing.clone(),
                rrule: None,
            },
            TZ,
        )
        .await
        .unwrap();
    engine.sync_account(&id).await.unwrap();
    let occ = store.query_range(june(), TZ).await.unwrap();
    store
        .update_following(
            &occ[1].id,
            "local-tail",
            EventDraft {
                reminders: None,
                calendar_id: cal.clone(),
                title: "Later".into(),
                description: String::new(),
                location: String::new(),
                timing: occ[1].timing.clone(),
                rrule: Some("FREQ=DAILY;COUNT=2".into()),
            },
        )
        .await
        .unwrap();
    assert_eq!(engine.sync_account(&id).await.unwrap().uploaded, 2);
    {
        let s = fake.state.lock().unwrap();
        let events = &s.events["alice@example.com"];
        assert_eq!(
            events["abc"].1["recurrence"],
            json!(["RRULE:FREQ=DAILY;COUNT=1"])
        );
        assert_eq!(events["abc_20260603T070000Z"].1["status"], "cancelled");
        let tail = events
            .values()
            .find(|(_, e)| e["summary"] == "Later" && e.get("recurringEventId").is_none())
            .unwrap();
        assert_eq!(tail.1["recurrence"], json!(["RRULE:FREQ=DAILY;COUNT=2"]));
        let tail_id = tail.1["id"].as_str().unwrap();
        assert!(
            events
                .values()
                .any(|(_, e)| e["recurringEventId"] == tail_id
                    && e["summary"] == "Last one"
                    && e["status"] != "cancelled")
        );
    }
    engine.sync_account(&id).await.unwrap();
    let again = engine.sync_account(&id).await.unwrap();
    assert_eq!((again.uploaded, again.downloaded), (0, 0));
    assert_eq!(
        titles(&store).await,
        vec!["Daily (moved)", "Last one", "Later"]
    );
}

#[tokio::test]
async fn events_move_between_google_calendars() {
    let fake = FakeGoogle::start().await;
    {
        let mut s = fake.state.lock().unwrap();
        s.team_calendar = true;
        s.put(
            "alice@example.com",
            json!({
                "id": "abc", "iCalUID": "abc@google.com", "summary": "Stand-up",
                "start": {"dateTime": "2026-06-01T09:00:00+02:00", "timeZone": "Europe/Berlin"},
                "end": {"dateTime": "2026-06-01T09:15:00+02:00", "timeZone": "Europe/Berlin"},
                "recurrence": ["RRULE:FREQ=DAILY;COUNT=3"],
                "attendees": [{"email": "bob@example.com"}]
            }),
        );
        s.put(
            "alice@example.com",
            json!({"id": "inv", "iCalUID": "inv", "summary": "Bob's meeting",
            "organizer": {"email": "bob@example.com", "self": false},
            "start": {"date": "2026-06-10"}, "end": {"date": "2026-06-11"}}),
        );
    }
    let (store, engine, id) = signed_in(&fake).await;
    engine.sync_account(&id).await.unwrap();
    let calendars = store.calendars_of(&id).await.unwrap();
    let team = calendars
        .iter()
        .find(|c| c.name == "Team")
        .unwrap()
        .id
        .clone();
    let shown = store.query_range(june(), TZ).await.unwrap();
    let series = shown[0].series_id.clone().unwrap();
    let invitation = shown
        .iter()
        .find(|o| o.title == "Bob's meeting")
        .unwrap()
        .id
        .clone();

    store.move_event(&series, "unused", &team).await.unwrap();
    store.move_event(&invitation, "copy", &team).await.unwrap();
    engine.sync_account(&id).await.unwrap();
    {
        let s = fake.state.lock().unwrap();
        let moved = &s.events["team@group"]["abc"].1;
        assert_eq!(
            moved["attendees"][0]["email"], "bob@example.com",
            "the same event"
        );
        assert_eq!(
            s.events["alice@example.com"]["abc"].1["status"],
            "cancelled"
        );
        // The invitation couldn't move: a copy, and the original deleted.
        assert_eq!(
            s.events["alice@example.com"]["inv"].1["status"],
            "cancelled"
        );
        assert_eq!(s.events["team@group"].len(), 2);
    }
    let again = engine.sync_account(&id).await.unwrap();
    assert_eq!((again.uploaded, again.conflicts), (0, 0));
    let shown = store.query_range(june(), TZ).await.unwrap();
    assert_eq!(shown.len(), 4, "three stand-ups and the meeting, once each");
    assert!(shown.iter().all(|o| o.calendar_id == team));
    assert!(
        shown
            .iter()
            .all(|o| o.sync_status == calendar_core::SyncStatus::Synced)
    );
    assert_eq!(
        shown
            .iter()
            .filter(|o| o.series_id.as_deref() == Some(series.as_str()))
            .count(),
        3
    );
}
