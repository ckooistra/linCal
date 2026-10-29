# Architecture

This describes the code as it stands after **Milestone 1** and the boundaries that later milestones fill in.

## Guiding rules

- The calendar engine is Rust and **independent of Tauri**. Tauri is a thin bridge.
- The UI consumes **provider-neutral** types only.
- **Local-first**: from Milestone 2, SQLite is the source of displayed events and sync runs in background workers.
- The calendar **widget sits behind an adapter**, so SVAR can be replaced without touching application logic.
- **Nothing blocks the UI**: no provider calls, sync, database writes or recurrence expansion on the interaction path.

## Current layout (Milestone 1)

```text
apps/desktop/
├── src/
│   ├── types/calendar.ts             Provider-neutral model (Account, Calendar, CalendarEvent, EventTiming, SyncStatus)
│   ├── services/
│   │   ├── eventSource.ts            EventSource interface: the data boundary the UI talks to
│   │   ├── mock/                     Deterministic generator + in-memory EventSource (M1 only)
│   │   ├── time.ts                   Date helpers (calendar-day arithmetic, all-day dates, validation)
│   │   ├── shortcuts.ts, theme.ts    Keyboard mapping; system theme tracking
│   │   ├── perf.ts                   Interaction timing, frame sampling, long-task observer
│   │   └── bridge.ts                 The only module that imports @tauri-apps/api
│   ├── stores/
│   │   ├── events.svelte.ts          EventStore: range cache, optimistic writes, change feed
│   │   ├── calendars.svelte.ts       Accounts/calendars, visibility, colours
│   │   └── notifications.svelte.ts   User-facing messages
│   ├── components/
│   │   ├── calendar/                 Widget adapter: the only code that imports SVAR
│   │   │   ├── CalendarAdapter.svelte
│   │   │   ├── svarMapping.ts        Pure domain ↔ SVAR conversions
│   │   │   └── types.ts              Adapter contract (controller, state, editor/move requests)
│   │   └── Sidebar, Toolbar, EventEditor, PerfPanel, Toasts
│   ├── bench/runner.ts               Scripted UI benchmark
│   └── App.svelte                    Composition root
└── src-tauri/                        Thin Tauri shell; M1 commands are benchmark hooks only
```

## Data flow

```text
 EventSource (mock in M1; Tauri → Rust core → SQLite in M2)
      │ queryRange(range, signal) / create / update / delete
      ▼
 EventStore ── change feed: upsert | remove | loaded | reset ──┐
      ▲                                                         ▼
      │ create/update/remove (optimistic)            CalendarAdapter ── provide-data / update-event ──► SVAR
      │                                                         │
 App.svelte ◄── editor/move requests, view state ───────────────┘
```

### Loading

When SVAR reports a new visible range (`request-data`), the adapter does the following:

1. Computes a month-aligned window: the visible range plus one month either side.
2. If the window is cached, hands those events to SVAR immediately.
3. Otherwise loads the visible range at **visible priority** and then the window at **prefetch priority**. A new visible load aborts older visible and prefetch loads (`AbortController`), so a stale response can never overwrite newer state.

Navigating to an adjacent month or week therefore renders from data that is already present.

### Writes

`EventStore` applies an edit to its cache straight away (`syncStatus: "pending-local"`) and emits one `upsert`. The adapter then updates that single event in SVAR. The store persists through `EventSource`:

- **On success** the event becomes `saved`.
- **On failure** the store restores the last confirmed state and shows a message, but only if no newer local edit has replaced it. Local revision numbers decide this.
- **Deletes in flight** are tombstoned, so a range load cannot bring them back.
- **Pending local edits** win over freshly loaded data.

### Widget adapter

SVAR keeps its own internal event store, and changing its `events` prop rebuilds that store completely. The adapter therefore:

- passes `events` once and then talks to SVAR only through its action bus (`provide-data`, `update-event`, `add-event`, `delete-event`, `filter-events`, `navigate-*`);
- **intercepts** SVAR's own `move-event`, `update-event`, `add-event` and `select-event` actions. Drags, drag-creates and clicks turn into domain requests (`onmove`, `oneditor`), so SVAR never mutates data on its own. Drag feedback stays inside SVAR and only the completed drag reaches the store.
- refuses moves of read-only events and reports them;
- implements visibility and search as SVAR view filters, which only re-run layout;
- applies calendar colours through one generated stylesheet (`.uc-cal-<id>` sets `--wx-color-primary`), so changing a colour never touches event objects;
- loads both SVAR theme stylesheets with `fonts={false}`, which stops SVAR from fetching fonts and icons from its CDN. Switching theme only swaps a class.

To replace SVAR, write a new component with the same props and the `CalendarViewController` from `components/calendar/types.ts`.

### Time model

`EventTiming` is either of the following:

- **Timed**: UTC epoch-ms instants plus the IANA zone it was authored in (`null` means floating).
- **All-day**: `YYYY-MM-DD` dates with an exclusive end, never converted through a time zone.

Day arithmetic uses calendar operations (`setDate`), never fixed 24-hour offsets, and tests run in `Europe/Berlin` to cover DST. Full time-zone conversion, RRULE/EXDATE/RECURRENCE-ID handling and occurrence expansion belong to the Rust core in Milestone 2. The M1 generator emits pre-expanded weekly occurrences with a `seriesId`, only to exercise rendering.

## Tauri layer

`src-tauri` registers three commands, all benchmark-only: `bench_config`, `bench_memory` and `bench_report`. Benchmark mode is controlled only by environment variables set by the person launching the app; the frontend cannot choose output paths. The capability file grants `core:default` and nothing else: no filesystem, shell or HTTP. The CSP restricts everything to `self`, plus inline styles because SVAR positions events with style attributes.

## Planned crates (Milestone 2+)

| Crate | Responsibility | Depends on Tauri? |
|---|---|---|
| `calendar-core` | Provider-neutral models, validation, time zones, recurrence expansion (evaluate `rrule`/`icalendar` crates) | No |
| `calendar-storage` | SQLite via SQLx, migrations, range-indexed queries, WAL | No |
| `calendar-provider-api` | `CalendarProvider` trait, capabilities, errors | No |
| `calendar-sync` | Durable operation queue, retries/backoff, conflict detection | No |
| `calendar-caldav`, `-google`, `-microsoft` | Provider adapters | No |
| `apps/desktop/src-tauri` | Commands/events forwarding to the above | Yes |

In M2, `MockEventSource` is replaced by a `TauriEventSource` that implements the same interface over `invoke()` and events. Stores and components stay unchanged.
