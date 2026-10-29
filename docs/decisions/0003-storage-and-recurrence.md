# 0003: SQLite storage model and recurrence handling

- **Date:** 2026-10-29
- **Status:** accepted (Milestone 2)

## Decisions

1. **Library.** Recurrence uses the `rrule` crate (0.14, MIT OR Apache-2.0, RFC 5545) rather than a hand-written engine. Rules are evaluated in the series' own IANA zone via `chrono-tz`, so wall-clock times survive DST.
2. **No occurrence table.** Series are stored once, as a master row with `rrule`, `rdates` and `exdates`, and expanded on demand for the queried range only, with a hard iteration cap. This avoids an unbounded occurrence table and keeps edits cheap.
3. **Overrides.** A modified single occurrence is an **override row** linked by `series_id` + `recurrence_id`, with id `<seriesId>::<recurrenceId>`. That id is the same as the generated occurrence's id, so the UI's cache key doesn't change when an occurrence is moved. A cancelled occurrence is an EXDATE on the master.
4. **Index columns.**
   - Each row carries zone-independent `range_start_ms`/`range_end_ms`.
   - Floating and all-day values use their wall time read as UTC, and queries add ±14 h slack before precise filtering in Rust with the viewer's zone.
   - For finite series the end is computed from the last occurrence; unbounded series store the maximum integer.
5. **Three timing kinds in the core.** `Zoned`, `Floating` and `AllDay`. The UI keeps its two-variant shape; floating is "timed with `timeZone: null`" and is resolved in the viewer's zone.
6. **Series edits.** Editing a series through one occurrence shifts DTSTART by that occurrence's change, measured from where it is displayed, so moved occurrences don't skew the series. EXDATEs and overrides shift with the series. Text changes propagate to overrides that still had the series' old value. Changing the timing kind, or removing the rule, drops overrides.
7. **Floating rules expand in the viewer's zone.** The DST rules of the zone the frontend reports apply.
8. **Static SQL only.** SQLx 0.9 rejects dynamic SQL; queries are string literals (`concat!` for the shared column list) with bound parameters.
9. **Dataset generator.** The deterministic generator moved to Rust (`calendar-core`, feature `sim`) so storage benchmarks and the app share it. The TypeScript generator remains only as a fixture for tests and the browser preview.

## Not yet covered

- Editing "this and following" occurrences, which needs provider-aware series splitting.
- RDATE editing in the UI.
- EXRULE.
- Per-series expansion caching.
- The sync tables: pending operations, sync state and provider metadata (Milestone 3).
