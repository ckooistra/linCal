# calendar-ical

**Where:** `crates/calendar-ical/`
**Depends on:** `calendar-core`, `chrono`, `chrono-tz`, `thiserror`.
**Used by:** the CalDAV, Google and ICS providers, and the sync engine.

## Its job in the architecture

**iCalendar** (`.ics`, RFC 5545) is the text format calendars have exchanged for decades:

```text
BEGIN:VEVENT
UID:abc123
DTSTART;TZID=Europe/Berlin:20261110T090000
SUMMARY:Stand-up
RRULE:FREQ=WEEKLY;BYDAY=MO
END:VEVENT
```

CalDAV servers such as Nextcloud store exactly this. This crate reads it into `calendar-core` types and writes it back.

Its key promise is **lossless round trips**. When you change an event's title, every property the app doesn't understand (attendees, colours, vendor extensions) is written back untouched. That's why it keeps its own detailed model instead of using a lossy library.

Google's provider reuses this crate too: it converts Google's JSON into iCalendar text first, so recurrence handling lives in one place.

## Files

### `lib.rs`

Declares the three modules, re-exports the main functions, and defines `IcalError` (`Syntax` or `Value`).

**Rust concepts:** a small crate API made of a few `pub use` lines ([§8](concepts.md#8-modules-and-visibility)).

### `content.rs`: parsing and writing the text format

The generic model: a `Component` (like `VEVENT`) has `Property`s (like `SUMMARY`), each with `Param`s (like `TZID=…`), and child components. Everything is kept in its original order.

- `parse` handles "folded" long lines, escaping and quoted parameters.
- `to_ics` writes it back, folding lines at 75 bytes without cutting a multi-byte character in half.
- `escape_text` and `unescape_text` handle commas, semicolons and newlines inside values.

```rust
pub fn children<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Component> + 'a {
    self.components.iter().filter(move |c| c.name.eq_ignore_ascii_case(name))
}
```

**Rust concepts:**
- **Recursive data:** `Component` contains `Vec<Component>`.
- **Returning an iterator** with `impl Iterator<…> + 'a`. The caller gets a lazy filter over the component's own data, with no copying. The `'a` says the iterator borrows from `self` ([§11](concepts.md#11-iterators), [§12](concepts.md#12-lifetimes-a)).
- **Builder-style methods:** `Property::new("X", v).with_param("TZID", tz)` takes `self` by value and returns it, so calls chain.
- **`impl Into<String>` parameters** accept both `&str` and `String`.
- **String handling:** `chars()` (Unicode characters), `len_utf8()`, `strip_prefix`, `split_once`. Rust strings are UTF-8, and the type system keeps you honest about bytes versus characters.
- **`Vec::retain`** removes items in place.

### `values.rs`: dates, times, durations and time zones

- Parses `20261110`, `20261110T090000Z` and `20261110T090000` (with or without a `TZID`) into `IcalTime`.
- Parses durations like `PT15M` or `-P1D`.
- `resolve_tzid` maps a `TZID` to a real time zone. It also understands Windows names like "W. Europe Standard Time", which Outlook writes.

```rust
pub enum IcalTime {
    Date(NaiveDate),
    Utc(DateTime<Utc>),
    Local(NaiveDateTime, Option<String>),   // Option = TZID, None = floating
}
```

**Rust concepts:** an enum keeps "what was written" before deciding what it means. Small private helpers (`bad`, `parse_date`) keep the public functions readable. Errors are built with `map_err` to add context.

### `events.rs`: VEVENT ⇄ `calendar-core` events

- `read_events` walks a VCALENDAR and returns an `ObjectEvents`: the series master, its overrides (moved or changed occurrences), and **warnings** for things it couldn't understand. One odd property never makes the whole object fail.
- `write_object` and `write_events` edit an existing object **in place**, changing only the properties this app owns (`SUMMARY`, `DTSTART`, `RRULE`, `VALARM`, …). Everything else stays as it was.

**Rust concepts:**
- **`&mut Vec<String>` as an "out" parameter** for collecting warnings while parsing.
- **Private enums** (`Resolved`) for steps between "raw text" and "final value".
- **`pub use calendar_core::EventContent as EventData;`** re-exports under a local name.
- **Taking ownership to transform:** `into_content(self)` consumes an `ObjectEvents` and returns an `ObjectContent` without copying the data.

## Tests

- **Unit tests** in `content.rs` and `values.rs` check folding, escaping and every date form.
- **Integration tests** in `tests/events.rs` read real files from `tests/fixtures/` (a recurring Nextcloud event, for example). They check that reading and writing again keeps everything.

```rust
std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/nextcloud-recurring.ics"))
```

`env!("CARGO_MANIFEST_DIR")` is filled in at compile time with the crate's folder, so the test finds its fixture from wherever it's run.
