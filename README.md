# Universal Calendar

A fast, local-first, free and open-source desktop calendar for Linux (KDE Plasma first), designed to unify CalDAV, Google Calendar, Microsoft 365 and ICS subscriptions behind one responsive interface. The app needs no hosted backend, telemetry or developer-run account.

> **Status: Milestone 1 (desktop foundation and performance prototype).**
> The app runs on **simulated data only**. Nothing is persisted yet and no
> calendar providers are connected. See [Roadmap](#roadmap).

![Month view](docs/screenshots/light-month.png)

| Week view, dark theme | Event editor |
|---|---|
| ![Week view, dark](docs/screenshots/dark-week.png) | ![Editor](docs/screenshots/light-editor.png) |

## Features (Milestone 1)

- Tauri 2 desktop shell with a Svelte 5 + TypeScript (strict) + Vite frontend.
- Month, week and day views powered by the MIT-licensed [SVAR Svelte Calendar](https://github.com/svar-widgets/calendar), kept behind an adapter so it can be replaced.
- Multiple simulated accounts and calendars, including read-only ones. Each calendar has a colour picker and a visibility toggle.
- Event creation and editing (click, drag-to-create, or **N**), drag/resize to move events, and timed and all-day events.
- Optimistic updates. A failed save is rolled back with a visible message (try *Fail next save* in the sidebar).
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
- Deterministic data generator (100 / 1,000 / 5,000 events) and a scripted benchmark that runs in the real WebKitGTK window. See [docs/performance.md](docs/performance.md).

## Supported platforms

Linux (developed on Ubuntu 24.04; target desktop KDE Plasma). Windows and macOS are architecturally possible through Tauri but not yet built or tested.

## Prerequisites

- Rust stable ≥ 1.90 (`rustup`), with `rustfmt` and `clippy`
- Node.js ≥ 22 (tested with 26.13 LTS) and npm
- Tauri's Linux system libraries:

  ```sh
  sudo apt install pkg-config libwebkit2gtk-4.1-dev libssl-dev \
    libayatana-appindicator3-dev librsvg2-dev libxdo-dev file
  ```

- Optional, for headless screenshots and benchmarks: `xvfb xauth imagemagick`

## Build and run

```sh
cd apps/desktop
npm install
npx tauri dev          # development window with hot reload
npx tauri build        # release binary + .deb + AppImage under target/release/bundle/
```

You can also run the frontend alone in a browser (no Tauri APIs needed):

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
cargo clippy --all-targets -- -D warnings
cargo test
```

## Benchmark

```sh
(cd apps/desktop && npx tauri build)
python3 scripts/bench.py --label my-machine          # 100/1,000/5,000 events + 30 s idle sample
scripts/screenshot.sh docs/screenshots/month.png     # optional
```

Both scripts use Xvfb and a private D-Bus session automatically when no display is available. Results are written to `docs/benchmarks/`; methodology and current numbers are in [docs/performance.md](docs/performance.md).

## Repository layout

```text
apps/desktop/src/            Svelte UI: components, stores, services (mock data, perf), bench runner
apps/desktop/src-tauri/      Thin Tauri 2 shell (no calendar logic)
docs/                        Architecture, performance, provider/sync design, decisions
scripts/                     Benchmark and screenshot tooling
```

The Rust calendar crates (`crates/calendar-core`, `calendar-storage`, …) arrive in Milestone 2; see [docs/architecture.md](docs/architecture.md).

## Roadmap

1. **Desktop foundation and performance prototype** ← current
2. Rust calendar core, SQLite storage, Tauri commands, recurrence
3. CalDAV (Nextcloud reference) with offline queue
4. Google Calendar and Microsoft Graph
5. Production quality: recurrence editing, notifications, agenda view, ICS subscriptions, packaging

## License

GPL-3.0-or-later. See [LICENSE](LICENSE). Third-party dependencies are under GPL-compatible permissive licenses; see [docs/decisions/dependencies.md](docs/decisions/dependencies.md).
