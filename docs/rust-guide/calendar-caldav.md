# calendar-caldav

**Where:** `crates/calendar-caldav/`
**Depends on:** `calendar-core`, `calendar-ical`, `calendar-provider-api`, `reqwest` (HTTP), `roxmltree` (XML), `url`.
**Used by:** `calendar-sync`, and `calendar-ics` (which reuses its codec).

## Its job in the architecture

**CalDAV** is the open standard Nextcloud, Radicale, Fastmail, iCloud and many others speak. It's HTTP with extra methods (`PROPFIND`, `REPORT`) and XML bodies, and each event is an `.ics` file on the server.

This crate implements `CalendarProvider` ([calendar-provider-api.md](calendar-provider-api.md)) for CalDAV. It was the first provider, and Nextcloud is the reference server, but nothing in it is Nextcloud-specific.

What a sync looks like on the wire:

1. **Discovery** (once, when you add the account): from the URL you typed, find your "principal", then your "calendar home" (the folder that holds your calendars). It tries `/.well-known/caldav` if needed.
2. **List calendars:** `PROPFIND` the home with `Depth: 1`.
3. **Fetch changes:** `REPORT sync-collection` with the last **sync token**, so the server lists only what changed. Then `calendar-multiget` fetches those objects, 50 at a time.
4. **Upload:** `PUT` with `If-Match: <etag>` (or `If-None-Match: *` for new events). **Delete** works the same way.

## Files

### `client.rs`: a small, careful HTTP client

`Client` wraps `reqwest`, tied to one server and one username and password.

- **Security:** credentials go only over HTTPS (plain HTTP only to `localhost`), and only to the server you entered. A redirect to another host is refused, not followed.
- **Limits:** at most 5 redirects; response bodies over 64 MB are refused, read in chunks so memory stays bounded.
- `map_status` turns HTTP status codes into `ProviderError`s (401 → `Unauthorized`, 412 → `PreconditionFailed`, 429/503 → `RateLimited` with the server's `Retry-After`, …).

```rust
pub fn check_transport(url: &Url) -> Result<(), ProviderError> {
    match url.scheme() {
        "https" => Ok(()),
        "http" if is_loopback(url) => Ok(()),     // a "match guard"
        _ => Err(ProviderError::InvalidResponse("… must use https:// …".into())),
    }
}
```

**Rust concepts:**
- **`async fn send(…)`** with `.await` on each network step ([concepts §14](concepts.md#14-asyncawait-and-tokio)).
- **Match guards** (`"http" if …`) and **or-patterns** (`404 | 410`) ([§4](concepts.md#4-match-and-pattern-matching)).
- **`while let Some(chunk) = response.chunk().await?`**, a loop that runs while a pattern matches.
- **Converting library errors** into our own (`fn network(e: reqwest::Error) -> ProviderError`), so other crates never see `reqwest` types.
- **Slices of tuples** for headers: `&[(&str, String)]`.

### `xml.rs`: reading WebDAV's XML

WebDAV answers with a "multistatus" XML document: one `<response>` per item, each with properties. `parse_multistatus` turns that into plain structs (`Multistatus`, `Response`, `Prop`) so the rest of the crate never handles XML nodes. The file also holds the request bodies as `const` strings, and `escape` for putting hrefs inside XML safely.

**Rust concepts:**
- `const` string literals with raw strings `r#"…"#` (no need to escape quotes);
- iterator chains over XML nodes (`descendants().filter(…).filter_map(…).collect::<String>()`), where the **turbofish** `::<String>` tells `collect` what to build ([§11](concepts.md#11-iterators));
- `Option` chaining with `?` inside a function returning `Option` (`status_code`).

### `lib.rs`: the provider

- **`ICalendarCodec`** implements `ObjectCodec` by calling `calendar-ical`. The struct has no fields at all (a "unit struct"), because it needs no state.
- **`discover`** finds the calendar home from a URL, username and password.
- **`CalDavProvider`** implements `CalendarProvider`:
  - `list_calendars` skips task lists and inboxes, and detects read-only calendars from your privileges.
  - `fetch_changes` uses sync tokens, falling back to comparing ETags when the server has none.
  - `put_object` and `delete_object` send conditional requests.

```rust
// Skip collections that cannot hold events (task lists etc.).
if let Some(comps) = r.prop(CALDAV, "supported-calendar-component-set")
    && !comps.elements.iter().any(|(_, _, name)| name.as_deref() == Some("VEVENT"))
{
    continue;
}
```

**Rust concepts:**
- **`impl Trait for Type`:** `impl CalendarProvider for CalDavProvider` fulfils the contract. If a method is missing or has the wrong signature, it doesn't compile ([§6](concepts.md#6-traits-shared-behaviour)).
- **Unit structs** (`pub struct ICalendarCodec;`).
- **Let chains** (`if let … && …`), as above ([§4](concepts.md#4-match-and-pattern-matching)).
- **`std::mem::take`** moves a value out of a struct and leaves an empty one behind, without cloning.
- **`as_deref()`** turns `Option<String>` into `Option<&str>` for comparing.

## Tests

- **`tests/mock_server.rs`** starts a fake server with **wiremock** that answers like Nextcloud does (copied XML), then checks discovery, listing (including skipping the task list and spotting the read-only shared calendar), syncing and writing. It needs no network ([§17](concepts.md#17-tests)).
- **`tests/real_server.rs`** runs against a real server, but only when `UC_CALDAV_URL` and the other variables are set. Otherwise it returns early and passes. Use it with the local Radicale from `scripts/dev-radicale.sh`.
