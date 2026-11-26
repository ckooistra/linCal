# calendar-google

**Where:** `crates/calendar-google/`
**Depends on:** `calendar-core`, `calendar-ical`, `calendar-oauth`, `calendar-provider-api`, `reqwest`, `serde_json`.
**Used by:** `calendar-sync`.

## Its job in the architecture

Implements `CalendarProvider` for the **Google Calendar API v3**, a JSON web API. From the sync engine's point of view it behaves exactly like CalDAV: list calendars, fetch changes since a cursor, put and delete objects with version checks.

The differences it hides:

| CalDAV | Google |
|---|---|
| username and password | OAuth tokens (via `calendar-oauth`) |
| one `.ics` file per series | a series is a "master" plus separate "exception" events |
| sync token in XML | `syncToken` in JSON; HTTP 410 means "start over" |
| server chooses nothing | **we** choose the event id, so a retried create can't duplicate |

To keep "one object = one series" (the contract), the provider bundles a master and its exceptions into one JSON object: `{"timeZone": …, "events": [master, exceptions…]}`.

## Files

### `lib.rs`: the provider

- `oauth_client` holds Google's OAuth settings (sign-in URL, the calendar scope, "offline" access to get a refresh token).
- `GoogleSecret` is what goes into the keyring: the tokens and an optional client secret. It's stored as JSON.
- `GoogleProvider::send` handles each API request: it adds the bearer token, and on a 401 refreshes the token and **retries once**.
- `impl CalendarProvider for GoogleProvider`: paging through results, sync tokens, and conditional writes with `If-Match`.
- `move_object` calls `events.move`. The API takes no version for it, so the series' version is compared first, as for deletions. Google refuses to move another organizer's events (invitations), and those get `Ok(None)`: the engine copies them instead.
- `account_email` fetches the address, shown as the account name.
- `event_id_for` picks a new event's id from its local name. Google accepts only base32hex that *decodes* (a length an encoding can have, no stray bits at the end), which its documentation doesn't spell out. Hex digits behind a prefix are allowed characters but can have an impossible length, such as 86, and Google refuses those with "Invalid resource id value". So the id is a proper base32hex encoding of `uc:` plus the name.

```rust
let mut token = self.tokens.access_token().await.map_err(oauth_error)?;
for attempt in 0..2 {
    let request = self.http.request(method.clone(), url).bearer_auth(&token) …;
    // on 401 in the first attempt: token = self.tokens.refresh_after_rejection(&token).await?
}
```

**Rust concepts:**
- **`Arc<TokenSource>`:** the token source is shared with whoever created the provider ([concepts §13](concepts.md#13-smart-pointers-and-sharing-box-arc-mutex)).
- **`std::sync::Mutex<HashMap<…>>`** caches each calendar's calendar-list entry: its default reminders and its time zone.
- **`serde_json::Value`:** working with JSON whose exact shape we don't want to fix in a struct, because Google adds fields all the time.
- **Named constructors:** `new` takes a token source (used by tests); `connect` builds everything from the stored secret (used by the app).

### `codec.rs`: Google JSON ⇄ the neutral model

**Reading:** rather than reimplementing recurrence for Google, the codec **converts Google's JSON into iCalendar text** and hands it to `calendar-ical`. Google already writes repeat rules as iCalendar lines (`RRULE:…`), so this is a natural fit, and all the recurrence logic stays in one place.

**Writing:** produces a *plan*, a list of small patches, one per series member, each containing **only the fields this app changed**, and each conditional on that member's ETag. Attendees, Meet links and anything else stay untouched on the server.

**Floating times** (a wall-clock time without a zone, common in events from CalDAV servers) don't exist on Google, so they're written in the calendar's time zone. A new object's plan doesn't know that zone yet, so it leaves placeholders, which `put_object` fills in from the calendar's calendar-list entry:
- `CALENDAR_ZONE` stands for the zone of floating `start`/`end` times.
- `FLOATING_INSTANT` stands for the UTC part of an exception's instance id (`<master>_<YYYYMMDDTHHMMSSZ>`). The op carries the local time as `"floating"`.

```rust
fn popup_minutes(list: Option<&Value>) -> Vec<u32> {
    normalize_reminders(
        list.and_then(Value::as_array).into_iter().flatten()
            .filter(|r| str_field(r, "method") == Some("popup"))  // not e-mail reminders
            .filter_map(|r| r.get("minutes")?.as_u64())
            .filter_map(|m| u32::try_from(m).ok())
            .filter(|m| *m <= MAX_REMINDER_MINUTES)
            .collect(),
    )
}
```

**Rust concepts:**
- **Iterator pipelines** that turn messy JSON into clean data. Every step that might fail (`?` inside `filter_map`, `try_from`) just drops that item ([§11](concepts.md#11-iterators)).
- **`Option` into an iterator:** `.into_iter().flatten()` treats "no list" as "empty list".
- **Function paths as closures:** `Value::as_array` and `str::to_owned` are passed where a closure would go.
- **Lifetimes on a helper:** `fn str_field<'a>(v: &'a Value, key: &str) -> Option<&'a str>`, meaning the returned text borrows from the JSON, not from the key ([§12](concepts.md#12-lifetimes-a)).

### `testing.rs`: a fake Google (feature `testing`)

A small, stateful imitation of the Google API on a wiremock server. It remembers events, issues ETags and sync tokens, pages results, returns 410 for old sync tokens and 401 for expired tokens. Tests for this crate and for `calendar-sync` use it, so no test needs a real Google account.

```rust
struct Handler(Arc<Mutex<State>>);
impl Respond for Handler {
    fn respond(&self, req: &Request) -> ResponseTemplate { let mut s = self.0.lock().unwrap(); … }
}
```

**Rust concepts:** a feature-gated module (`#[cfg(feature = "testing")]`), so it's never compiled into the app ([§16](concepts.md#16-features-and-conditional-compilation)). Implementing **another library's trait** (`wiremock::Respond`) for our own type. Shared changeable state as `Arc<Mutex<State>>`.

## Tests

- **Unit tests** in `codec.rs`: series with exceptions and cancellations, "patch only what changed", all-day events, malformed data.
- **`tests/provider.rs`** drives the real provider against `FakeGoogle`: sync tokens, paging, 410 recovery, token refresh on 401, conditional writes, and moving a series with its exceptions (an invitation can't move).

The crate turns on its own `testing` feature for its tests (`calendar-google = { path = ".", features = ["testing"] }` under `[dev-dependencies]`), a common Cargo trick.
