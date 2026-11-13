# calendar-core

**Where:** `crates/calendar-core/`
**Depends on:** only general libraries: `chrono` (dates), `chrono-tz` (time zones), `rrule` (repeat rules), `serde` (JSON), `thiserror` (errors).
**Used by:** every other crate.

## Its job in the architecture

This is the **heart** of the app: what an account, calendar and event *are*, and the hard calendar maths (time zones, daylight saving, repeating events, merging edits). It has **no I/O**: no database, no network, no files, no Tauri. That makes it:

- easy to test: functions take values and return values;
- reusable: storage, sync, providers and the app all speak its types;
- the reason the rest of the app can be "provider-neutral". Google, Nextcloud and Outlook events all become the same `CalendarEvent`.

## Files

### `lib.rs`: the front door

Declares the modules and re-exports the most-used types, so other crates write `calendar_core::Calendar` instead of `calendar_core::model::Calendar`.

```rust
pub mod model;
#[cfg(feature = "sim")]
pub mod sim;
pub use model::*;
```

**Rust concepts:** modules, `pub use` re-exports, the `//!` comment (documents the whole crate), and a module that only exists when the `sim` feature is on ([concepts §8](concepts.md#8-modules-and-visibility), [§16](concepts.md#16-features-and-conditional-compilation)).

### `error.rs`: what can go wrong

One enum, `CoreError`, listing every way validation can fail ("end before start", "unknown time zone", …). `thiserror` turns each `#[error("…")]` into the message text.

**Rust concepts:** enums as error types, derive macros, `Result<T, CoreError>` as the return type of anything that can fail ([§9](concepts.md#9-error-handling-with-thiserror)).

### `model.rs`: the vocabulary

The provider-neutral data types:

| Type | What it is |
|---|---|
| `Account`, `Calendar` | an account (local, Nextcloud, Google …) and one calendar in it |
| `ProviderKind` | `Local`, `Simulated`, `Caldav`, `Google`, `Microsoft`, `Ics` |
| `CalendarEvent` | one stored event: a single event, a series master, or one changed occurrence |
| `EventTiming` | **zoned** (a real instant plus its zone), **floating** (a wall-clock time anywhere) or **all-day** (dates only) |
| `Recurrence` | the repeat rule (RRULE) plus added and cancelled dates |
| `SeriesLink` | "I am occurrence X of series Y" |
| `Occurrence` | one concrete appearance on screen, for example "Tuesday's stand-up" |
| `EventDraft` | what the editor sends when you save |
| `EventContent`, `ObjectContent` | an event as read from a server, used for syncing and merging |
| `SyncStatus` | `Saved`, `PendingSync`, `Synced`, `SyncFailed`, `Conflict`, … |

**Rust concepts:**
- **Enums with data** make impossible states unrepresentable. An `AllDay` timing simply has no time-zone field ([§3](concepts.md#3-structs-and-enums)).
- **Type aliases** (`pub type EventId = String;`) give meaning to plain strings.
- **`Option<T>`** for "maybe": `recurrence: Option<Recurrence>` is `None` for a one-off event ([§5](concepts.md#5-option-and-result-no-null-no-exceptions)).
- **`impl` blocks** add methods to a type. `ProviderKind::as_str` and `ProviderKind::parse` convert to and from database text.
- **`#[serde(rename_all = "camelCase")]`** makes the JSON field names match TypeScript conventions.

### `occurrence_key.rs`: naming one occurrence

`OccurrenceKey` identifies an occurrence of a series by its *original* start time, the iCalendar `RECURRENCE-ID`. Like `EventTiming`, it comes in three forms: instant, floating or date.

**Rust concepts:**
- **Implementing standard traits by hand.** `Display` makes `key.to_string()` produce `20261029T083000Z`; `FromStr` makes `"20261029".parse::<OccurrenceKey>()` work. That pair gives a lossless round trip to text ([§6](concepts.md#6-traits-shared-behaviour)).
- **`#[derive(PartialOrd, Ord, Hash)]`**, so keys can be sorted and used in sets and maps.

### `timing.rs`: time zones and daylight saving

Turns timings into real instants and checks them:

- `resolve_local` turns "02:30 in Berlin" into an instant, even on the night clocks jump forward, when 02:30 doesn't exist.
- `EventTiming::bounds` gives where an event sits on the timeline for a viewer.
- `validate` checks that the end is after the start and the title isn't too long.

```rust
match tz.from_local_datetime(&local) {
    LocalResult::Single(t) => t.with_timezone(&Utc),
    LocalResult::Ambiguous(earlier, _) => earlier.with_timezone(&Utc), // clocks went back
    LocalResult::None => { /* clocks jumped forward: shift by the gap */ }
}
```

**Rust concepts:** a `match` that forces you to handle every daylight-saving case ([§4](concepts.md#4-match-and-pattern-matching)); `impl EventTiming` adding methods to a type defined in another file; `const` values like `MAX_TITLE_CHARS`; a `#[cfg(test)] mod tests` block at the bottom ([§17](concepts.md#17-tests)).

### `recurrence.rs`: repeating events (the biggest file)

- `expand` turns a series ("every Monday, except 3 Nov") into the concrete occurrences inside a date range, always **bounded** by the range and by `MAX_ITERATIONS`, so a "repeat forever" rule can't run away.
- `retime_series` and `split_series` handle "this and following" edits: one series becomes two, the old one ending where the new one starts.
- `index_end` works out when a series ends, for the database index.

The real RRULE maths comes from the `rrule` library. This file feeds it the right time zone so "09:00 every Monday" stays at 09:00 across daylight-saving changes.

**Rust concepts:**
- private helper structs and enums (`Prepared`, `Kind`) that the rest of the crate never sees;
- `HashSet` for fast "is this date cancelled?" checks;
- `std::borrow::Cow` to avoid copying a string when it needs no change ([§18](concepts.md#18-numbers-and-small-details));
- many small, well-named tests: around 500 lines of the file are tests, a good place to see how each feature is expected to behave.

### `merge.rs`: combining two people's edits

When you change an event offline and someone else changes it on the server, `three_way(base, local, server)` compares both against the version you both started from:

- a field changed on only one side: take that change;
- a field changed on both sides to *different* values: report a `FieldConflict`;
- cancelled occurrences on both sides: combine them (a set union).

```rust
fn pick<T: PartialEq + Clone>(b: &T, l: &T, s: &T) -> Option<T> {
    if l == b || l == s { Some(s.clone()) }     // local unchanged, or both agree
    else if s == b      { Some(l.clone()) }     // only local changed
    else                { None }                // both changed: conflict
}
```

**Rust concepts:** **generics** with trait bounds. `pick` works for titles, timings, reminder lists and anything else that can be compared and cloned ([§7](concepts.md#7-generics-and-trait-objects-dyn)). Also `Result<ObjectContent, Vec<FieldConflict>>`: the error side carries useful data. And `&'static str` for field names written in the source.

### `wire.rs`: the shapes sent to the UI

`WireEvent`, `WireDraft`, `WireTiming` and `WireRange` mirror `apps/desktop/src/types/calendar.ts`. Times are converted to milliseconds and ISO dates so JavaScript can use them. `to_draft` and `from_occurrence` convert in each direction, checking everything that comes from the UI.

**Rust concepts:** `Serialize` and `Deserialize`, with serde attributes to match TypeScript's naming. Keeping **"internal model" and "wire format" separate**, so the database model can change without breaking the UI, and the other way round.

### `sim.rs`: fake data for benchmarks (feature `sim`)

Generates 100, 1,000 or 5,000 realistic events, the same ones every time, for performance tests. It includes its own tiny random-number generator (`Prng`), so the output never changes between runs or library versions.

**Rust concepts:**
- a **tuple struct** (`struct Prng(u32);`), accessed as `self.0`;
- `&mut self` methods that change state;
- `wrapping_add` and `wrapping_mul` (overflow on purpose);
- an explicit lifetime in `pick<'a, T>(&mut self, items: &'a [T]) -> &'a T` ([§12](concepts.md#12-lifetimes-a));
- compiled only when the `sim` feature is on. The desktop app turns it on for its benchmark mode, and other users of the crate leave it out.

## Tests

All tests here are unit tests inside each file. Run them with `cargo test -p calendar-core`. They run in milliseconds because nothing touches the disk or network: that's the payoff of keeping I/O out of the core.
