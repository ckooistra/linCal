//! Optional end-to-end test against a real CalDAV server. Skipped unless
//! configured, e.g. with the local Radicale from `scripts/dev-radicale.sh`:
//!
//!     UC_CALDAV_URL=http://127.0.0.1:5232/ UC_CALDAV_USER=test UC_CALDAV_PASSWORD=test \
//!       cargo test -p calendar-caldav --test real_server -- --nocapture
//!
//! It only touches the calendar named by `UC_CALDAV_CALENDAR` (default
//! "UC Test") and deletes what it creates. Use an app password and a
//! dedicated test calendar on real accounts.

use calendar_caldav::{CalDavProvider, discover};
use calendar_provider_api::{CalendarProvider, Precondition, ProviderError};
use url::Url;

fn config() -> Option<(Url, String, String, String)> {
    let url = Url::parse(&std::env::var("UC_CALDAV_URL").ok()?).ok()?;
    Some((
        url,
        std::env::var("UC_CALDAV_USER").ok()?,
        std::env::var("UC_CALDAV_PASSWORD").ok()?,
        std::env::var("UC_CALDAV_CALENDAR").unwrap_or_else(|_| "UC Test".into()),
    ))
}

#[tokio::test]
async fn round_trip_against_a_real_server() {
    let Some((url, user, password, calendar_name)) = config() else {
        eprintln!("UC_CALDAV_URL not set; skipping real-server test");
        return;
    };
    let d = discover(&url, &user, &password).await.expect("discovery");
    let p = CalDavProvider::new(d.calendar_home, &user, &password).unwrap();
    let calendars = p.list_calendars().await.expect("list calendars");
    let cal = calendars
        .iter()
        .find(|c| c.name == calendar_name)
        .unwrap_or_else(|| {
            panic!(
                "calendar {calendar_name:?} not found in {:?}",
                calendars.iter().map(|c| &c.name).collect::<Vec<_>>()
            )
        })
        .clone();
    assert!(!cal.read_only);

    let before = p
        .fetch_changes(&cal, None, &[])
        .await
        .expect("initial sync");
    let uid = format!("uc-test-{}", std::process::id());
    let href = format!("{}{uid}.ics", cal.href);
    let ics = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//UC test//EN\r\nBEGIN:VEVENT\r\nUID:{uid}\r\nDTSTAMP:20261029T120000Z\r\nDTSTART:20261030T090000Z\r\nDTEND:20261030T100000Z\r\nSUMMARY:UC integration test\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let created = p
        .put_object(&href, &ics, &Precondition::Create)
        .await
        .expect("create");
    assert_eq!(
        p.put_object(&href, &ics, &Precondition::Create).await,
        Err(ProviderError::PreconditionFailed),
        "If-None-Match: * must refuse to overwrite"
    );

    let changes = p
        .fetch_changes(&cal, before.cursor.as_deref(), &[])
        .await
        .expect("delta sync");
    let fetched = changes
        .upserted
        .iter()
        .find(|o| o.href == href)
        .expect("created object is reported");
    assert!(fetched.ical.contains("UC integration test"));
    let etag = fetched.etag.clone().or(created.etag).expect("etag");

    let updated = ics.replace("UC integration test", "UC integration test (edited)");
    let w = p
        .put_object(&href, &updated, &Precondition::Version(etag.clone()))
        .await
        .expect("update");
    assert_eq!(
        p.put_object(&href, &updated, &Precondition::Version(etag))
            .await,
        Err(ProviderError::PreconditionFailed),
        "a stale ETag must be rejected"
    );
    let after = p
        .fetch_changes(&cal, changes.cursor.as_deref(), &[])
        .await
        .expect("delta sync 2");
    let new_etag = after
        .upserted
        .iter()
        .find(|o| o.href == href)
        .and_then(|o| o.etag.clone())
        .or(w.etag);

    p.delete_object(&href, new_etag.as_deref())
        .await
        .expect("delete");
    let gone = p
        .fetch_changes(&cal, after.cursor.as_deref(), &[])
        .await
        .expect("delta sync 3");
    if cal.capabilities.incremental_sync {
        assert!(
            gone.deleted.contains(&href),
            "deletion is reported: {:?}",
            gone.deleted
        );
    }
}
