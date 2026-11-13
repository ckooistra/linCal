# calendar-ics

**Where:** `crates/calendar-ics/`
**Depends on:** `calendar-caldav` (for its iCalendar codec), `calendar-ical`, `calendar-provider-api`, `reqwest`, `serde_json`.
**Used by:** `calendar-sync`.

## Its job in the architecture

**ICS subscriptions:** a read-only calendar published at a URL (`webcal://…` or `https://…`), such as public holidays, a sports team's fixtures, or the "secret address" of a calendar in another service.

The trick is that it **pretends to be an ordinary provider**, so the sync engine needs no special case:

- the feed appears as one read-only calendar;
- the single big `.ics` file is **split into one object per event UID**, each with a **content hash** as its version, so the engine's usual "what changed?" comparison works;
- every write is refused.

It also behaves politely towards the server: each refresh is a *conditional* GET (`If-None-Match` / `If-Modified-Since`, so an unchanged feed costs almost nothing), and downloads happen at most every 15 minutes.

Because a private feed URL is effectively a password, the URL is stored in the **keyring** like one, and never appears in logs or object addresses (`ics:feed/<uid>`).

## Files

### `lib.rs`

- `feed_url` turns what you typed into a URL: `webcal://` becomes `https://`, `example.org/cal.ics` gets `https://` added, and plain `http` is refused except for localhost.
- `split_feed` parses the feed once and builds one small VCALENDAR per UID, each with the time zones it needs. It also reads the feed's own name and colour.
- `fnv` is a tiny, stable hash (FNV-1a) used as each object's version.
- `IcsProvider` implements `CalendarProvider`:
  - `list_calendars` returns one read-only calendar.
  - `fetch_changes` downloads (conditionally), splits, and compares with the hashes we already hold.
  - `put_object` and `delete_object` always return "read-only".
- `probe` fetches and splits once, to check a feed when you subscribe.

```rust
let http = reqwest::Client::builder()
    .user_agent(concat!("UniversalCalendar/", env!("CARGO_PKG_VERSION")))
    .redirect(reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 5            { attempt.error("too many redirects") }
        else if check_transport(attempt.url()).is_err() { attempt.error("redirect to an insecure address") }
        else                                        { attempt.follow() }
    }))
    .timeout(Duration::from_secs(60))
    .build()?;
```

**Rust concepts:**
- **Reuse across crates.** `IcsProvider::codec()` returns `calendar_caldav::ICalendarCodec`; there's no need to write a second iCalendar codec ([concepts §6](concepts.md#6-traits-shared-behaviour)).
- **The builder pattern** (`Client::builder()…build()`), common in Rust libraries.
- **A closure as a policy:** the redirect rule is a closure that `reqwest` calls for each redirect ([§10](concepts.md#10-closures)).
- **Compile-time strings:** `concat!` and `env!("CARGO_PKG_VERSION")` build the `User-Agent` text from `Cargo.toml` when compiling ([§15](concepts.md#15-macros)).
- **Struct update syntax:** `ChangeSet { cursor: …, ..ChangeSet::default() }` sets one field and uses defaults for the rest.
- **`#[must_use]`** on `with_min_refresh` makes the compiler warn if you call it and throw away the result.
- **Unused parameters** start with `_` (`_calendar`), which tells the compiler it's intentional.
- **`saturating_sub`** subtracts without going below zero (no overflow panic).
- **Serde for the cursor:** the private `Validators` struct (ETag, Last-Modified, time) is stored as JSON in the cursor string.

### `examples/probe.rs`

A small command-line tool that fetches a feed and reports how it would be read, without storing anything:

```sh
cargo run -p calendar-ics --example probe -- 'webcal://example.org/feed.ics'
```

**Rust concepts:** `#[tokio::main]` makes `main` async. `let … else` handles a missing argument. `usize::from(bool)` counts `true` as 1.

## Tests

- **Unit tests** in `lib.rs`: URL handling, splitting, hashing.
- **`crates/calendar-sync/tests/ics.rs`** runs a whole subscription through the engine against a wiremock feed: first import, unchanged feed (304), changed and removed events, and refusing edits.
