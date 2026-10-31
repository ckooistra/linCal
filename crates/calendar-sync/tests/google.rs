//! A Google account through the whole engine: OAuth sign-in, download,
//! local edits, creates and deletes, against the fake Calendar API.

use std::sync::Arc;

use calendar_core::timing::resolve_local;
use calendar_core::{EventDraft, EventTiming, ProviderKind, TimeRange};
use calendar_google::testing::FakeGoogle;
use calendar_google::{GoogleProvider, oauth_client};
use calendar_provider_api::CalendarProvider;
use calendar_storage::Store;
use calendar_sync::{BrowserOpener, MemoryCredentials, SyncEngine};
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
    let store = Store::open_in_memory().await.unwrap();
    let api = fake.api();
    let factory_api = api.clone();
    let engine = SyncEngine::new(
        store.clone(),
        Arc::new(MemoryCredentials::default()),
        Arc::new(move |account, secret| {
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
