# AGENTS.md — Universal Calendar

## Mission
Build a fast, local-first, free and open-source desktop calendar for Linux (primarily KDE Plasma) that unifies multiple calendar services. The UI must remain responsive during network access, synchronization, and database activity. Preserve a path to Windows and macOS support.

## Current priority
**Implement Milestone 4: Google Calendar and Microsoft Graph** (see `PROJECT_SPEC.md` §14), alongside the existing CalDAV support. Settle the provider-contract questions in `docs/provider-integration.md` first. Use official native-app OAuth flows (PKCE, loopback redirect), and do not embed confidential client secrets.

Status:
- Milestones 1–3 are complete. Milestone 3 (CalDAV) has been validated against the user's Nextcloud.
- Milestone 3 audit findings have been addressed, except performance on target KDE hardware (F19), which still needs measuring there.
- Milestone 4: Google verified with a live account; Microsoft (including whole-series editing) passes the API-fake tests and still needs a live check.
- Do not start Milestone 5 until Milestone 4 is accepted.

## Required stack
- **Desktop:** Tauri 2
- **Frontend:** Svelte 5, TypeScript (strict mode), Vite; no SvelteKit without a documented reason
- **Calendar widget:** SVAR Svelte Calendar's free/MIT edition only; evaluate an open-source alternative if needed
- **Core:** Rust, Tokio, Reqwest
- **Persistence (later milestones):** SQLite and SQLx
- **Dates/time zones:** appropriate Rust iCalendar and time-zone libraries, evaluated for correctness
- **Credentials (later):** system keyring / Secret Service, never plaintext token storage

Use stable compatible releases and verify their licenses. No commercial calendar component, required hosted backend, or paid API dependency for core features. The project should be GPL-3.0-or-later, subject to dependency license review.

## Architecture rules
1. **Keep the Rust calendar engine independent of Tauri.** Tauri is a thin command/event bridge; do not put calendar logic in Tauri commands.
2. Keep provider adapters (CalDAV, Google Calendar, Microsoft Graph, ICS) separate from the core and UI. UI components consume provider-neutral types.
3. Build a **local-first** experience: SQLite is the source for displayed events; remote synchronization runs in background workers.
4. Separate calendar rendering behind a frontend adapter so SVAR can be replaced without rewriting application logic.
5. Support multiple accounts and multiple calendars per account. Respect read-only calendars and provider capability differences.
6. No proprietary service account, web server, telemetry, or cloud backend is required to use the application.
7. Favor simple modular code over speculative abstractions. Do not create microservices.

Suggested layout (adapt if needed):

```text
apps/desktop/src/                # Svelte UI, stores, view adapters
apps/desktop/src-tauri/          # Thin Tauri integration
crates/calendar-core/            # Provider-neutral models and recurrence
crates/calendar-storage/         # SQLite access and migrations
crates/calendar-sync/            # Sync queue, retries, conflicts
crates/calendar-provider-api/    # Provider contracts and capabilities
crates/calendar-caldav/          # CalDAV adapter
crates/calendar-google/          # Google adapter
crates/calendar-microsoft/       # Microsoft adapter
docs/                             # Architecture, decisions, benchmarks
```

Only create modules when the corresponding milestone needs them.

## Responsiveness — non-negotiable
- **Never block the UI on provider calls, sync, database writes, or recurrence expansion.**
- Keep drag/resize interaction in the frontend. Send the completed operation to Rust.
- Use optimistic UI updates, followed by durable local persistence and background synchronization. Clearly show failures/conflicts; never silently lose user changes.
- Fetch/render events for the visible date range plus bounded prefetch, not full calendar history.
- Support incremental event updates and avoid replacing the entire event store for one changed event.
- Cancel obsolete requests and avoid unnecessary reactive recomputations.
- Use bounded concurrency and appropriate async/blocking execution in Rust; no busy polling.

### Proposed performance goals (measure, don't assume)
- Month navigation: <50 ms with cached events
- Open event editor: <50 ms
- Calendar visibility toggle: <30 ms
- Cached event display and local search: <100 ms
- Event dragging/resizing: smooth 60 FPS where hardware permits
- Idle CPU: near zero

These are targets, not guarantees; record hardware, datasets, methodology, and actual measurements. Test 100, 1,000, and 5,000 simulated events, including overlapping events and multiple calendars. Optimize proven bottlenecks rather than guessing.

## Calendar correctness
- Use provider-neutral account, calendar, event, occurrence, attendee, reminder, and sync-state models.
- Correctly represent all-day dates, time zones, DST transitions, floating times, recurring events, exceptions, canceled occurrences, and stable identifiers.
- Recurrence must be supported without SVAR PRO. The Rust core should expand bounded visible-range occurrences; editing one occurrence, whole series, and future occurrences requires careful provider-aware semantics.
- Prefer well-tested open-source iCalendar/RRULE libraries. Do not implement recurrence as fixed-second arithmetic.
- Preserve provider metadata and version identifiers needed for safe round trips.

## Future provider/sync rules (not Milestone 1)
- **CalDAV first**, using Nextcloud as a reference server without hard-coding Nextcloud assumptions; implement discovery, ETags, and sync tokens where available.
- Then Google Calendar API and Microsoft Graph; use official native-app authorization flows (OAuth with PKCE where applicable), incremental sync, pagination, and rate-limit handling.
- ICS subscriptions should be read-only by default.
- Persist an offline operation queue and sync cursors; recover after restart/crash.
- Detect conflicts with ETags/provider versions. Retry safely with backoff and avoid duplicate writes.
- Keep sync failures isolated per account/provider.

## UX requirements
- Month, week, and day views; calendar/account sidebar; calendar colors and visibility toggles.
- Event creation/editing, day navigation, all-day events, overlapping events, and drag/resize where supported.
- Accessible keyboard navigation and light/dark themes; work well on KDE Plasma and HiDPI displays.
- Plan shortcuts: T=today, N=new event, M=month, W=week, D=day, Ctrl+F=search; do not trigger shortcuts while typing in inputs.
- Agenda view, desktop notifications, subscriptions, invitations, and advanced recurrence editing belong to later milestones unless needed to validate the UI.

## Coding and security practices
- Idiomatic Rust; `cargo fmt`, `cargo clippy`, documented errors, and unit tests.
- Strict TypeScript; Svelte 5 conventions; small components; cleanup of listeners, subscriptions, and pending async operations.
- Database migrations and transactions for related writes; indexes for date-range queries; no unbounded recurrence tables.
- Keep credentials in a secure OS credential store; never log tokens or embed client secrets in the application.
- Graceful, user-friendly error states; no unhandled promise rejections or hidden sync failures.
- No new dependencies without a clear reason and license check.

## Milestone workflow
1. Inspect repository and available development tooling before modifying files.
2. State a concise implementation plan and identify blockers.
3. Implement the **smallest complete vertical slice** for the active milestone.
4. Run formatters, type checks, builds, and tests; repair failures when possible.
5. Benchmark representative UI interactions and document measurements rather than claiming unverified performance.
6. Update `README.md` and relevant docs with setup/run/test steps and decisions.
7. End with a summary of changed files, test results, measured performance, limitations, and next steps.
8. Do **not** advance to another milestone autonomously.

## Milestone 1 acceptance criteria
- App builds and launches on Linux with Tauri 2, Svelte 5, Vite, and TypeScript.
- SVAR free edition is integrated after verifying licensing and features.
- Working month/week/day navigation, simulated calendars, color/visibility controls, and mock event creation/editing.
- Deterministic event-data generator with 100/1,000/5,000-event scenarios.
- Basic performance instrumentation and documented real measurements when runnable.
- Responsive UI while data loads; no required remote services.
- All tests/build checks that can run in the environment pass, or blockers are explicitly reported.

## Working with this file
Treat this file as persistent project guidance. If another project specification exists, follow both; flag genuine conflicts rather than silently disregarding either. Ask questions only for blockers that cannot be resolved safely through repository inspection and reasonable defaults. Do not claim an implementation, benchmark, or test passed unless it actually ran.
