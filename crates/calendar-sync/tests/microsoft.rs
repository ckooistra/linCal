//! A Microsoft account through the whole engine against the fake Graph API:
//! sign-in, windowed delta download, edits, server-assigned ids on create,
//! deletes, and whole series with exceptions and cancelled occurrences.

use std::sync::Arc;

use calendar_core::timing::resolve_local;
use calendar_core::{EventDraft, EventTiming, Occurrence, ProviderKind, TimeRange};
use calendar_microsoft::testing::{FakeGraph, event};
use calendar_microsoft::{GraphProvider, oauth_client};
use calendar_provider_api::CalendarProvider;
use calendar_storage::{StorageError, Store};
use calendar_sync::{BrowserOpener, MemoryCredentials, SyncEngine};
use chrono::NaiveDate;
use serde_json::json;
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

async fn occurrences(store: &Store) -> Vec<Occurrence> {
    let mut v = store.query_range(june(), TZ).await.unwrap();
    v.sort_by_key(|o| o.timing.start_key());
    v
}

/// (day of June, title) of everything shown in June.
async fn shown(store: &Store) -> Vec<(u32, String)> {
    use chrono::Datelike;
    occurrences(store)
        .await
        .into_iter()
        .map(|o| {
            let EventTiming::Zoned { start, .. } = o.timing else {
                panic!("timed events only")
            };
            (start.with_timezone(&TZ).day(), o.title)
        })
        .collect()
}

fn draft(calendar: &str, title: &str, day: u32, rrule: Option<&str>) -> EventDraft {
    let start = resolve_local(
        &TZ,
        NaiveDate::from_ymd_opt(2026, 6, day)
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
            end: start + chrono::Duration::hours(1),
            time_zone: TZ,
        },
        rrule: rrule.map(Into::into),
    }
}

fn as_draft(o: &Occurrence) -> EventDraft {
    EventDraft {
        reminders: None,
        calendar_id: o.calendar_id.clone(),
        title: o.title.clone(),
        description: o.description.clone(),
        location: o.location.clone(),
        timing: o.timing.clone(),
        rrule: o.rrule.clone(),
    }
}

fn s(day: u32, title: &str) -> (u32, String) {
    (day, title.to_owned())
}

#[tokio::test]
async fn microsoft_account_round_trip() {
    let fake = FakeGraph::start().await;
    {
        let mut st = fake.state.lock().unwrap();
        st.put(
            "cal-1",
            event(
                "A",
                "Planning",
                "2026-06-01T07:00:00",
                "2026-06-01T08:00:00",
            ),
        );
        // Daily at 10:00 Berlin, 1–5 June.
        let mut master = event(
            "S",
            "Stand-up",
            "2026-06-01T08:00:00",
            "2026-06-01T08:15:00",
        );
        master["type"] = json!("seriesMaster");
        master["recurrence"] = json!({
            "pattern": {"type": "daily", "interval": 1},
            "range": {"type": "numbered", "startDate": "2026-06-01", "numberOfOccurrences": 5}
        });
        st.put("cal-1", master);
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

    // Download: the single event, and the series as one object.
    assert_eq!(engine.sync_account(&id).await.unwrap().downloaded, 2);
    assert_eq!(
        shown(&store).await,
        vec![
            s(1, "Planning"),
            s(1, "Stand-up"),
            s(2, "Stand-up"),
            s(3, "Stand-up"),
            s(4, "Stand-up"),
            s(5, "Stand-up"),
        ]
    );
    let status = store.account_statuses().await.unwrap();
    assert_eq!(
        status
            .iter()
            .find(|s| s.account_id == id)
            .unwrap()
            .approximated,
        0
    );

    // Reminders on the single event: Outlook holds one.
    let planning = occurrences(&store)
        .await
        .into_iter()
        .find(|o| o.title == "Planning")
        .unwrap();
    let mut d = as_draft(&planning);
    d.reminders = Some(vec![5, 60]);
    assert!(matches!(
        store.update_event(&planning.event_id, d.clone()).await,
        Err(StorageError::TooManyReminders(1))
    ));
    d.reminders = Some(vec![60]);
    store.update_event(&planning.event_id, d).await.unwrap();
    assert_eq!(engine.sync_account(&id).await.unwrap().uploaded, 1);
    {
        let st = fake.state.lock().unwrap();
        let a = &st.events["cal-1"]["A"].1;
        assert_eq!(a["isReminderOn"], true);
        assert_eq!(a["reminderMinutesBeforeStart"], 60);
    }
    assert_eq!(
        store
            .get_event(&planning.event_id)
            .await
            .unwrap()
            .unwrap()
            .reminders,
        vec![60]
    );

    // Edit one occurrence (it becomes an exception) and cancel another.
    let series: Vec<Occurrence> = occurrences(&store)
        .await
        .into_iter()
        .filter(|o| o.series_id.is_some())
        .collect();
    let mut d = as_draft(&series[2]);
    d.title = "Stand-up (moved)".into();
    store.update_occurrence(&series[2].id, d, TZ).await.unwrap();
    store.delete_occurrence(&series[3].id).await.unwrap();
    assert_eq!(engine.sync_account(&id).await.unwrap().uploaded, 1);
    {
        let st = fake.state.lock().unwrap();
        let x = &st.events["cal-1"]["S.2026-06-03"].1;
        assert_eq!(x["type"], "exception");
        assert_eq!(x["subject"], "Stand-up (moved)");
        assert_eq!(st.cancelled["S"], vec!["2026-06-04".to_owned()]);
    }

    // Rename the whole series; the exception keeps its own title.
    let first = occurrences(&store)
        .await
        .into_iter()
        .find(|o| o.title == "Stand-up")
        .unwrap();
    let mut d = as_draft(&first);
    d.title = "Daily".into();
    store.update_series(&first.id, d).await.unwrap();
    engine.sync_account(&id).await.unwrap();
    assert_eq!(
        fake.state.lock().unwrap().events["cal-1"]["S"].1["subject"],
        "Daily"
    );
    // The next sync reads back what the server made of it; nothing changes.
    engine.sync_account(&id).await.unwrap();
    let again = engine.sync_account(&id).await.unwrap();
    assert_eq!((again.uploaded, again.downloaded), (0, 0));
    assert_eq!(
        shown(&store).await,
        vec![
            s(1, "Planning"),
            s(1, "Daily"),
            s(2, "Daily"),
            s(3, "Stand-up (moved)"),
            s(5, "Daily"),
        ]
    );

    // Another client cancels an occurrence: the series is read again.
    {
        let mut st = fake.state.lock().unwrap();
        st.cancelled.get_mut("S").unwrap().push("2026-06-02".into());
        st.seq += 1;
        let seq = st.seq;
        st.removed
            .entry("cal-1".into())
            .or_default()
            .push((seq, "S.2026-06-02".into()));
    }
    engine.sync_account(&id).await.unwrap();
    assert!(!shown(&store).await.contains(&s(2, "Daily")));

    // "This and following" from the moved 3 June occurrence: the series
    // keeps 1–2 June, a new series takes 3–5 June (with 4 June still
    // cancelled), and the old exception is removed from the first series.
    let moved = occurrences(&store)
        .await
        .into_iter()
        .find(|o| o.title == "Stand-up (moved)")
        .unwrap();
    let mut d = as_draft(&moved);
    d.title = "Daily (new)".into();
    d.rrule = Some("FREQ=DAILY;COUNT=3".into());
    store
        .update_following(&moved.id, "local-tail", d)
        .await
        .unwrap();
    assert_eq!(engine.sync_account(&id).await.unwrap().uploaded, 2);
    {
        let st = fake.state.lock().unwrap();
        let events = &st.events["cal-1"];
        assert_eq!(
            events["S"].1["recurrence"]["range"]["numberOfOccurrences"],
            2
        );
        assert!(!events.contains_key("S.2026-06-03"));
        let (tail_id, tail) = events
            .iter()
            .find(|(_, (_, e))| e["subject"] == "Daily (new)" && e["type"] == "seriesMaster")
            .map(|(id, (_, e))| (id.clone(), e.clone()))
            .unwrap();
        assert_eq!(tail["recurrence"]["range"]["numberOfOccurrences"], 3);
        assert_eq!(st.cancelled[&tail_id], vec!["2026-06-04".to_owned()]);
    }
    engine.sync_account(&id).await.unwrap();
    let again = engine.sync_account(&id).await.unwrap();
    assert_eq!((again.uploaded, again.downloaded), (0, 0));
    assert_eq!(
        shown(&store).await,
        vec![
            s(1, "Planning"),
            s(1, "Daily"),
            s(3, "Daily (new)"),
            s(5, "Daily (new)"),
        ]
    );
    let tail_first = occurrences(&store)
        .await
        .into_iter()
        .find(|o| o.title == "Daily (new)")
        .unwrap();

    // A new weekly series: created with its pattern; nothing is duplicated.
    store
        .create_event("local-1", draft(&cal, "Gym", 12, Some("FREQ=WEEKLY")))
        .await
        .unwrap();
    assert_eq!(engine.sync_account(&id).await.unwrap().uploaded, 1);
    {
        let st = fake.state.lock().unwrap();
        let gym = st.events["cal-1"]
            .values()
            .find(|(_, e)| e["subject"] == "Gym")
            .unwrap();
        assert_eq!(gym.1["type"], "seriesMaster");
        assert_eq!(
            gym.1["recurrence"]["pattern"]["daysOfWeek"],
            json!(["friday"])
        );
        assert_eq!(gym.1["originalStartTimeZone"], "W. Europe Standard Time");
        assert_eq!(gym.1["start"]["dateTime"], "2026-06-12T12:00:00.0000000");
    }
    engine.sync_account(&id).await.unwrap();
    let again = engine.sync_account(&id).await.unwrap();
    assert_eq!((again.uploaded, again.downloaded), (0, 0));
    let gyms: Vec<_> = shown(&store)
        .await
        .into_iter()
        .filter(|(_, t)| t == "Gym")
        .collect();
    assert_eq!(gyms, vec![s(12, "Gym"), s(19, "Gym"), s(26, "Gym")]);
    // The rule reads back as the editor's "Weekly" preset.
    let gym = occurrences(&store)
        .await
        .into_iter()
        .find(|o| o.title == "Gym")
        .unwrap();
    assert_eq!(gym.rrule.as_deref(), Some("FREQ=WEEKLY"));

    // Rules Outlook cannot hold are refused before anything is stored.
    assert!(matches!(
        store
            .create_event(
                "local-2",
                draft(&cal, "Rent", 1, Some("FREQ=MONTHLY;BYMONTHDAY=1,15"))
            )
            .await,
        Err(StorageError::UnsupportedRecurrence(_))
    ));

    // Delete the series and the single event; a server-side deletion of
    // the gym series arrives by delta.
    store.delete_series(&first.id).await.unwrap();
    store.delete_series(&tail_first.id).await.unwrap();
    let planning = occurrences(&store)
        .await
        .into_iter()
        .find(|o| o.title == "Planning")
        .unwrap();
    store.delete_event(&planning.event_id).await.unwrap();
    engine.sync_account(&id).await.unwrap();
    {
        let st = fake.state.lock().unwrap();
        assert!(!st.events["cal-1"].contains_key("S"));
        assert!(!st.events["cal-1"].contains_key("S.2026-06-03"));
        assert!(!st.events["cal-1"].contains_key("A"));
    }
    let gym_id = fake.state.lock().unwrap().events["cal-1"]
        .iter()
        .find(|(_, (_, e))| e["subject"] == "Gym")
        .map(|(id, _)| id.clone())
        .unwrap();
    fake.state.lock().unwrap().remove("cal-1", &gym_id);
    engine.sync_account(&id).await.unwrap();
    assert_eq!(shown(&store).await, vec![]);
}
