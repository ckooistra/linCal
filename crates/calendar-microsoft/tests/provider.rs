//! The Graph provider against a stateful fake of the calendar API.

use std::sync::Arc;

use calendar_microsoft::testing::{FakeGraph, event};
use calendar_microsoft::{GraphCodec, GraphProvider, GraphSecret, oauth_client};
use calendar_oauth::{TokenSource, Tokens};
use calendar_provider_api::{
    CalendarProvider, ObjectCodec, Precondition, ProviderError, RemoteCalendar,
};
use chrono::NaiveDate;

fn provider(fake: &FakeGraph, access_token: &str) -> GraphProvider {
    let mut client = oauth_client("client-1");
    client.token_url = fake.token_url();
    let tokens = TokenSource::new(
        client,
        Tokens {
            access_token: access_token.into(),
            refresh_token: Some("refresh-1".into()),
            expires_at_ms: i64::MAX / 2,
        },
    );
    GraphProvider::new(Arc::new(tokens), fake.api())
        .unwrap()
        .with_today(|| NaiveDate::from_ymd_opt(2026, 6, 15).unwrap())
}

async fn primary(p: &GraphProvider) -> RemoteCalendar {
    p.list_calendars().await.unwrap().remove(0)
}

#[tokio::test]
async fn lists_calendars_and_the_account() {
    let fake = FakeGraph::start().await;
    let p = provider(&fake, "token-1");
    let cals = p.list_calendars().await.unwrap();
    let summary: Vec<_> = cals
        .iter()
        .map(|c| (c.name.as_str(), c.read_only))
        .collect();
    assert_eq!(summary, vec![("Calendar", false), ("Team", true)]);
    assert_eq!(cals[0].color.as_deref(), Some("#aabbcc"));
    assert_eq!(p.account_email().await.unwrap(), "carol@example.com");
}

#[tokio::test]
async fn delta_sync_reports_changes_and_removals() {
    let fake = FakeGraph::start().await;
    {
        let mut s = fake.state.lock().unwrap();
        s.page_size = 1;
        s.put(
            "cal-1",
            event(
                "A",
                "Planning",
                "2026-06-01T07:00:00",
                "2026-06-01T08:00:00",
            ),
        );
        s.put(
            "cal-1",
            event("B", "Review", "2026-06-02T07:00:00", "2026-06-02T08:00:00"),
        );
    }
    let p = provider(&fake, "token-1");
    let cal = primary(&p).await;
    let full = p.fetch_changes(&cal, None, &[]).await.unwrap();
    assert_eq!(full.upserted.len(), 2);
    let cursor = full.cursor.clone().unwrap();
    assert!(cursor.contains("2025-06-01"), "{cursor}");
    let known: Vec<_> = full
        .upserted
        .iter()
        .map(|o| (o.href.clone(), o.etag.clone()))
        .collect();

    {
        let mut s = fake.state.lock().unwrap();
        let mut a = s.events["cal-1"]["A"].1.clone();
        a["subject"] = serde_json::json!("Planning (moved)");
        s.put("cal-1", a);
        s.remove("cal-1", "B");
    }
    let delta = p.fetch_changes(&cal, Some(&cursor), &known).await.unwrap();
    assert_eq!(delta.upserted.len(), 1);
    assert_eq!(
        GraphCodec
            .read(&delta.upserted[0].data)
            .unwrap()
            .content
            .master
            .unwrap()
            .title,
        "Planning (moved)"
    );
    assert!(delta.deleted[0].ends_with("/events/B"));

    // Expired delta state: the window is listed again.
    fake.state.lock().unwrap().min_token = u64::MAX;
    let known: Vec<_> = known
        .into_iter()
        .filter(|(h, _)| h.ends_with("/A"))
        .collect();
    let again = p
        .fetch_changes(&cal, Some(&delta.cursor.unwrap()), &known)
        .await
        .unwrap();
    assert_eq!(again.upserted.len(), 1);
    assert!(again.deleted.is_empty());
}

#[tokio::test]
async fn the_window_moves_forward_with_a_full_listing() {
    let fake = FakeGraph::start().await;
    fake.state.lock().unwrap().put(
        "cal-1",
        event(
            "A",
            "Planning",
            "2026-06-01T07:00:00",
            "2026-06-01T08:00:00",
        ),
    );
    let p = provider(&fake, "token-1");
    let cal = primary(&p).await;
    let first = p.fetch_changes(&cal, None, &[]).await.unwrap();
    let known: Vec<_> = first
        .upserted
        .iter()
        .map(|o| (o.href.clone(), o.etag.clone()))
        .collect();
    // Two years later the stored window has too little future left.
    let later =
        provider(&fake, "token-1").with_today(|| NaiveDate::from_ymd_opt(2028, 1, 10).unwrap());
    let moved = later
        .fetch_changes(&cal, first.cursor.as_deref(), &known)
        .await
        .unwrap();
    assert!(moved.cursor.unwrap().contains("2027-01-01"));
    // Unchanged events are not downloaded again.
    assert!(moved.upserted.is_empty());
}

#[tokio::test]
async fn creates_patches_and_deletes_conditionally() {
    let fake = FakeGraph::start().await;
    let p = provider(&fake, "token-1");
    let cal = primary(&p).await;
    let new = GraphCodec
        .read(&event("x", "Dentist", "2026-06-12T12:00:00", "2026-06-12T13:00:00").to_string())
        .unwrap()
        .content;
    let payload = GraphCodec.write(None, "local-1", &new, &[]);
    let href = format!("{}local-1.ics", cal.href);
    let created = p
        .put_object(&href, &payload, &Precondition::Create)
        .await
        .unwrap();
    assert!(created.href.ends_with("/events/AAMk1"), "{}", created.href);
    // A retried create (lost answer) returns the same event.
    let retried = p
        .put_object(&href, &payload, &Precondition::Create)
        .await
        .unwrap();
    assert_eq!(retried.href, created.href);
    assert_eq!(fake.state.lock().unwrap().events["cal-1"].len(), 1);

    // Patch only what changed; attendees stay.
    let base = created.data.clone().unwrap();
    let mut edited = GraphCodec.read(&base).unwrap().content;
    edited.master.as_mut().unwrap().location = "Clinic".into();
    let payload = GraphCodec.write(Some(&base), "local-1", &edited, &[]);
    let updated = p
        .put_object(
            &created.href,
            &payload,
            &Precondition::Version(created.etag.clone().unwrap()),
        )
        .await
        .unwrap();
    assert_ne!(updated.etag, created.etag);
    // Stale versions are refused, for writes and deletes.
    assert_eq!(
        p.put_object(
            &created.href,
            &payload,
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
async fn edits_keep_attendees() {
    let fake = FakeGraph::start().await;
    fake.state.lock().unwrap().put(
        "cal-1",
        event(
            "A",
            "Planning",
            "2026-06-01T07:00:00",
            "2026-06-01T08:00:00",
        ),
    );
    let p = provider(&fake, "token-1");
    let cal = primary(&p).await;
    let href = format!("{}events/A", cal.href);
    let current = p.get_object(&href).await.unwrap().unwrap();
    let mut edited = GraphCodec.read(&current.data).unwrap().content;
    edited.master.as_mut().unwrap().title = "Planning v2".into();
    let payload = GraphCodec.write(Some(&current.data), "A", &edited, &[]);
    p.put_object(
        &href,
        &payload,
        &Precondition::Version(current.etag.unwrap()),
    )
    .await
    .unwrap();
    let s = fake.state.lock().unwrap();
    let e = &s.events["cal-1"]["A"].1;
    assert_eq!(e["subject"], "Planning v2");
    assert_eq!(
        e["attendees"][0]["emailAddress"]["address"],
        "dave@example.com"
    );
}

#[tokio::test]
async fn refreshed_tokens_are_reported() {
    let fake = FakeGraph::start().await;
    fake.state.lock().unwrap().valid_token = "token-2".into();
    let p = provider(&fake, "token-1");
    assert_eq!(p.list_calendars().await.unwrap().len(), 2);
    let saved: GraphSecret = serde_json::from_str(&p.take_updated_secret().unwrap()).unwrap();
    assert_eq!(saved.tokens.access_token, "token-2");
}
