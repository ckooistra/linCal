# Architecture

This describes the code after **Milestone 2** (Rust core + SQLite) and the boundaries that later milestones fill in.

## Guiding rules

- The calendar engine is Rust and **independent of Tauri**. Tauri is a thin bridge.
- The UI consumes **provider-neutral** types only.
- **Local-first**: SQLite is the source of displayed events. From Milestone 3, sync runs in background workers.
- The calendar **widget sits behind an adapter**, so SVAR can be replaced without touching application logic.
- **Nothing blocks the UI**: no provider calls, sync, database writes or recurrence expansion on the interaction path.

## Layout

```text
crates/
├── calendar-core/            No I/O, no Tauri
│   ├── model.rs              Account, Calendar, CalendarEvent, EventTiming, Recurrence, SeriesLink, Occurrence, SyncStatus
│   ├── timing.rs             Instants for zoned/floating/all-day timings, DST-gap/overlap resolution, validation
│   ├── occurrence_key.rs     RECURRENCE-ID / EXDATE values (instant, floating, date)
│   ├── recurrence.rs         Bounded range expansion (rrule crate), series retiming, index bounds
│   ├── wire.rs               Serializable shapes shared with the UI (mirror of src/types/calendar.ts)
│   └── sim.rs                Deterministic simulated dataset (feature "sim")
└── calendar-storage/         SQLite via SQLx; depends only on calendar-core
    ├── migrations/           Embedded, applied on open
    ├── src/lib.rs            Store: queries, transactional writes, settings
    ├── src/codec.rs          Row ↔ model mapping
    └── examples/bench_queries.rs

apps/desktop/
├── src-tauri/src/
│   ├── lib.rs                Opens the Store in setup, registers commands
│   ├── commands.rs           One call per command into calendar-storage; error → {code, message}
│   └── bench.rs              Benchmark-only hooks (env-var controlled)
└── src/
    ├── types/calendar.ts     UI model (matches wire.rs)
    ├── services/
    │   ├── backend.ts        Desktop (Tauri + SQLite) or preview (in-memory mock) data context
    │   ├── tauriEventSource.ts  EventSource over Tauri commands
    │   ├── eventSource.ts    The data boundary used by stores
    │   ├── bridge.ts         The only module that imports @tauri-apps/api
    │   ├── mock/             Test and preview fixture (no persistence, pre-expanded recurrence)
    │   ├── recurrence.ts     Repeat presets for the editor
    │   └── time.ts, shortcuts.ts, theme.ts, perf.ts
    ├── stores/               EventStore (range cache, optimistic writes, change feed), calendars, notifications
    ├── components/calendar/  Widget adapter: the only code that imports SVAR
    └── components/, bench/, App.svelte
```

`cargo tree -p calendar-storage` contains no Tauri crates, and the engine builds on its own (`cargo build -p calendar-core -p calendar-storage`).

## Data flow

```text
 Svelte components ──► EventStore (optimistic cache) ──► EventSource
                                                         │
                         TauriEventSource ── invoke ──► commands.rs (thin)
                                                         │
                                         calendar-storage::Store ──► SQLite (WAL)
                                                         │
                                         calendar-core::recurrence (spawn_blocking)
```

### Reading

1. The adapter asks the store for the visible range plus a month-aligned prefetch window.
2. `query_range(range, timeZone)` selects candidate rows using the index columns, with ±14 h slack for floating and all-day values.
3. It loads the recurrence ids of overrides belonging to the matching series.
4. It decodes and **expands series for that range only**, on Tokio's blocking pool.
5. Every occurrence has a stable id: `<seriesId>::<recurrenceId>` for occurrences of a series, otherwise the event id.

The viewer's IANA zone is sent with every call, so all-day dates and floating times resolve exactly as the UI renders them.

### Writing

`EventStore` applies the change optimistically (`pending-local`) and calls `EventSource`. Tauri forwards each call to one `Store` method, which validates in `calendar-core`, checks the calendar is writable, and writes in one transaction.

- **On success** the event becomes `saved`.
- **On failure** the store restores the last confirmed state and shows a message.
- **Changes affecting other occurrences** (series edits, new series, changing the repeat rule) invalidate the cache, and the visible range reloads from SQLite.

| UI action | Scope | Storage method |
|---|---|---|
| Edit or drag a one-off event | `event` | `update_event` |
| Drag one occurrence of a series | `occurrence` | `update_occurrence`: creates or updates an override row |
| Save the editor on a series occurrence | `series` | `update_series`: the series start moves by the same offset; EXDATEs and overrides move with it. Text changes reach overrides that hadn't customised that field. "Does not repeat" turns the series into the edited event. |
| Delete this event (series) | `occurrence` | `delete_occurrence`: adds an EXDATE and drops the override |
| Delete series | `series` | `delete_series`: overrides are removed by `ON DELETE CASCADE` |

### Time and recurrence model

`EventTiming` takes one of three forms:

- **`Zoned`**: UTC instants plus the IANA zone they were authored in.
- **`Floating`**: wall time with no zone.
- **`AllDay`**: dates with an exclusive end.

Recurrence uses the `rrule` crate (RFC 5545), evaluated in the series' own zone, so a 09:30 meeting stays at 09:30 across DST. All-day series expand on pure dates. Expansion is bounded by the requested range and by `MAX_ITERATIONS`. Wall times that fall into a DST gap move forward by the gap, and ambiguous times take the earlier instant (RFC 5545 §3.3.5).

Over the wire, zoned timings are `{kind: "timed", start, end, timeZone}` in epoch ms. Floating timings are the same with `timeZone: null`, resolved in the viewer's zone in both directions. All-day timings are `{kind: "all-day", startDate, endDate}`.

### Schema

The schema is defined in `crates/calendar-storage/migrations/` and has four tables: `accounts`, `calendars`, `events` and `application_settings`.

The `events` table holds one-off events, series masters (`rrule`, `rdates`, `exdates`) and overrides (`series_id`, `recurrence_id`, unique together).

**Occurrences are never stored.** `range_start_ms` / `range_end_ms` index each row; for a series, the end is its last occurrence, or "unbounded". Indexes cover `(range_start_ms, range_end_ms)` and `(calendar_id, range_start_ms)`. Settings hold non-secret JSON values only.

The sync state, pending-operation and provider-metadata tables arrive with CalDAV in Milestone 3.

### Widget adapter

SVAR keeps its own internal event store, and changing its `events` prop rebuilds that store completely. The adapter therefore:

- passes `events` once and then uses SVAR's action bus;
- **intercepts** SVAR's own moves, edits, creates and clicks and routes them through `EventStore`;
- refuses moves of read-only events;
- implements visibility and search as SVAR view filters;
- applies colours through one generated stylesheet;
- loads both theme stylesheets with `fonts={false}`, so nothing is fetched from SVAR's CDN.

A new `invalidated` change makes it reload the visible window. To replace SVAR, implement the same props and `CalendarViewController` from `components/calendar/types.ts`.

## Tauri layer

Commands:

| Group | Commands |
|---|---|
| Data | `list_accounts`, `list_calendars`, `query_range`, `create_event`, `update_event`, `delete_event` |
| Preferences | `set_calendar_color`, `get_settings`, `set_setting` |
| Simulation | `load_simulated_dataset` |
| Benchmark | `bench_*` |

Each decodes its arguments and calls one storage method. Errors reach the UI as `{code, message}` with readable text; database details go to stderr, never to the UI.

The database opens in `setup`. If it can't be opened, the app still starts and shows the reason. Benchmark runs (`UC_BENCH=1`) use a temporary database, and `UC_DATA_DIR` overrides the location for testing.

The capability file grants only `core:default`. The CSP restricts everything to `self`, plus inline styles for SVAR.

## Planned crates (Milestone 3+)

| Crate | Responsibility |
|---|---|
| `calendar-provider-api` | `CalendarProvider` trait, capabilities, errors |
| `calendar-sync` | Durable operation queue, retries/backoff, conflict detection |
| `calendar-caldav`, `-google`, `-microsoft` | Provider adapters |
