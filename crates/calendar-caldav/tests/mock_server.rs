//! Protocol tests against a mock server replaying Nextcloud-shaped responses.

use calendar_caldav::{CalDavProvider, discover};
use calendar_provider_api::{CalendarProvider, Precondition, ProviderError};
use url::Url;
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn ms(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(207).set_body_raw(
        format!(r#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:" xmlns:cal="urn:ietf:params:xml:ns:caldav" xmlns:cs="http://calendarserver.org/ns/" xmlns:x1="http://apple.com/ns/ical/" xmlns:oc="http://owncloud.org/ns">{body}</d:multistatus>"#),
        "application/xml",
    )
}

const EVENT: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:e1\r\nDTSTART:20261029T080000Z\r\nDTEND:20261029T090000Z\r\nSUMMARY:Review\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

async fn nextcloud() -> MockServer {
    let server = MockServer::start().await;
    // The user enters the bare server URL; discovery goes via .well-known.
    Mock::given(method("PROPFIND"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("PROPFIND"))
        .and(path("/.well-known/caldav"))
        .respond_with(ResponseTemplate::new(301).insert_header("Location", "/remote.php/dav/"))
        .mount(&server)
        .await;
    Mock::given(method("PROPFIND")).and(path("/remote.php/dav/")).and(body_string_contains("current-user-principal"))
        .respond_with(ms(r#"<d:response><d:href>/remote.php/dav/</d:href><d:propstat><d:prop><d:current-user-principal><d:href>/remote.php/dav/principals/users/alice/</d:href></d:current-user-principal></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"#))
        .mount(&server).await;
    Mock::given(method("PROPFIND")).and(path("/remote.php/dav/principals/users/alice/"))
        .respond_with(ms(r#"<d:response><d:href>/remote.php/dav/principals/users/alice/</d:href><d:propstat><d:prop><cal:calendar-home-set><d:href>/remote.php/dav/calendars/alice/</d:href></cal:calendar-home-set></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"#))
        .mount(&server).await;
    Mock::given(method("PROPFIND")).and(path("/remote.php/dav/calendars/alice/")).and(header("Depth", "1"))
        .respond_with(ms(r#"
<d:response><d:href>/remote.php/dav/calendars/alice/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
<d:response><d:href>/remote.php/dav/calendars/alice/personal/</d:href><d:propstat><d:prop>
  <d:resourcetype><d:collection/><cal:calendar/></d:resourcetype><d:displayname>Personal</d:displayname>
  <x1:calendar-color>#0082C9FF</x1:calendar-color><cs:getctag>http://sabre.io/ns/sync/7</cs:getctag><d:sync-token>http://sabre.io/ns/sync/7</d:sync-token>
  <cal:supported-calendar-component-set><cal:comp name="VEVENT"/></cal:supported-calendar-component-set>
  <d:current-user-privilege-set><d:privilege><d:read/></d:privilege><d:privilege><d:write/></d:privilege></d:current-user-privilege-set>
</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
<d:response><d:href>/remote.php/dav/calendars/alice/holidays_shared_by_bob/</d:href><d:propstat><d:prop>
  <d:resourcetype><d:collection/><cal:calendar/><oc:shared/></d:resourcetype><d:displayname>Holidays</d:displayname>
  <cal:supported-calendar-component-set><cal:comp name="VEVENT"/></cal:supported-calendar-component-set>
  <d:current-user-privilege-set><d:privilege><d:read/></d:privilege></d:current-user-privilege-set>
</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
<d:response><d:href>/remote.php/dav/calendars/alice/tasks/</d:href><d:propstat><d:prop>
  <d:resourcetype><d:collection/><cal:calendar/></d:resourcetype><d:displayname>Tasks</d:displayname>
  <cal:supported-calendar-component-set><cal:comp name="VTODO"/></cal:supported-calendar-component-set>
</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
<d:response><d:href>/remote.php/dav/calendars/alice/inbox/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><cal:schedule-inbox/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"#))
        .mount(&server).await;
    server
}

async fn provider(server: &MockServer) -> CalDavProvider {
    let base = Url::parse(&server.uri()).unwrap();
    let d = discover(&base, "alice", "app-password").await.unwrap();
    CalDavProvider::new(d.calendar_home, "alice", "app-password").unwrap()
}

#[tokio::test]
async fn discovers_and_lists_event_calendars_with_permissions() {
    let server = nextcloud().await;
    let base = Url::parse(&server.uri()).unwrap();
    let d = discover(&base, "alice", "app-password").await.unwrap();
    assert_eq!(
        d.principal.path(),
        "/remote.php/dav/principals/users/alice/"
    );
    assert_eq!(d.calendar_home.path(), "/remote.php/dav/calendars/alice/");

    let p = CalDavProvider::new(d.calendar_home, "alice", "app-password").unwrap();
    let cals = p.list_calendars().await.unwrap();
    assert_eq!(
        cals.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        vec!["Personal", "Holidays"]
    );
    assert_eq!(cals[0].color.as_deref(), Some("#0082c9"));
    assert!(!cals[0].read_only);
    assert!(cals[1].read_only, "shared read-only calendar");
    assert!(cals[0].capabilities.incremental_sync);
    // Every request carried the credentials.
    for r in server.received_requests().await.unwrap() {
        assert!(r.headers.get("authorization").is_some());
    }
}

#[tokio::test]
async fn initial_and_incremental_sync() {
    let server = nextcloud().await;
    let p = provider(&server).await;
    let cal = p.list_calendars().await.unwrap().remove(0);
    let cal_path = "/remote.php/dav/calendars/alice/personal/";

    // Initial: empty token lists everything.
    Mock::given(method("REPORT")).and(path(cal_path)).and(body_string_contains("<d:sync-token></d:sync-token>"))
        .respond_with(ms(r#"
<d:response><d:href>/remote.php/dav/calendars/alice/personal/e1.ics</d:href><d:propstat><d:prop><d:getetag>"1"</d:getetag></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
<d:response><d:href>/remote.php/dav/calendars/alice/personal/e2.ics</d:href><d:propstat><d:prop><d:getetag>"2"</d:getetag></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
<d:sync-token>http://sabre.io/ns/sync/7</d:sync-token>"#))
        .mount(&server).await;
    Mock::given(method("REPORT")).and(path(cal_path)).and(body_string_contains("calendar-multiget"))
        .respond_with(ms(&format!(r#"
<d:response><d:href>/remote.php/dav/calendars/alice/personal/e2.ics</d:href><d:propstat><d:prop><d:getetag>"2"</d:getetag><cal:calendar-data>{EVENT}</cal:calendar-data></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"#)))
        .mount(&server).await;
    // e1 is already known with the same ETag: it is not downloaded again.
    let known = vec![
        (
            format!("{}{}e1.ics", server.uri(), cal_path),
            Some("\"1\"".to_owned()),
        ),
        (
            format!("{}{}stale.ics", server.uri(), cal_path),
            Some("\"9\"".to_owned()),
        ),
    ];
    let changes = p.fetch_changes(&cal, None, &known).await.unwrap();
    assert_eq!(changes.upserted.len(), 1);
    assert!(changes.upserted[0].href.ends_with("/e2.ics"));
    assert!(changes.upserted[0].ical.contains("SUMMARY:Review"));
    assert_eq!(changes.deleted.len(), 1, "unlisted known object is deleted");
    assert!(changes.deleted[0].ends_with("/stale.ics"));
    assert_eq!(changes.cursor.as_deref(), Some("http://sabre.io/ns/sync/7"));

    // Incremental: only the delta, including a deletion.
    Mock::given(method("REPORT")).and(path(cal_path)).and(body_string_contains("sync/7</d:sync-token>"))
        .respond_with(ms(r#"
<d:response><d:href>/remote.php/dav/calendars/alice/personal/e1.ics</d:href><d:status>HTTP/1.1 404 Not Found</d:status></d:response>
<d:sync-token>http://sabre.io/ns/sync/8</d:sync-token>"#))
        .mount(&server).await;
    let changes = p
        .fetch_changes(&cal, Some("http://sabre.io/ns/sync/7"), &[])
        .await
        .unwrap();
    assert!(changes.upserted.is_empty());
    assert_eq!(changes.deleted.len(), 1);
    assert_eq!(changes.cursor.as_deref(), Some("http://sabre.io/ns/sync/8"));
}

#[tokio::test]
async fn expired_tokens_fall_back_to_a_full_listing() {
    let server = nextcloud().await;
    let p = provider(&server).await;
    let cal = p.list_calendars().await.unwrap().remove(0);
    let cal_path = "/remote.php/dav/calendars/alice/personal/";
    Mock::given(method("REPORT"))
        .and(path(cal_path))
        .and(body_string_contains("sync/1</d:sync-token>"))
        .respond_with(ResponseTemplate::new(403).set_body_raw(
            r#"<d:error xmlns:d="DAV:"><d:valid-sync-token/></d:error>"#,
            "application/xml",
        ))
        .mount(&server)
        .await;
    Mock::given(method("REPORT"))
        .and(path(cal_path))
        .and(body_string_contains("<d:sync-token></d:sync-token>"))
        .respond_with(ms(
            r#"<d:sync-token>http://sabre.io/ns/sync/9</d:sync-token>"#,
        ))
        .mount(&server)
        .await;
    let known = vec![(
        format!("{}{}old.ics", server.uri(), cal_path),
        Some("\"1\"".to_owned()),
    )];
    let changes = p
        .fetch_changes(&cal, Some("http://sabre.io/ns/sync/1"), &known)
        .await
        .unwrap();
    assert_eq!(changes.deleted.len(), 1);
    assert_eq!(changes.cursor.as_deref(), Some("http://sabre.io/ns/sync/9"));
}

#[tokio::test]
async fn conditional_writes_and_errors() {
    let server = nextcloud().await;
    let p = provider(&server).await;
    let href = format!(
        "{}/remote.php/dav/calendars/alice/personal/new.ics",
        server.uri()
    );
    Mock::given(method("PUT"))
        .and(path("/remote.php/dav/calendars/alice/personal/new.ics"))
        .and(header("If-None-Match", "*"))
        .respond_with(ResponseTemplate::new(201).insert_header("ETag", "\"abc\""))
        .mount(&server)
        .await;
    let w = p
        .put_object(&href, EVENT, &Precondition::Create)
        .await
        .unwrap();
    assert_eq!(w.etag.as_deref(), Some("\"abc\""));

    Mock::given(method("PUT"))
        .and(path("/remote.php/dav/calendars/alice/personal/new.ics"))
        .and(header("If-Match", "\"old\""))
        .respond_with(ResponseTemplate::new(412))
        .mount(&server)
        .await;
    assert_eq!(
        p.put_object(&href, EVENT, &Precondition::Version("\"old\"".into()))
            .await,
        Err(ProviderError::PreconditionFailed)
    );

    Mock::given(method("DELETE"))
        .and(path("/remote.php/dav/calendars/alice/personal/new.ics"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    assert_eq!(
        p.delete_object(&href, Some("\"abc\"")).await,
        Ok(()),
        "already deleted is fine"
    );

    Mock::given(method("PUT"))
        .and(path("/remote.php/dav/calendars/alice/personal/busy.ics"))
        .respond_with(ResponseTemplate::new(503).insert_header("Retry-After", "30"))
        .mount(&server)
        .await;
    let busy = format!(
        "{}/remote.php/dav/calendars/alice/personal/busy.ics",
        server.uri()
    );
    assert_eq!(
        p.put_object(&busy, EVENT, &Precondition::Create).await,
        Err(ProviderError::RateLimited {
            retry_after: Some(std::time::Duration::from_secs(30))
        })
    );
}

#[tokio::test]
async fn wrong_password_and_foreign_redirects_are_reported() {
    let server = MockServer::start().await;
    Mock::given(method("PROPFIND"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let base = Url::parse(&server.uri()).unwrap();
    assert_eq!(
        discover(&base, "alice", "wrong").await,
        Err(ProviderError::Unauthorized)
    );

    let redirecting = MockServer::start().await;
    Mock::given(method("PROPFIND"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", "https://elsewhere.example/dav/"),
        )
        .mount(&redirecting)
        .await;
    let base = Url::parse(&redirecting.uri()).unwrap();
    let err = discover(&base, "alice", "pw").await.unwrap_err();
    assert!(matches!(err, ProviderError::InvalidResponse(m) if m.contains("elsewhere.example")));
}

#[tokio::test]
async fn unreachable_servers_are_network_errors() {
    let base = Url::parse("http://127.0.0.1:9/").unwrap();
    assert!(matches!(
        discover(&base, "a", "b").await,
        Err(ProviderError::Network(_))
    ));
}
