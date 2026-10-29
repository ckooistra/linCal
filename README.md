# Universal Calendar

A fast, local-first, free and open-source desktop calendar for Linux (KDE Plasma first), designed to unify CalDAV, Google Calendar, Microsoft 365 and ICS subscriptions behind one responsive interface. The app needs no hosted backend, telemetry or developer-run account.

> **Status: Milestone 3 (CalDAV).** CalDAV accounts (Nextcloud, Fastmail,
> Radicale, Baïkal, …) sync both ways with an offline queue; everything is
> stored locally in SQLite. Google and Microsoft arrive in Milestone 4.
> Simulated demo calendars remain available. See [Roadmap](#roadmap).

![Month view](docs/screenshots/light-month.png)

| Week view, dark theme | Event editor |
|---|---|
| ![Week view, dark](docs/screenshots/dark-week.png) | ![Editor](docs/screenshots/light-editor.png) |

## Features

### CalDAV accounts

- **Add CalDAV account…** in the sidebar: enter the server address (for Nextcloud, just `https://your.cloud`), the user name and an **app password**.
- **Discovery** follows the standard route (`/.well-known/caldav` → principal → calendar home). Every event calendar is listed with its server colour; shared read-only calendars are detected.
- **Two-way sync** in the background:
  - Every change is saved locally first, then uploaded with ETag checks, so another client's edits are never silently overwritten.
  - Server changes are fetched incrementally with sync tokens where supported (RFC 6578), falling back to an ETag comparison.
  - It syncs on start-up, a few seconds after local edits, every 5 minutes, and on **Sync now**.
- **Offline:**
  - Changes are queued in SQLite and survive restarts and crashes.
  - Failed uploads retry with backoff (30 s doubling to 1 h).
  - The sidebar shows *Offline*, *N changes waiting* or errors.
- **Conflicts:** if an event changed on the server and here, it is marked with a dashed red outline, and the editor offers *Keep mine* or *Use server version*.
- **Round trips are lossless.** Properties this app doesn't edit (alarms, attendees, vendor `X-` fields, VTIMEZONEs) are kept exactly as the server sent them.
- **Credentials** are stored in the system keyring via the Secret Service API (KWallet on Plasma, GNOME Keyring elsewhere), never in the database or logs.

### Calendar

- Tauri 2 desktop shell with a Svelte 5 + TypeScript (strict) + Vite frontend.
- Month, week and day views powered by the MIT-licensed [SVAR Svelte Calendar](https://github.com/svar-widgets/calendar), kept behind an adapter so it can be replaced.
- Multiple simulated accounts and calendars, including read-only ones. Each calendar has a colour picker and a visibility toggle.
- Event creation and editing (click, drag-to-create, or **N**), drag/resize to move events, and timed and all-day events.
- **Local persistence** in SQLite (WAL, migrations). Events, calendar colours, hidden calendars, theme and dataset choice survive restarts. The database lives at `~/.local/share/org.universalcalendar.desktop/calendar.db`.
- **Recurring events** (RRULE, RDATE, EXDATE, moved occurrences) expanded by the Rust core for the visible range only, time-zone and DST correct:
  - **Dragging** an occurrence moves just that one.
  - **The editor** edits the whole series and offers *Delete this event* or *Delete series*.
  - **Repeat presets:** daily, weekdays, weekly, monthly, yearly. Custom rules are kept.
- Optimistic updates. A failed save is rolled back with a visible message.
- Visible-range loading with adjacent-month prefetch and cancellation of obsolete loads.
- Debounced search over loaded events.
- Light and dark themes that follow the system preference.
- Keyboard shortcuts:

  | Key | Action |
  |---|---|
  | `T` | Today |
  | `N` | New event |
  | `M` / `W` / `D` | Month / week / day view |
  | `J` / `K` | Next / previous |
  | `Ctrl+F` | Search |
  | `Esc` | Close dialog |
  | `Ctrl+Shift+P` | Toggle performance panel |

  Shortcuts don't fire while you're typing in a field.
- Deterministic data generator (100 / 1,000 / 5,000 events, written in Rust), a scripted UI benchmark that runs in the real WebKitGTK window, and a storage benchmark. See [docs/performance.md](docs/performance.md).
- On first run the app seeds 1,000 simulated events in three demo accounts. **Choosing a dataset in the sidebar replaces the simulated events only**; CalDAV accounts are not touched.

## Supported platforms

Linux (developed on Ubuntu 24.04; target desktop KDE Plasma). Windows and macOS are architecturally possible through Tauri but not yet built or tested.

## Requirements at runtime

- A Secret Service provider (the `org.freedesktop.secrets` D-Bus API) for storing CalDAV passwords:
  - **KDE Plasma:** KWallet implements it since KDE Frameworks 5.97. It must be enabled (see KWallet settings) and the wallet unlocked.
  - **Elsewhere:** GNOME Keyring works too.

  Without one, adding an account fails with a clear message. Not yet tested on a real Plasma desktop.

## Prerequisites

- Rust stable ≥ 1.90 (`rustup`), with `rustfmt` and `clippy`
- Node.js ≥ 22 (tested with 26.13 LTS) and npm
- Tauri's Linux system libraries:

  ```sh
  sudo apt install build-essential pkg-config libwebkit2gtk-4.1-dev libssl-dev \
    libayatana-appindicator3-dev librsvg2-dev libxdo-dev file
  ```

  `build-essential` provides the C compiler used for Tauri's native parts and for the SQLite that is bundled into the app (no system SQLite is needed).

- Optional, for headless screenshots and benchmarks: `xvfb xauth imagemagick`

## Build and run

```sh
cd apps/desktop
npm install
npx tauri dev          # development window with hot reload
npx tauri build        # release binary + .deb + AppImage under target/release/bundle/
```

You can also run the frontend alone in a browser. That preview uses an in-memory mock with **no persistence and no recurrence expansion**; it exists for quick UI work and for the latency and failure simulation controls:

```sh
cd apps/desktop
npm run dev            # http://localhost:1420
```

## Test and check

```sh
# Frontend (from apps/desktop)
npm run check          # svelte-check + strict TypeScript, warnings fail
npm test               # Vitest unit + component tests (runs with TZ=Europe/Berlin for DST coverage)
npm run build          # production bundle

# Rust (from the repository root)
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build -p calendar-core -p calendar-storage -p calendar-sync   # the engine builds without Tauri
```

### Testing against a real CalDAV server

```sh
scripts/dev-radicale.sh start      # local Radicale on 127.0.0.1:5232 (user test/test, calendar "UC Test")
UC_CALDAV_URL=http://127.0.0.1:5232/ UC_CALDAV_USER=test UC_CALDAV_PASSWORD=test \
  cargo test -p calendar-caldav -p calendar-sync --test real_server -- --nocapture
scripts/dev-radicale.sh stop
```

The script needs Radicale (`python3 -m pip install radicale`, or set `RADICALE=/path/to/radicale`).

The same tests work against Nextcloud or any other server:
- use an **app password** and a **dedicated test calendar**;
- set `UC_CALDAV_CALENDAR` to that calendar's name (default "UC Test").

They only touch events they create, and delete them at the end. Without these variables the tests skip themselves.

For trying the UI on a machine without a Secret Service daemon, `UC_CREDENTIALS=memory` keeps passwords in memory only. They are lost on exit, so use it for testing only.

## Benchmark

```sh
(cd apps/desktop && npx tauri build)
python3 scripts/bench.py --label my-machine          # UI: 100/1,000/5,000 events + 30 s idle sample
cargo run --release -p calendar-storage --example bench_queries   # SQLite + recurrence query latency
scripts/screenshot.sh docs/screenshots/month.png     # optional
```

The UI benchmark uses a temporary database and never touches your calendar. Both scripts use Xvfb and a private D-Bus session automatically when no display is available. Results are written to `docs/benchmarks/`; methodology and current numbers are in [docs/performance.md](docs/performance.md).

## Repository layout

```text
crates/calendar-core/        Provider-neutral models, validation, recurrence, wire types, simulated data
crates/calendar-ical/        Lossless iCalendar parser/writer and VEVENT ⇄ model mapping
crates/calendar-provider-api/ Provider contract (objects, ETags, change sets, errors)
crates/calendar-caldav/      CalDAV client: discovery, listing, sync-collection, conditional writes
crates/calendar-sync/        Sync engine, offline queue processing, scheduler, credentials
crates/calendar-storage/     SQLite (SQLx) persistence, migrations, range queries, sync state
apps/desktop/src/            Svelte UI: components, stores, services, bench runner
apps/desktop/src-tauri/      Thin Tauri 2 shell: commands forward to the crates
docs/                        Architecture, performance, provider/sync design, decisions
scripts/                     Benchmark and screenshot tooling
```

See [docs/architecture.md](docs/architecture.md).

## Roadmap

1. Desktop foundation and performance prototype ✔
2. Rust calendar core, SQLite storage, Tauri commands, recurrence ✔
3. **CalDAV (Nextcloud reference) with offline queue** ← current
4. Google Calendar and Microsoft Graph
5. Production quality: recurrence editing, notifications, agenda view, ICS subscriptions, packaging

## License

GPL-3.0-or-later. See [LICENSE](LICENSE). Third-party dependencies are under GPL-compatible permissive licenses; see [docs/decisions/dependencies.md](docs/decisions/dependencies.md).
