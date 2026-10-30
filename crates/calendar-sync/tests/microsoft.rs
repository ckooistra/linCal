//! A Microsoft account through the whole engine against the fake Graph API:
//! sign-in, windowed delta download, edits, server-assigned ids on create,
//! deletes, and refusal of series edits.

use std::sync::Arc;

use calendar_core::timing::resolve_local;
use calendar_core::{EventDraft, EventTiming, ProviderKind, TimeRange};
use calendar_microsoft::testing::{FakeGraph, event};
use calendar_microsoft::{GraphProvider, oauth_client};
use calendar_provider_api::CalendarProvider;
use calendar_storage::{StorageError, Store};
use calendar_sync::{BrowserOpener, MemoryCredentials, SyncEngine};
use chrono::NaiveDate;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Notify;

const TZ: chrono_tz::Tz = chrono_tz::Europe::Berlin;

fn browser() -> BrowserOpener {
    Arc::new(|auth: &str| {
        let auth = url::Url::parse(auth).unwrap();
        let q = |k: &str| {
            auth.query_pairs()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.into_owned())
                .unwrap()
        };
        let redirect = url::Url::parse(&q("redirect_uri")).unwrap();
        assert_eq!(redirect.host_str(), Some("localhost"));
        let port = redirect.port().unwrap();
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

fn draft(calendar: &str, title: &str, rrule: Option<&str>) -> EventDraft {
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
            end: start + chrono::Duration::hours(1),
            time_zone: TZ,
        },
        rrule: rrule.map(Into::into),
    }
}

#[tokio::test]
async fn microsoft_account_round_trip() {
    let fake = FakeGraph::start().await;
    {
        let mut s = fake.state.lock().unwrap();
        s.put(
            "cal-1",
            event(
                "A",
                "Planning",
                "2026-06-01T07:00:00",
                "2026-06-01T08:00:00",
            ),
        );
        let mut occurrence = event(
            "S1",
            "Weekly sync",
            "2026-06-03T08:00:00",
            "2026-06-03T08:30:00",
        );
        occurrence["type"] = serde_json::json!("occurrence");
        occurrence["seriesMasterId"] = serde_json::json!("S");
        s.put("cal-1", occurrence);
        s.put(
            "cal-1",
            serde_json::json!({"id": "S", "type": "seriesMaster", "subject": "Weekly sync"}),
        );
    }
    let store = Store::open_in_memory().await.unwrap();
    let api = fake.api();
    let factory_api = api.clone();
    let engine = SyncEngine::new(
        store.clone(),
        Arc::new(MemoryCredentials::default()),
        Arc::new(move |account, secret| {
            let config: calendar_sync::OAuthConfig =
                serde_json::from_value(account.config.clone()).unwrap();
            Ok(Box::new(GraphProvider::connect(
                &config.client_id,
                secret,
                factory_api.clone(),
            )?) as Box<dyn CalendarProvider>)
        }),
        Arc::new(|_| {}),
    );
    let mut client = oauth_client("cid");
    client.auth_url = url::Url::parse(&format!("{}/auth", fake.server.uri())).unwrap();
    client.token_url = fake.token_url();
    let id = engine
        .add_oauth_account(
            ProviderKind::Microsoft,
            "cid",
            None,
            browser(),
            &Notify::new(),
            Some((client, api)),
        )
        .await
        .unwrap();
    assert_eq!(
        store.remote_accounts().await.unwrap()[0].account.name,
        "carol@example.com (Microsoft)"
    );
    let cal = store.calendars_of(&id).await.unwrap()[0].id.clone();

    // Download: single events and occurrences; the series master is skipped.
    assert_eq!(engine.sync_account(&id).await.unwrap().downloaded, 2);
    assert_eq!(titles(&store).await, vec!["Planning", "Weekly sync"]);
    // The occurrence is flagged as approximated (no whole-series editing).
    let status = store.account_statuses().await.unwrap();
    assert_eq!(
        status
            .iter()
            .find(|s| s.account_id == id)
            .unwrap()
            .approximated,
        1
    );

    // Edit one occurrence: it becomes an exception on the server.
    let occurrence = store
        .query_range(june(), TZ)
        .await
        .unwrap()
        .into_iter()
        .find(|o| o.title == "Weekly sync")
        .unwrap();
    let mut d = store
        .get_event(&occurrence.event_id)
        .await
        .unwrap()
        .unwrap()
        .draft();
    d.title = "Weekly sync (moved)".into();
    store.update_event(&occurrence.event_id, d).await.unwrap();
    assert_eq!(engine.sync_account(&id).await.unwrap().uploaded, 1);
    {
        let s = fake.state.lock().unwrap();
        let e = &s.events["cal-1"]["S1"].1;
        assert_eq!(e["type"], "exception");
        assert_eq!(
            e["attendees"][0]["emailAddress"]["address"],
            "dave@example.com"
        );
    }

    // Create: the server assigns the id; nothing is duplicated.
    store
        .create_event("local-1", draft(&cal, "Dentist", None))
        .await
        .unwrap();
    assert_eq!(engine.sync_account(&id).await.unwrap().uploaded, 1);
    let again = engine.sync_account(&id).await.unwrap();
    assert_eq!((again.uploaded, again.downloaded), (0, 0));
    assert_eq!(
        titles(&store).await,
        vec!["Dentist", "Planning", "Weekly sync (moved)"]
    );

    // Repeating events cannot be created in Microsoft calendars yet.
    assert!(matches!(
        store
            .create_event("local-2", draft(&cal, "Gym", Some("FREQ=WEEKLY")))
            .await,
        Err(StorageError::Unsupported(_))
    ));

    // Delete locally, and a deletion on the server arrives by delta.
    store.delete_event("local-1").await.unwrap();
    engine.sync_account(&id).await.unwrap();
    fake.state.lock().unwrap().remove("cal-1", "A");
    engine.sync_account(&id).await.unwrap();
    assert_eq!(titles(&store).await, vec!["Weekly sync (moved)"]);
    assert_eq!(fake.state.lock().unwrap().events["cal-1"].len(), 2); // S1 and the master S
}
