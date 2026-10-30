//! The full authorization-code + PKCE flow against a mock token server, with
//! a fake "browser" that follows the redirect.

use calendar_oauth::{OAuthClient, OAuthError, TokenSource, Tokens, authorize, pkce_challenge};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Notify;
use url::Url;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client(server: &MockServer) -> OAuthClient {
    OAuthClient {
        auth_url: Url::parse(&format!("{}/auth", server.uri())).unwrap(),
        token_url: Url::parse(&format!("{}/token", server.uri())).unwrap(),
        client_id: "client-1".into(),
        client_secret: None,
        scopes: vec!["calendar".into()],
        extra_params: vec![("access_type".into(), "offline".into())],
        redirect_host: "127.0.0.1".into(),
    }
}

/// Plays the browser: checks the authorization URL and calls the redirect.
fn browser(
    code: &'static str,
    seen: std::sync::Arc<std::sync::Mutex<Option<Url>>>,
) -> impl Fn(&str) -> Result<(), String> + Send + Sync {
    move |auth: &str| {
        let auth = Url::parse(auth).unwrap();
        *seen.lock().unwrap() = Some(auth.clone());
        let q = |k: &str| {
            auth.query_pairs()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.into_owned())
                .unwrap()
        };
        let redirect = Url::parse(&q("redirect_uri")).unwrap();
        let state = q("state");
        tokio::spawn(async move {
            // A stray request first (wrong state) must be ignored.
            for target in [
                "/favicon.ico".to_owned(),
                format!("/?code={code}&state={state}"),
            ] {
                let mut s = tokio::net::TcpStream::connect(("127.0.0.1", redirect.port().unwrap()))
                    .await
                    .unwrap();
                s.write_all(format!("GET {target} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes())
                    .await
                    .unwrap();
                let mut reply = String::new();
                let _ = s.read_to_string(&mut reply).await;
            }
        });
        Ok(())
    }
}

#[tokio::test]
async fn signs_in_with_pkce_and_refreshes() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=authorization_code"))
        .and(body_string_contains("code=the-code"))
        .and(body_string_contains("code_verifier="))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"access_token":"a1","refresh_token":"r1","expires_in":3600,"token_type":"Bearer"}"#,
        ))
        .mount(&server)
        .await;
    let seen = std::sync::Arc::new(std::sync::Mutex::new(None));
    let tokens = authorize(
        &client(&server),
        &browser("the-code", seen.clone()),
        &Notify::new(),
    )
    .await
    .unwrap();
    assert_eq!(tokens.access_token, "a1");
    assert_eq!(tokens.refresh_token.as_deref(), Some("r1"));
    let auth = seen.lock().unwrap().clone().unwrap();
    let q: std::collections::HashMap<_, _> = auth.query_pairs().into_owned().collect();
    assert_eq!(q["code_challenge_method"], "S256");
    assert_eq!(q["access_type"], "offline");
    assert!(q["redirect_uri"].starts_with("http://127.0.0.1:"));

    // The verifier sent in the exchange matches the challenge.
    let requests = server.received_requests().await.unwrap();
    let body = String::from_utf8(requests[0].body.clone()).unwrap();
    let verifier = url::form_urlencoded::parse(body.as_bytes())
        .find(|(k, _)| k == "code_verifier")
        .unwrap()
        .1
        .into_owned();
    assert_eq!(pkce_challenge(&verifier), q["code_challenge"]);

    // An expired access token is refreshed; the rotation is reported once.
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .and(body_string_contains("refresh_token=r1"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"access_token":"a2","expires_in":3600}"#),
        )
        .mount(&server)
        .await;
    let source = TokenSource::new(
        client(&server),
        Tokens {
            expires_at_ms: 0,
            ..tokens
        },
    );
    assert_eq!(source.access_token().await.unwrap(), "a2");
    let changed = source.take_changed().unwrap();
    assert_eq!(changed.refresh_token.as_deref(), Some("r1"));
    assert!(source.take_changed().is_none());
    assert_eq!(source.access_token().await.unwrap(), "a2");
}

#[tokio::test]
async fn revoked_refresh_tokens_mean_sign_in_again() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(400).set_body_string(r#"{"error":"invalid_grant"}"#))
        .mount(&server)
        .await;
    let source = TokenSource::new(
        client(&server),
        Tokens {
            access_token: "old".into(),
            refresh_token: Some("revoked".into()),
            expires_at_ms: 0,
        },
    );
    assert_eq!(source.access_token().await, Err(OAuthError::Expired));
}

#[tokio::test]
async fn sign_in_can_be_cancelled() {
    let server = MockServer::start().await;
    let cancel = Notify::new();
    cancel.notify_one();
    let result = authorize(&client(&server), &|_: &str| Ok(()), &cancel).await;
    assert_eq!(result, Err(OAuthError::Cancelled));
}
