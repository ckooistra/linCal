//! The Google provider against a stateful fake of the Calendar API.

use std::sync::Arc;

use calendar_core::{EventTiming, OccurrenceKey};
use calendar_google::testing::FakeGoogle;
use calendar_google::{GoogleCodec, GoogleProvider, oauth_client};
use calendar_oauth::{TokenSource, Tokens};
use calendar_provider_api::{
    CalendarProvider, ObjectCodec, Precondition, ProviderError, RemoteCalendar,
};
use serde_json::json;

fn provider(fake: &FakeGoogle, access_token: &str, expires_at_ms: i64) -> GoogleProvider {
    let mut client = oauth_client("client-1", None);
    client.token_url = fake.token_url();
    let tokens = TokenSource::new(
        client,
        Tokens {
            access_token: access_token.into(),
            refresh_token: Some("refresh-1".into()),
            expires_at_ms,
        },
    );
    GoogleProvider::new(Arc::new(tokens), fake.api()).unwrap()
}

const FAR: i64 = i64::MAX / 2;

fn standup() -> serde_json::Value {
    json!({
        "id": "abc", "iCalUID": "abc@google.com", "summary": "Stand-up",
        "start": {"dateTime": "2026-06-01T09:00:00+02:00", "timeZone": "Europe/Berlin"},
        "end": {"dateTime": "2026-06-01T09:15:00+02:00", "timeZone": "Europe/Berlin"},
        "recurrence": ["RRULE:FREQ=DAILY;COUNT=5"],
        "attendees": [{"email": "bob@example.com"}]
    })
}

async fn primary(p: &GoogleProvider) -> RemoteCalendar {
    p.list_calendars().await.unwrap().remove(0)
}

#[tokio::test]
async fn lists_calendars_with_access_levels() {
    let fake = FakeGoogle::start().await;
    let p = provider(&fake, "token-1", FAR);
    let cals = p.list_calendars().await.unwrap();
    let names: Vec<_> = cals
        .iter()
        .map(|c| (c.name.as_str(), c.read_only))
        .collect();
    assert_eq!(names, vec![("Alice", false), ("Holidays", true)]);
    assert_eq!(cals[0].color.as_deref(), Some("#9fc6e7"));
    assert_eq!(p.account_email().await.unwrap(), "alice@example.com");
}

#[tokio::test]
async fn full_then_incremental_sync_groups_series() {
    let fake = FakeGoogle::start().await;
    {
        let mut s = fake.state.lock().unwrap();
        s.page_size = 1; // exercise paging
        s.put("alice@example.com", standup());
        s.put(
            "alice@example.com",
            json!({"id": "one", "iCalUID": "one", "summary": "Dentist",
            "start": {"date": "2026-06-10"}, "end": {"date": "2026-06-11"}}),
        );
    }
    let p = provider(&fake, "token-1", FAR);
    let cal = primary(&p).await;
    let full = p.fetch_changes(&cal, None, &[]).await.unwrap();
    assert_eq!(full.upserted.len(), 2);
    let token = full.cursor.clone().unwrap();
    let known: Vec<_> = full
        .upserted
        .iter()
        .map(|o| (o.href.clone(), o.etag.clone()))
        .collect();

    // An exception changes and the single event is deleted elsewhere.
    {
        let mut s = fake.state.lock().unwrap();
        s.put("alice@example.com", json!({"id": "abc_20260603T070000Z", "iCalUID": "abc@google.com",
            "recurringEventId": "abc", "summary": "Late stand-up",
            "originalStartTime": {"dateTime": "2026-06-03T09:00:00+02:00", "timeZone": "Europe/Berlin"},
            "start": {"dateTime": "2026-06-03T11:00:00+02:00"}, "end": {"dateTime": "2026-06-03T11:15:00+02:00"}}));
        let mut one = s.events["alice@example.com"]["one"].1.clone();
        one["status"] = json!("cancelled");
        s.put("alice@example.com", one);
    }
    let delta = p.fetch_changes(&cal, Some(&token), &known).await.unwrap();
    assert_eq!(delta.upserted.len(), 1);
    assert!(delta.deleted[0].ends_with("/events/one"));
    let series = GoogleCodec.read(&delta.upserted[0].data).unwrap().content;
    assert_eq!(series.master.unwrap().title, "Stand-up");
    assert_eq!(series.overrides[0].title, "Late stand-up");

    // An expired token falls back to a full listing.
    fake.state.lock().unwrap().min_token = u64::MAX;
    let again = p.fetch_changes(&cal, Some(&token), &known).await.unwrap();
    assert!(again.deleted.iter().any(|h| h.ends_with("/events/one")));
    assert!(again.cursor.is_some());
}

#[tokio::test]
async fn creates_patches_and_deletes_conditionally() {
    let fake = FakeGoogle::start().await;
    let p = provider(&fake, "token-1", FAR);
    let cal = primary(&p).await;

    // Create from a locally made object; the provider picks the id.
    let content = GoogleCodec
        .read(&json!({"events": [standup()]}).to_string())
        .unwrap()
        .content;
    let payload = GoogleCodec.write(None, "local-1", &content, &[]);
    let created = p
        .put_object(
            &format!("{}local-1.ics", cal.href),
            &payload,
            &Precondition::Create,
        )
        .await
        .unwrap();
    assert!(created.href.contains("/events/uc"), "{}", created.href);
    // A retried create does not duplicate.
    assert_eq!(
        p.put_object(
            &format!("{}local-1.ics", cal.href),
            &payload,
            &Precondition::Create
        )
        .await
        .unwrap_err(),
        ProviderError::PreconditionFailed
    );

    // Edit the title and one occurrence: only those fields travel.
    let base = created.data.clone().unwrap();
    let mut edited = GoogleCodec.read(&base).unwrap().content;
    edited.master.as_mut().unwrap().title = "Daily".into();
    let mut occurrence = edited.master.clone().unwrap();
    occurrence.recurrence = None;
    let key = OccurrenceKey::Instant(
        chrono::DateTime::parse_from_rfc3339("2026-06-02T07:00:00Z")
            .unwrap()
            .into(),
    );
    occurrence.recurrence_id = Some(key);
    occurrence.timing = match occurrence.timing {
        EventTiming::Zoned { time_zone, .. } => EventTiming::Zoned {
            start: chrono::DateTime::parse_from_rfc3339("2026-06-02T08:00:00Z")
                .unwrap()
                .into(),
            end: chrono::DateTime::parse_from_rfc3339("2026-06-02T08:15:00Z")
                .unwrap()
                .into(),
            time_zone,
        },
        other => other,
    };
    edited.overrides.push(occurrence);
    let payload = GoogleCodec.write(Some(&base), "local-1", &edited, &[]);
    let updated = p
        .put_object(
            &created.href,
            &payload,
            &Precondition::Version(created.etag.clone().unwrap()),
        )
        .await
        .unwrap();
    let after = GoogleCodec
        .read(&updated.data.clone().unwrap())
        .unwrap()
        .content;
    assert_eq!(after.master.unwrap().title, "Daily");
    assert_eq!(after.overrides.len(), 1);

    // A stale version is refused; deleting checks the version too.
    let stale = GoogleCodec.write(Some(&base), "local-1", &edited, &[]);
    assert_eq!(
        p.put_object(
            &created.href,
            &stale,
            &Precondition::Version(created.etag.clone().unwrap())
        )
        .await
        .unwrap_err(),
        ProviderError::PreconditionFailed
    );
    assert_eq!(
        p.delete_object(&created.href, created.etag.as_deref())
            .await
            .unwrap_err(),
        ProviderError::PreconditionFailed
    );
    p.delete_object(&created.href, updated.etag.as_deref())
        .await
        .unwrap();
    assert!(p.get_object(&created.href).await.unwrap().is_none());
}

#[tokio::test]
async fn edits_keep_fields_this_app_does_not_model() {
    let fake = FakeGoogle::start().await;
    fake.state
        .lock()
        .unwrap()
        .put("alice@example.com", standup());
    let p = provider(&fake, "token-1", FAR);
    let cal = primary(&p).await;
    let href = format!("{}events/abc", cal.href);
    let current = p.get_object(&href).await.unwrap().unwrap();
    let mut edited = GoogleCodec.read(&current.data).unwrap().content;
    edited.master.as_mut().unwrap().location = "Room 4".into();
    let payload = GoogleCodec.write(Some(&current.data), "abc@google.com", &edited, &[]);
    p.put_object(
        &href,
        &payload,
        &Precondition::Version(current.etag.unwrap()),
    )
    .await
    .unwrap();
    let s = fake.state.lock().unwrap();
    let master = &s.events["alice@example.com"]["abc"].1;
    assert_eq!(master["location"], "Room 4");
    assert_eq!(master["attendees"][0]["email"], "bob@example.com");
    assert_eq!(master["recurrence"][0], "RRULE:FREQ=DAILY;COUNT=5");
}

#[tokio::test]
async fn expired_access_tokens_are_refreshed_and_reported() {
    let fake = FakeGoogle::start().await;
    fake.state.lock().unwrap().valid_token = "token-2".into();
    // Not expired by the clock, but rejected by the server: refresh once.
    let p = provider(&fake, "token-1", FAR);
    assert_eq!(p.list_calendars().await.unwrap().len(), 2);
    let saved: calendar_google::GoogleSecret =
        serde_json::from_str(&p.take_updated_secret().unwrap()).unwrap();
    assert_eq!(saved.tokens.access_token, "token-2");
    assert_eq!(saved.tokens.refresh_token.as_deref(), Some("refresh-1"));
}
