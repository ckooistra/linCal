# Architecture

This describes the code after **Milestone 3** (CalDAV sync) and the boundaries that later milestones fill in.

## Guiding rules

- The calendar engine is Rust and **independent of Tauri**. Tauri is a thin bridge.
- The UI consumes **provider-neutral** types only.
- **Local-first**: SQLite is the source of displayed events. From Milestone 3, sync runs in background workers.
- The calendar **widget sits behind an adapter**, so SVAR can be replaced without touching application logic.
- **Nothing blocks the UI**: no provider calls, sync, database writes or recurrence expansion on the interaction path.

## Layout

```text
crates/
├── calendar-ical/            Lossless iCalendar content model, values, VEVENT ⇄ EventContent
├── calendar-provider-api/    CalendarProvider + ObjectCodec traits: objects (href + version + raw data), ChangeSet, errors
├── calendar-caldav/          CalDAV provider + iCalendar codec: discovery, PROPFIND/REPORT, sync-collection, multiget, PUT/DELETE
├── calendar-oauth/           OAuth 2.0 for installed apps: PKCE, loopback redirect, token refresh
├── calendar-google/          Google Calendar API v3 provider + JSON codec (series = master + exceptions)
├── calendar-microsoft/       Microsoft Graph provider + JSON codec (calendarView delta over a moving window)
├── calendar-ics/             Read-only iCalendar feed subscriptions (one object per UID, conditional GET)
├── calendar-sync/            SyncEngine (push → calendars → pull), scheduler, reminder notifications loop, account sign-in, CredentialStore (keyring / memory)
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
    │   ├── agenda.ts         Groups occurrences into days for the agenda and search results
    │   └── time.ts, shortcuts.ts, theme.ts, perf.ts
    ├── stores/               EventStore (range cache, optimistic writes, change feed), calendars, notifications
    ├── components/calendar/  Widget adapter: the only code that imports SVAR
    ├── components/AgendaView.svelte, AgendaList.svelte, SearchResults.svelte  Lists drawn without SVAR
    ├── components/SettingsDialog.svelte, CommandPalette.svelte, Sidebar.svelte  Settings (tabs), Ctrl+K palette, minimal sidebar
    ├── stores/groups.svelte.ts  Calendar groups and which group the visible calendars match
    ├── services/commands.ts  Palette matching (the commands themselves are built in App.svelte)
    └── components/, bench/, App.svelte
```

None of the engine crates depend on Tauri (`cargo tree -p calendar-sync | grep tauri` is empty), and they build on their own.

Dependency direction:

- `calendar-sync` → `calendar-caldav` / `calendar-google` / `calendar-microsoft` → `calendar-provider-api`
- `calendar-google`, `calendar-microsoft` → `calendar-oauth`, and → `calendar-ical` (Google reuses its recurrence reading)
- `calendar-sync` → `calendar-ical` → `calendar-core`
- `calendar-sync` → `calendar-storage` → `calendar-core`

Storage knows nothing about iCalendar or HTTP. It stores parsed `ObjectContent` plus the raw server copy.

## Synchronization

```text
 UI edit ─► Store: write rows + queue operation (one transaction) ─► scheduler.trigger()
                                                                        │ (debounced 1.5 s, or every 5 min, or retry due)
 SyncEngine.sync_account
   1. push   due operations: rebuild the object from rows + last server copy (calendar-ical),
             PUT with If-Match / If-None-Match:* (DELETE with If-Match)
             412 → conflict · network/5xx/429 → retry with backoff · 401 → account error
   2. list   calendars (added / renamed / permissions / removed-unless-pending)
   3. pull   per calendar: sync-collection(token) or ETag listing → multiget changed →
             parse → apply in one transaction (objects with queued changes are skipped)
 ─► "sync-event" (Tauri event) ─► UI refreshes statuses; on changes reloads the visible range
```

The details (queue semantics, conflicts, recovery) are in [synchronization.md](synchronization.md).

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

**Search** (`search_events`) uses the same path over a wider range. The UI asks for two years ahead (the first 200 matches) and two years back (the latest 50). Rows are filtered in Rust before decoding: every word must occur in the title, location or description, compared after Unicode lower-casing, which SQLite's `LIKE` would not do. Only the matching series are then expanded. An override whose text no longer matches is left out, even if its series matches, and vice versa.

### Writing

`EventStore` applies the change optimistically (`pending-local`) and calls `EventSource`. Tauri forwards each call to one `Store` method, which validates in `calendar-core`, checks the calendar is writable, and writes in one transaction.

- **On success** the event becomes `saved`.
- **On failure** the store restores the last confirmed state and shows a message.
- **Changes affecting other occurrences** (series edits, new series, changing the repeat rule) invalidate the cache, and the visible range reloads from SQLite.

| UI action | Scope | Storage method |
|---|---|---|
| Edit or drag a one-off event | `event` | `update_event` |
| Drag one occurrence of a series | `occurrence` | `update_occurrence`: creates or updates an override row |
| Editor on a series occurrence: *this event* | `occurrence` | `update_occurrence` (as dragging) |
| Editor: *all events* | `series` | `update_series`: the series start moves by the same offset; EXDATEs and overrides move with it. Text changes reach overrides that hadn't customised that field. "Does not repeat" turns the series into the edited event. |
| Editor: *this and following events* | `following` | `update_following`: `recurrence::split_series` ends the series before the occurrence (UNTIL at its last earlier occurrence, or the COUNT so far) and a new series (id chosen by the UI) starts there with the remaining COUNT. Later overrides, EXDATEs and RDATEs move to it; then `update_series` applies the edit to the new series. An unchanged rule continues as before. At the first occurrence this is `update_series`. |
| Delete: *this event* | `occurrence` | `delete_occurrence`: adds an EXDATE and drops the override |
| Delete: *this and following* | `following` | `delete_following`: ends the series before the occurrence and drops later overrides |
| Delete: *all events* | `series` | `delete_series`: overrides are removed by `ON DELETE CASCADE` |

UNTIL is stored as written (RFC 5545 wants a date for all-day series and a local time for floating ones). Before evaluation it is converted to the UTC form the `rrule` crate requires; until October 2026 such rules were rejected.

### Time and recurrence model

`EventTiming` takes one of three forms:

- **`Zoned`**: UTC instants plus the IANA zone they were authored in.
- **`Floating`**: wall time with no zone.
- **`AllDay`**: dates with an exclusive end.

Recurrence uses the `rrule` crate (RFC 5545), evaluated in the series' own zone, so a 09:30 meeting stays at 09:30 across DST. All-day series expand on pure dates. Expansion is bounded by the requested range and by `MAX_ITERATIONS`. Wall times that fall into a DST gap move forward by the gap, and ambiguous times take the earlier instant (RFC 5545 §3.3.5).

Over the wire, zoned timings are `{kind: "timed", start, end, timeZone}` in epoch ms. Floating timings are the same with `timeZone: null`, resolved in the viewer's zone in both directions. All-day timings are `{kind: "all-day", startDate, endDate}`.

### Schema

The schema is defined in `crates/calendar-storage/migrations/`. Migration 1 creates `accounts`, `calendars`, `events` and `application_settings`; migrations 2 and 3 add the sync tables and columns listed below.

The `events` table holds one-off events, series masters (`rrule`, `rdates`, `exdates`) and overrides (`series_id`, `recurrence_id`, unique together).

**Occurrences are never stored.** `range_start_ms` / `range_end_ms` index each row; for a series, the end is its last occurrence, or "unbounded". Indexes cover `(range_start_ms, range_end_ms)` and `(calendar_id, range_start_ms)`. Settings hold non-secret JSON values only.

Migration 2 (Milestone 3) adds:

- non-secret account `config` (JSON) and sync status;
- per-calendar `remote_url`, `sync_token` and `ctag`;
- `events.remote_href`;
- `remote_objects`: the last server copy and ETag of every object;
- `pending_operations`: the durable queue, at most one operation per object, with a generation counter.

Migration 3 (sync integrity) adds:

- `remote_objects.presence`, so an object's existence on the server is tracked separately from its version;
- `import_error` and `import_warnings`, recording objects that couldn't be read or were read only approximately;
- `events.base_recurrence_id`, so a moved override is matched to its component in the server copy.

### Widget adapter

SVAR keeps its own internal event store, and changing its `events` prop rebuilds that store completely. The adapter therefore:

- passes `events` once and then uses SVAR's action bus;
- **intercepts** SVAR's own moves, edits, creates and clicks and routes them through `EventStore`;
- refuses moves of read-only events;
- implements visibility as a SVAR view filter;
- applies colours through one generated stylesheet;
- loads both theme stylesheets with `fonts={false}`, so nothing is fetched from SVAR's CDN.

A new `invalidated` change makes it reload the visible window. To replace SVAR, implement the same props and `CalendarViewController` from `components/calendar/types.ts`.

The **agenda** (SVAR's own agenda is a PRO feature) is a separate list component implementing the same controller. It loads its month through `EventStore`. `App.svelte` swaps it for the grid adapter and reopens the grid at the date the agenda showed. **Search results** cover the calendar while a query is entered; the calendar stays mounted underneath.

## Tauri layer

Commands:

| Group | Commands |
|---|---|
| Data | `list_accounts`, `list_calendars`, `query_range`, `search_events`, `create_event`, `update_event`, `delete_event` |
| Preferences | `set_calendar_color`, `get_settings`, `set_setting` |
| Simulation | `load_simulated_dataset` (benchmark runs only; replaces simulated accounts only) |
| Accounts & sync | `add_caldav_account`, `remove_account`, `sync_now`, `account_statuses`, `resolve_conflict` |
| Benchmark | `bench_*` |

Each decodes its arguments and calls one storage method. Errors reach the UI as `{code, message}` with readable text; database details go to stderr, never to the UI.

The database opens in `setup`. If it can't be opened, the app still starts and shows the reason. Outside benchmark runs, setup then removes demo data left by older versions, and creates the *Personal* calendar (account `local`, never synchronized) if no local account exists. It also starts the reminder loop, which shows due reminders through `tauri-plugin-notification`; the `notifications` setting turns them off. Benchmark runs (`UC_BENCH=1`) use a temporary database, and `UC_DATA_DIR` overrides the location for testing.

The capability file grants only `core:default` and the window controls; the opener plugin, which opens the browser for OAuth sign-in, is called from Rust only. The CSP restricts everything to `self`, plus inline styles for SVAR.

The sync engine runs on Tauri's Tokio runtime. Progress reaches the UI as `sync-event` events: `syncing`, `synced` (with a `changed` flag) and `failed` (with an `offline` flag).
