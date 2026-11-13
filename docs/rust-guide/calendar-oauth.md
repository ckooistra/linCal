# calendar-oauth

**Where:** `crates/calendar-oauth/src/lib.rs` (one file)
**Depends on:** `tokio` (networking), `reqwest`, `sha2` and `base64` (PKCE), `url`, `serde`.
**Used by:** `calendar-google`, `calendar-microsoft`, `calendar-sync`.

## Its job in the architecture

Google and Microsoft don't take a username and password. You sign in **in your browser**, and the app receives **tokens**. This crate implements that "OAuth 2.0 for desktop apps" flow once, generically. Google and Microsoft differ only in the settings they pass in (`OAuthClient`).

How sign-in works:

1. The app opens a tiny web server on `127.0.0.1` at a random port.
2. It opens your browser at Google's or Microsoft's sign-in page, including a **PKCE challenge** (a hash of a secret only the app knows) and a random `state`.
3. You sign in. The browser is sent back to `http://127.0.0.1:<port>/?code=…&state=…`.
4. The app checks `state`, then swaps the code (plus the PKCE secret) for an **access token** (short-lived) and a **refresh token** (long-lived).
5. Later, `TokenSource` refreshes the access token automatically before it expires.

No client secret is built into the app, and tokens are only ever stored in the OS keyring (by `calendar-sync`).

## What's in the file

| Item | Purpose |
|---|---|
| `OAuthClient` | settings: sign-in URL, token URL, client id, scopes |
| `Tokens` | access token, refresh token, expiry |
| `authorize` | the whole browser sign-in flow above |
| `wait_for_code` | the tiny loopback web server, reading one HTTP request by hand |
| `pkce_challenge` | SHA-256 and base64url of the secret (RFC 7636) |
| `TokenSource` | hands out valid access tokens, refreshing when needed |

### Tokens never leak into logs

```rust
impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tokens")
            .field("access_token", &"…")
            .field("refresh_token", &self.refresh_token.as_ref().map(|_| "…"))
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}
```

Instead of `#[derive(Debug)]`, `Debug` is written by hand to print `…` in place of secrets. A test (`tokens_never_print_secrets`) makes sure it stays that way.

### Waiting for the browser, or a timeout, or Cancel

```rust
let code = tokio::select! {
    result = tokio::time::timeout(SIGN_IN_TIMEOUT, wait_for_code(&listener, &state)) => {
        result.map_err(|_| OAuthError::TimedOut)??
    }
    () = cancel.notified() => return Err(OAuthError::Cancelled),
};
```

`select!` runs both branches and takes whichever finishes first: the browser redirect (within the timeout) or the user pressing Cancel. The `??` unwraps two layers: the timeout's `Result`, then `wait_for_code`'s own `Result`.

### Refreshing safely when several requests run at once

```rust
pub struct TokenSource {
    client: OAuthClient,
    tokens: tokio::sync::Mutex<Tokens>,      // held across .await while refreshing
    changed: std::sync::Mutex<Option<Tokens>>,  // quick access, never across .await
}
```

If three requests notice an expired token at the same moment, the async mutex makes them queue. The first one refreshes; the others then see the fresh token and don't refresh again (`refresh_after_rejection` compares against the token that was rejected). New tokens are left in `changed` for the sync engine to save to the keyring (`take_changed`).

## Rust concepts in this file

- **Implementing a trait by hand to control behaviour:** `Debug` above ([concepts §6](concepts.md#6-traits-shared-behaviour)).
- **`tokio::select!`, `timeout` and `Notify`** for "first of several events" ([§14](concepts.md#14-asyncawait-and-tokio)).
- **Two kinds of `Mutex`**, chosen on purpose ([§13](concepts.md#13-smart-pointers-and-sharing-box-arc-mutex)).
- **A borrowed closure as a parameter:** `open_browser: &(dyn Fn(&str) -> Result<(), String> + Send + Sync)`. The caller decides *how* to open a browser (Tauri's opener in the app, a fake in tests) ([§10](concepts.md#10-closures)).
- **Low-level async I/O:** `TcpListener::bind`, `accept`, `read`, `write_all`. A real (tiny) web server in about 50 lines.
- **`let … else`:** `let Ok(url) = Url::parse(…) else { continue; };` ([§4](concepts.md#4-match-and-pattern-matching)).
- **Scoped blocks:** `{ let mut q = url.query_pairs_mut(); … }` ends the mutable borrow of `url` at the `}`, so `url` can be used again afterwards ([§1](concepts.md#1-variables-ownership-and-borrowing)).
- **`#[derive(Deserialize)]` on private structs** (`TokenResponse`) to read the token server's JSON.

## Tests

- **Unit tests** check PKCE against the example in the RFC, and that secrets never print.
- **`tests/flow.rs`** runs the whole flow: a wiremock token server plus a fake "browser" closure that follows the redirect into the real loopback listener.
