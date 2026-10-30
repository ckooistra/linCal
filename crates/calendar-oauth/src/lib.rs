//! OAuth 2.0 for installed applications (RFC 8252): the authorization-code
//! flow with PKCE (RFC 7636), a one-shot loopback redirect listener, and
//! access-token refresh.
//!
//! * Nothing here is provider-specific: Google and Microsoft differ only in
//!   their [`OAuthClient`] settings.
//! * No client secret is required. Some providers issue "secrets" to
//!   desktop clients that are documented as non-confidential; one can be
//!   supplied, but callers must store it like a token (OS keyring).
//! * Tokens are never logged; [`Tokens`] has a redacting `Debug`.

use std::sync::Mutex;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Notify;
use url::Url;

/// How long the user has to finish signing in in the browser.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Access tokens are refreshed this long before they expire.
const EXPIRY_MARGIN_MS: i64 = 60_000;

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum OAuthError {
    #[error("sign-in was cancelled")]
    Cancelled,
    #[error("sign-in timed out")]
    TimedOut,
    #[error("the provider refused the sign-in: {0}")]
    Denied(String),
    /// The refresh token was rejected: the user must sign in again.
    #[error("the sign-in has expired or was revoked")]
    Expired,
    #[error("could not reach the sign-in server: {0}")]
    Network(String),
    #[error("unexpected answer from the sign-in server: {0}")]
    Invalid(String),
    #[error("could not open the browser: {0}")]
    Browser(String),
}

/// Settings of one OAuth client registration.
#[derive(Debug, Clone)]
pub struct OAuthClient {
    pub auth_url: Url,
    pub token_url: Url,
    pub client_id: String,
    /// Non-confidential "secret" of a desktop client, if the provider
    /// requires one (Google). Stored only in the OS keyring.
    pub client_secret: Option<String>,
    pub scopes: Vec<String>,
    /// Extra authorization parameters (e.g. `access_type=offline`).
    pub extra_params: Vec<(String, String)>,
    /// Host of the loopback redirect: `127.0.0.1` (Google) or `localhost`
    /// (Microsoft, which registers `http://localhost`).
    pub redirect_host: String,
}

/// Tokens of one account. Serialized as JSON into the OS keyring.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Epoch ms after which the access token is no longer valid.
    pub expires_at_ms: i64,
}

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tokens")
            .field("access_token", &"…")
            .field("refresh_token", &self.refresh_token.as_ref().map(|_| "…"))
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn random_token(bytes: usize) -> Result<String, OAuthError> {
    let mut buf = vec![0u8; bytes];
    getrandom::getrandom(&mut buf).map_err(|e| OAuthError::Invalid(format!("randomness: {e}")))?;
    Ok(URL_SAFE_NO_PAD.encode(buf))
}

/// PKCE S256 challenge of a verifier.
pub fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn http() -> Result<reqwest::Client, OAuthError> {
    reqwest::Client::builder()
        .user_agent(concat!("UniversalCalendar/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| OAuthError::Network(e.to_string()))
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
}

#[derive(Deserialize)]
struct ErrorResponse {
    error: String,
    error_description: Option<String>,
}

/// The form for a token request (built outside async code: the serializer
/// is not `Send`).
fn form_body(client: &OAuthClient, params: &[(&str, &str)]) -> String {
    let mut form = url::form_urlencoded::Serializer::new(String::new());
    form.append_pair("client_id", &client.client_id);
    if let Some(secret) = &client.client_secret {
        form.append_pair("client_secret", secret);
    }
    for (k, v) in params {
        form.append_pair(k, v);
    }
    form.finish()
}

/// POSTs a form to the token endpoint.
async fn token_request(
    client: &OAuthClient,
    params: &[(&str, &str)],
) -> Result<TokenResponse, OAuthError> {
    let body = form_body(client, params);
    let response = http()?
        .post(client.token_url.clone())
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|e| OAuthError::Network(e.to_string()))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| OAuthError::Network(e.to_string()))?;
    if status.is_success() {
        return serde_json::from_str(&body).map_err(|e| OAuthError::Invalid(e.to_string()));
    }
    match serde_json::from_str::<ErrorResponse>(&body) {
        // RFC 6749 §5.2: the grant (code or refresh token) is no longer valid.
        Ok(e) if e.error == "invalid_grant" => Err(OAuthError::Expired),
        Ok(e) => Err(OAuthError::Denied(e.error_description.unwrap_or(e.error))),
        Err(_) if status.is_server_error() => Err(OAuthError::Network(format!("HTTP {status}"))),
        Err(_) => Err(OAuthError::Invalid(format!("HTTP {status}"))),
    }
}

fn tokens_from(response: TokenResponse, previous_refresh: Option<&str>) -> Tokens {
    Tokens {
        access_token: response.access_token,
        // Providers may or may not rotate the refresh token.
        refresh_token: response
            .refresh_token
            .or_else(|| previous_refresh.map(str::to_owned)),
        expires_at_ms: now_ms() + response.expires_in.unwrap_or(3600) * 1000,
    }
}

/// Signs in: opens the provider's page with `open_browser`, waits for the
/// redirect to a one-shot loopback listener, and exchanges the code.
pub async fn authorize(
    client: &OAuthClient,
    open_browser: &(dyn Fn(&str) -> Result<(), String> + Send + Sync),
    cancel: &Notify,
) -> Result<Tokens, OAuthError> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|e| OAuthError::Network(format!("loopback listener: {e}")))?;
    let port = listener
        .local_addr()
        .map_err(|e| OAuthError::Network(e.to_string()))?
        .port();
    let redirect_uri = format!("http://{}:{port}/", client.redirect_host);
    let verifier = random_token(48)?;
    let state = random_token(24)?;

    let mut url = client.auth_url.clone();
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("response_type", "code")
            .append_pair("client_id", &client.client_id)
            .append_pair("redirect_uri", &redirect_uri)
            .append_pair("scope", &client.scopes.join(" "))
            .append_pair("state", &state)
            .append_pair("code_challenge", &pkce_challenge(&verifier))
            .append_pair("code_challenge_method", "S256");
        for (k, v) in &client.extra_params {
            q.append_pair(k, v);
        }
    }
    open_browser(url.as_str()).map_err(OAuthError::Browser)?;

    let code = tokio::select! {
        result = tokio::time::timeout(SIGN_IN_TIMEOUT, wait_for_code(&listener, &state)) => {
            result.map_err(|_| OAuthError::TimedOut)??
        }
        () = cancel.notified() => return Err(OAuthError::Cancelled),
    };
    let response = token_request(
        client,
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", &redirect_uri),
            ("code_verifier", &verifier),
        ],
    )
    .await?;
    Ok(tokens_from(response, None))
}

const DONE_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Universal Calendar</title>\
<body style=\"font-family:sans-serif;padding:3rem\"><h1>Signed in</h1>\
<p>You can close this tab and return to Universal Calendar.</p>";

const FAILED_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Universal Calendar</title>\
<body style=\"font-family:sans-serif;padding:3rem\"><h1>Sign-in did not complete</h1>\
<p>Return to Universal Calendar to try again.</p>";

/// Accepts redirects until one carries our `state`, answers it with a
/// small page, and returns its code (or the provider's error).
async fn wait_for_code(listener: &TcpListener, state: &str) -> Result<String, OAuthError> {
    loop {
        let (mut stream, _) = listener
            .accept()
            .await
            .map_err(|e| OAuthError::Network(e.to_string()))?;
        let mut buf = vec![0u8; 8192];
        let n = match tokio::time::timeout(Duration::from_secs(10), stream.read(&mut buf)).await {
            Ok(Ok(n)) => n,
            _ => continue,
        };
        let request = String::from_utf8_lossy(&buf[..n]);
        let target = request
            .lines()
            .next()
            .and_then(|line| line.strip_prefix("GET "))
            .and_then(|rest| rest.split(' ').next())
            .unwrap_or("");
        let Ok(url) = Url::parse(&format!("http://localhost{target}")) else {
            continue;
        };
        let param = |name: &str| {
            url.query_pairs()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.into_owned())
        };
        // Other requests (favicon, a stale tab) are not ours.
        if param("state").as_deref() != Some(state) {
            let _ = respond(&mut stream, "404 Not Found", "").await;
            continue;
        }
        if let Some(error) = param("error") {
            let _ = respond(&mut stream, "200 OK", FAILED_PAGE).await;
            return Err(OAuthError::Denied(
                param("error_description").unwrap_or(error),
            ));
        }
        let Some(code) = param("code") else {
            let _ = respond(&mut stream, "200 OK", FAILED_PAGE).await;
            return Err(OAuthError::Invalid("redirect without a code".into()));
        };
        let _ = respond(&mut stream, "200 OK", DONE_PAGE).await;
        return Ok(code);
    }
}

async fn respond(
    stream: &mut tokio::net::TcpStream,
    status: &str,
    body: &str,
) -> std::io::Result<()> {
    let reply = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(reply.as_bytes()).await?;
    stream.shutdown().await
}

/// Hands out valid access tokens for one account, refreshing them when
/// they are about to expire, and remembers rotated tokens for the caller
/// to persist.
pub struct TokenSource {
    client: OAuthClient,
    tokens: tokio::sync::Mutex<Tokens>,
    changed: Mutex<Option<Tokens>>,
}

impl TokenSource {
    pub fn new(client: OAuthClient, tokens: Tokens) -> Self {
        Self {
            client,
            tokens: tokio::sync::Mutex::new(tokens),
            changed: Mutex::new(None),
        }
    }

    /// A valid access token; refreshes first if it expires soon.
    pub async fn access_token(&self) -> Result<String, OAuthError> {
        let mut tokens = self.tokens.lock().await;
        if tokens.expires_at_ms - EXPIRY_MARGIN_MS <= now_ms() {
            self.refresh_locked(&mut tokens).await?;
        }
        Ok(tokens.access_token.clone())
    }

    /// Refreshes now (after the server rejected `rejected`), unless another
    /// request already did so.
    pub async fn refresh_after_rejection(&self, rejected: &str) -> Result<String, OAuthError> {
        let mut tokens = self.tokens.lock().await;
        if tokens.access_token == rejected {
            self.refresh_locked(&mut tokens).await?;
        }
        Ok(tokens.access_token.clone())
    }

    async fn refresh_locked(&self, tokens: &mut Tokens) -> Result<(), OAuthError> {
        let refresh = tokens.refresh_token.clone().ok_or(OAuthError::Expired)?;
        let response = token_request(
            &self.client,
            &[("grant_type", "refresh_token"), ("refresh_token", &refresh)],
        )
        .await?;
        *tokens = tokens_from(response, Some(&refresh));
        *self.changed.lock().unwrap_or_else(|e| e.into_inner()) = Some(tokens.clone());
        Ok(())
    }

    /// Tokens obtained since the last call, to be persisted.
    pub fn take_changed(&self) -> Option<Tokens> {
        self.changed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_matches_the_rfc_example() {
        // RFC 7636 Appendix B.
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn tokens_never_print_secrets() {
        let t = Tokens {
            access_token: "secret-access".into(),
            refresh_token: Some("secret-refresh".into()),
            expires_at_ms: 1,
        };
        let shown = format!("{t:?}");
        assert!(!shown.contains("secret"));
    }
}
