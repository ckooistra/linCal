# calendar-microsoft

**Where:** `crates/calendar-microsoft/`
**Depends on:** `calendar-core`, `calendar-ical`, `calendar-oauth`, `calendar-provider-api`, `reqwest`, `serde_json`.
**Used by:** `calendar-sync`.

## Its job in the architecture

Implements `CalendarProvider` for **Microsoft Graph**: Outlook.com and Microsoft 365 calendars. Like Google, it uses OAuth and JSON. Graph is the most different of the providers, so this crate does the most translation:

- **Sync is tied to a date window.** Graph's change feed (`calendarView/delta`) only covers a date range. The provider mirrors one year back and two years ahead, and moves the window forward monthly. The window and the delta link are stored together as the cursor.
- **Graph assigns event ids,** so creates send a `transactionId` and a retry can't create a duplicate.
- **Outlook can express fewer repeat rules** than iCalendar. `pattern.rs` translates between them and refuses rules Outlook can't hold *before* they're saved, so you never get a silent approximation.
- **Time zones may have Windows names** ("W. Europe Standard Time"), mapped by `calendar-ical`.

## Files

### `lib.rs`: the provider

- `oauth_client` and `SCOPES` hold Microsoft's sign-in settings.
- `GraphProvider` holds the HTTP client, `Arc<TokenSource>`, the API URL and a `today` function.
- The window logic (`window`, `Cursor`) and the `CalendarProvider` implementation live here.
- `version_of`: a series has no single ETag in Graph, so its version is a **hash** over the master's, its exceptions' and its cancellations' versions.

```rust
pub struct GraphProvider {
    http: reqwest::Client,
    tokens: Arc<TokenSource>,
    api: Url,
    today: fn() -> NaiveDate,      // replaced in tests to "travel in time"
}

pub fn with_today(mut self, today: fn() -> NaiveDate) -> Self { self.today = today; self }
```

**Rust concepts:**
- **A function pointer field** (`fn() -> NaiveDate`): tests swap in a fixed date to check the window moving, without waiting a month.
- **Builder method taking `mut self`:** `with_today` consumes the provider, changes it and returns it, so you can write `GraphProvider::new(…)?.with_today(fixed)`.
- **`fold`** to compute a hash over bytes ([concepts §11](concepts.md#11-iterators)).
- **`split_once` and `rsplit_once`** for parsing hrefs; `format!("{hash:016x}")` for hex output.
- **Private serde struct** `Cursor`, saved as JSON inside the opaque cursor string.

### `codec.rs`: Graph JSON ⇄ the neutral model

- `event_content` reads one Graph event (subject, body, location, start and end, all-day, reminder) into `EventContent`.
- `GraphCodec` implements `ObjectCodec` for a single event or a whole series (master plus `exceptionOccurrences` plus `cancelledOccurrences`).
- `patch` builds a JSON patch with **only the changed fields**, like Google's codec, so attendees and Teams links are left alone.

Times are read in UTC and written as local times in the event's own zone, so a weekly 09:00 meeting stays at 09:00 after a daylight-saving change.

**Rust concepts:** `serde_json::Map<String, Value>` built up field by field; returning `Result<(EventContent, Vec<String>), String>`, a tuple of result plus warnings.

### `pattern.rs`: Outlook recurrence ⇄ RRULE

- `to_graph(rrule, timing)` turns `FREQ=WEEKLY;BYDAY=MO,WE` into Graph's `{"pattern": {"type": "weekly", "daysOfWeek": […]}, "range": {…}}`.
- `to_rrule` goes the other way.
- `check` says whether Outlook can hold a rule at all. The sync engine registers it with storage, so the editor's save is refused with a clear message ("Outlook calendars cannot repeat …").

```rust
struct Rule {
    freq: String,
    interval: u32,
    by_day: Vec<(Option<i32>, String)>,   // e.g. (Some(-1), "FR") = last Friday
    by_month_day: Option<u32>,
    …
}
```

**Rust concepts:**
- **Parsing into a struct first,** then validating and translating. Each step is a small function returning `Result<_, String>`.
- **Generic helper with a trait bound:** `fn single<T: FromStr>(key: &str, value: &str) -> Result<T, String>` parses any number type ([§7](concepts.md#7-generics-and-trait-objects-dyn)).
- **`BTreeMap`** (a sorted map) for the rule's `KEY=VALUE` parts.
- **Tuples inside `Vec`** for small paired data.

### `testing.rs`: a fake Microsoft Graph (feature `testing`)

The same idea as `calendar-google/src/testing.rs`, but larger, because Graph's series behaviour is more involved. It covers delta paging, `@removed` entries, 410 for expired delta state, instances with ids like `<master>.<date>`, `transactionId` on create, and 401 for expired tokens. `event()` is a helper to build test events quickly.

**Rust concepts:** `impl Respond for Handler` with a tuple struct `Handler(Arc<Mutex<State>>, String)` ([§13](concepts.md#13-smart-pointers-and-sharing-box-arc-mutex)).

## Tests

- **Unit tests** in `pattern.rs` (every supported and refused rule) and `codec.rs`.
- **`tests/provider.rs`** runs the provider against `FakeGraph`, including the window moving forward.

A live check against a real Microsoft account is still pending: it needs a registered client ID.
