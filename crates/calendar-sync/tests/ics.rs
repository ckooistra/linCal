//! An ICS subscription through the whole engine against a local feed:
//! subscribing, download, conditional refreshes, changes and removals in
//! the feed, read-only enforcement and a feed that disappears.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use calendar_core::{EventDraft, EventTiming, ProviderKind, TimeRange};
use calendar_ics::IcsProvider;
use calendar_provider_api::CalendarProvider;
use calendar_storage::{StorageError, Store};
use calendar_sync::{CredentialStore, IcsConfig, MemoryCredentials, SyncEngine};
use chrono::NaiveDate;
use tokio::sync::Notify;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate, matchers::path};

const TZ: chrono_tz::Tz = chrono_tz::Europe::Berlin;

fn feed(events: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\nX-WR-CALNAME:Holidays\r\n\
         X-APPLE-CALENDAR-COLOR:#2F9E44\r\n{events}END:VCALENDAR\r\n"
    )
}

fn holiday(uid: &str, date: &str, title: &str) -> String {
    format!(
        "BEGIN:VEVENT\r\nUID:{uid}\r\nDTSTART;VALUE=DATE:{date}\r\nSUMMARY:{title}\r\nEND:VEVENT\r\n"
    )
}

/// The feed body and its ETag; counts full and not-modified answers.
#[derive(Default)]
struct State {
    body: String,
    etag: String,
    full: usize,
    not_modified: usize,
    gone: bool,
}

struct Feed(Arc<Mutex<State>>);

impl Respond for Feed {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let mut s = self.0.lock().unwrap();
        if s.gone {
            return ResponseTemplate::new(404);
        }
        let matches = req
            .headers
            .get("if-none-match")
            .and_then(|v| v.to_str().ok())
            == Some(s.etag.as_str());
        if matches {
            s.not_modified += 1;
            return ResponseTemplate::new(304);
        }
        s.full += 1;
        ResponseTemplate::new(200)
            .insert_header("etag", s.etag.as_str())
            .insert_header("content-type", "text/calendar")
            .set_body_string(s.body.clone())
    }
}

fn year() -> TimeRange {
    let day = |m, d| {
        calendar_core::timing::resolve_local(
            &TZ,
            NaiveDate::from_ymd_opt(2027, m, d)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap(),
        )
    };
    TimeRange::new(day(1, 1), day(12, 31))
}

async fn titles(store: &Store) -> Vec<String> {
    let mut v: Vec<String> = store
        .query_range(year(), TZ)
        .await
        .unwrap()
        .into_iter()
        .map(|o| o.title)
        .collect();
    v.sort();
    v
}

#[tokio::test]
async fn ics_subscription_round_trip() {
    let server = MockServer::start().await;
    let state = Arc::new(Mutex::new(State {
        body: feed(
            &(holiday("ny", "20270101", "New Year") + &holiday("may", "20270501", "Labour Day")),
        ),
        etag: "\"v1\"".into(),
        ..State::default()
    }));
    Mock::given(path("/private/abc123/basic.ics"))
        .respond_with(Feed(Arc::clone(&state)))
        .mount(&server)
        .await;
    let store = Store::open_in_memory().await.unwrap();
    let creds = Arc::new(MemoryCredentials::default());
    let engine = SyncEngine::new(
        store.clone(),
        creds.clone(),
        Arc::new(|account, secret| {
            assert_eq!(account.account.provider, ProviderKind::Ics);
            let config: IcsConfig = serde_json::from_value(account.config.clone()).unwrap();
            Ok(Box::new(
                IcsProvider::new(url::Url::parse(secret).unwrap(), &config.name)?
                    .with_min_refresh(Duration::ZERO),
            ) as Box<dyn CalendarProvider>)
        }),
        Arc::new(|_| {}),
    );

    // webcal:// would mean https; a local test feed is plain http.
    let address = format!("{}/private/abc123/basic.ics", server.uri());
    let id = engine
        .add_ics_subscription(&address, &Notify::new())
        .await
        .unwrap();
    let accounts = store.remote_accounts().await.unwrap();
    assert_eq!(accounts[0].account.name, "127.0.0.1 (subscription)");
    // The (possibly secret) address is the credential, not configuration.
    assert!(!accounts[0].config.to_string().contains("abc123"));
    assert_eq!(creds.get(&id).unwrap().as_deref(), Some(address.as_str()));
    let calendars = store.calendars_of(&id).await.unwrap();
    assert_eq!(calendars.len(), 1);
    assert_eq!(calendars[0].name, "Holidays");
    assert_eq!(calendars[0].color, "#2f9e44");
    assert!(calendars[0].read_only);
    let cal = calendars[0].id.clone();

    let report = engine.sync_account(&id).await.unwrap();
    assert_eq!(report.downloaded, 2);
    assert_eq!(titles(&store).await, vec!["Labour Day", "New Year"]);

    // Read-only.
    let draft = EventDraft {
        calendar_id: cal.clone(),
        title: "Mine".into(),
        description: String::new(),
        location: String::new(),
        timing: EventTiming::AllDay {
            start_date: NaiveDate::from_ymd_opt(2027, 2, 1).unwrap(),
            end_date: NaiveDate::from_ymd_opt(2027, 2, 2).unwrap(),
        },
        rrule: None,
    };
    assert!(matches!(
        store.create_event("mine", draft).await,
        Err(StorageError::ReadOnly(_))
    ));

    // Unchanged: a conditional request, nothing downloaded.
    let again = engine.sync_account(&id).await.unwrap();
    assert_eq!(again.downloaded, 0);
    assert_eq!(state.lock().unwrap().not_modified, 1);

    // The feed changes: one renamed, one removed, one added.
    {
        let mut s = state.lock().unwrap();
        s.body = feed(
            &(holiday("ny", "20270101", "Neujahr") + &holiday("xmas", "20271225", "Christmas")),
        );
        s.etag = "\"v2\"".into();
    }
    let report = engine.sync_account(&id).await.unwrap();
    assert_eq!((report.downloaded, report.deleted), (2, 1));
    assert_eq!(titles(&store).await, vec!["Christmas", "Neujahr"]);

    // The feed disappears: the account reports it, the events stay.
    state.lock().unwrap().gone = true;
    let err = engine.sync_account(&id).await.unwrap_err();
    assert!(err.to_string().contains("no longer exists"), "{err}");
    assert!(!err.to_string().contains("abc123"));
    assert_eq!(titles(&store).await, vec!["Christmas", "Neujahr"]);
}

#[tokio::test]
async fn refuses_what_is_not_a_feed() {
    let server = MockServer::start().await;
    Mock::given(path("/page"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>hello</html>"))
        .mount(&server)
        .await;
    let store = Store::open_in_memory().await.unwrap();
    let engine = SyncEngine::new(
        store.clone(),
        Arc::new(MemoryCredentials::default()),
        calendar_sync::default_factory(),
        Arc::new(|_| {}),
    );
    let err = engine
        .add_ics_subscription(&format!("{}/page", server.uri()), &Notify::new())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not an iCalendar feed"), "{err}");
    assert!(
        engine
            .add_ics_subscription("http://example.org/a.ics", &Notify::new())
            .await
            .is_err()
    );
    assert!(store.remote_accounts().await.unwrap().is_empty());
}
