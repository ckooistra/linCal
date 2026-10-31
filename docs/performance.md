# Performance

Responsiveness is a first-class requirement. This page records targets, method, the **measured** results per milestone, and what they do and don't show.

## Targets (from the project spec; goals, not guarantees)

| Operation | Target |
|---|---|
| Month navigation (cached) | < 50 ms |
| Open event editor | < 50 ms |
| Calendar visibility toggle | < 30 ms |
| Cached display / local search | < 100 ms |
| Drag / resize | smooth 60 fps |
| Idle CPU | ≈ 0 |

## Method

`scripts/bench.py` starts the **release build** with `UC_BENCH=1`. The app then drives itself through `apps/desktop/src/bench/runner.ts`, using the same controller, store and adapter code paths as user input, inside the real Tauri/WebKitGTK window. Afterwards the script launches the app normally and samples idle CPU and RSS over 30 s from `/proc`.

### Per-interaction metrics

| Metric | What it covers |
|---|---|
| **p50 / p95 / max** | Start of the action until the frame showing its result has been produced: a `requestAnimationFrame` callback followed by a `MessageChannel` task. This is the user-perceived latency compared with the targets. |
| **JS** | The action plus Svelte's DOM updates, flushed with `flushSync()`. |
| **JS + layout** | The JS time plus a forced synchronous style/layout (`offsetHeight`). |

Whatever remains after JS + layout is paint and compositing, plus any async follow-up work.

### Caveats

- WebKit clamps `performance.now()` to **1 ms**. Single samples are coarse, and raw range-query timings are measured in batches of 100.
- `PerformanceObserver('longtask')` is **not supported** by WebKitGTK 2.52, so long tasks are reported as `null`.
- The **drag proxy** calls the completed-drag path (`store.update` → adapter → SVAR) once per animation frame for 2 s in week view and reports the achieved frame rate. That is **pessimistic** compared with a real drag:
  - each step persists, so SVAR re-renders twice per frame (optimistic, then saved);
  - during a real drag SVAR only moves a projection and persists once, on drop.
- The UI path was also checked by hand with synthetic X11 input (XTest): a real mouse drag in week view, an all-day drag, a drag on a read-only event (refused), and a failed save (rolled back). Those checks were not timed.

### Datasets

`generateDataset({ seed: 20261029, anchor: 2026-06-15, eventCount })` is deterministic: the same seed always gives the same events. It produces:

- 8 calendars in 3 accounts, 2 of them read-only;
- events spread over ±6 months;
- ~12% packed into "busy days" with many overlaps;
- ~10% recurring-series occurrences (pre-expanded);
- ~8% all-day or multi-day events.

The scenarios run at 100, 1,000 and 5,000 events.

## Results: settings dialog, calendar groups, command palette, 2026-10-31

Raw data: [`benchmarks/20261031-214414-vm-xvfb-m5-ui.json`](benchmarks/20261031-214414-vm-xvfb-m5-ui.json). Same VM and Xvfb setup (software rendering).

| p50 / p95 ms | 100 events | 1,000 events | 5,000 events | Target |
|---|---|---|---|---|
| open settings | 27 / 85 (first open) | 33 / 42 | 51 / 61 | < 50 |
| close settings | 12 / 13 | 18 / 21 | 36 / 37 | immediate |
| open command palette | 24 / 31 | 30 / 32 | 48 / 51 | < 50 |
| activate a calendar group (half the calendars, then all) | 14 / 25 | 40 / 55 | 54 / 60 | < 30 |
| calendar visibility toggle | 15 / 23 | 34 / 40 | 51 / 61 | < 30 |

Opening and closing a dialog takes 1–2 ms of our JavaScript. The rest is repainting the window under the dimmed backdrop, and grows with the events drawn behind it, as when opening the event editor. Activating a group costs about as much as toggling one calendar: a single visibility update and a single debounced save, with no reload from the database. Like the toggle, it is paint-bound in SVAR's re-layout. It meets the target up to 100 events and misses it from 1,000 on this VM; real hardware with GPU rendering should be checked.

## Results: Milestone 5, accessible grid events, 2026-10-31

Raw data: [`benchmarks/20261031-vm-xvfb-m5-a11y.json`](benchmarks/20261031-vm-xvfb-m5-a11y.json). Same VM and Xvfb setup.

Events in the grid are now drawn by our own `EventContent` component, a labelled, focusable button, instead of SVAR's default content. The first version used `toLocaleDateString`/`toLocaleTimeString` per event and render, and these build a new formatter on every call. That made the grid slower. Creating the formatters once and caching each event's label more than undid it:

| 1,000 events (p50 ms) | Before | `toLocale…` per render | Cached formatters |
|---|---:|---:|---:|
| visibility toggle | 26 | 64 | 30 |
| week next | 43 | 57 | 36 |
| month next | 52 | 61 | 45 |
| drag proxy | 45.6 fps | 25.4 fps | 115 fps |
| drag proxy, 5,000 events | 12.8 fps | 5.1 fps | 72 fps |

The drag improvement over "before" suggests SVAR's default event content was itself formatting times expensively on every frame.

## Results: Milestone 5, agenda and search, 2026-10-31

Same VM and Xvfb setup (software rendering), so these remain a pessimistic baseline.

Raw data: [`benchmarks/20261031-171422-vm-xvfb-m5.json`](benchmarks/20261031-171422-vm-xvfb-m5.json). Storage numbers come from `bench_queries`.

Search now covers the whole database through `search_events` instead of filtering the loaded range. The benchmark runs four queries (`stand`, `review`, `zzz-no-match`, `gym`) the way the UI does: two years ahead (first 200 matches) and two years back (latest 50).

### Storage: search (release build, p50 / p95 ms)

| Dataset | First version (rows filtered in Rust) | With SQL `LIKE` prefilter |
|---|---:|---:|
| 100 | 2.3–3.1 | 0.4–1.9 |
| 1,000 | 22–24 | 1.2–3.0 |
| 5,000 | 84–132 | 4.9–10.7 |

The first version read every row of the four-year window before matching. Now SQLite discards non-matching rows first, using `LIKE` on ASCII words. Rust then applies the exact Unicode-aware match.

### UI: search, p50 / p95 ms

| Step | 100 events | 1,000 events | 5,000 events | Target |
|---|---|---|---|---|
| query → results painted | 17 / 139 | 45 / 88 | 72 / 82 | < 100: ✔ at p50; p95 ✘ only for the first (cold) search |
| of which the queries (IPC + SQLite) | 14 / 21 | 19 / 22 | 21 / 30 | |
| close search (calendar repainted) | 19 / 24 | 31 / 34 | 46 / 46 | |

Two changes brought the 5,000-event p50 down from 258 ms. The *query* time is the SQL prefilter above. Most of the rest was paint:
- **Results are drawn 50 upcoming / 20 earlier at a time**, with *Show more*. Drawing 250 rows cost about 100 ms under software rendering.
- **The calendar under the results is `visibility: hidden`.** It stays mounted (no SVAR rebuild), but WebKit no longer paints its event boxes under the opaque overlay. Before this, both opening and closing search grew with the number of events in the month.

The old in-view filter measured 15 / 51 ms at 1,000 events. The new search does more (every stored event, every recurring occurrence) and stays within the target from 1,000 events up.

**A one-off 1.5 s stall** appears in month navigation (p95) at 5,000 events in most of these runs. Probing per step showed one navigation stalling while our code took 3 ms of it. The stall is time-triggered, not caused by search: replacing the 1,000-event search phase with a plain 600 ms wait reproduces it, and without the wait it falls into an unmeasured phase (as in the pre-search baseline run). The likely suspects are a WebKit GC or memory timer under the VM. It needs checking on real hardware.

The agenda view isn't benchmarked separately yet. It reads the same cached range as month view, and draws at most one month of rows.

## Results: Milestone 2 (SQLite + Rust core), 2026-10-29

Same VM and Xvfb setup as below, so treat these as a pessimistic baseline too.

Raw data:
- UI benchmark: [`benchmarks/20261029-162950-vm-xvfb-m2.json`](benchmarks/20261029-162950-vm-xvfb-m2.json)
- Storage benchmark: `cargo run --release -p calendar-storage --example bench_queries`

The data now comes from SQLite through Tauri IPC, with recurring series expanded by the Rust core. The 1,000-event dataset holds 7 series masters (four of them unbounded), one moved occurrence and two cancelled occurrences.

### Storage: on-disk SQLite (WAL), release build

12 ranges spread over the year around the anchor. Times include decoding and recurrence expansion.

| Dataset | Load dataset (one transaction) | Week query p50 / p95 | 6-week month query p50 / p95 | 3-month prefetch query p50 / p95 |
|---|---:|---:|---:|---:|
| 100 | 10 ms | 0.8 / 1.0 ms | 1.1 / 1.6 ms | 1.8 / 2.1 ms |
| 1,000 | 63 ms | 1.5 / 2.0 ms | 3.1 / 5.5 ms | 6.1 / 10.4 ms |
| 5,000 | 336 ms | 4.0 / 4.6 ms | 10.9 / 18.0 ms | 20.5 / 31.3 ms |

From the UI, a full `query_range` round trip for the visible range (IPC + SQLite + expansion + JSON) took **1.5 ms p50 at 1,000 events and 2.7 ms at 5,000**. These queries run asynchronously and never block input.

### UI interactions: 1,000 events, M2 vs M1 (p50 / p95 ms)

| Interaction | M1 (in-memory mock) | M2 (SQLite) | Target |
|---|---|---|---|
| month next | 14 / 51 | 41 / 80 | < 50 — p50 ✔, p95 ✘ |
| month previous | 27 / 54 | 40 / 64 | < 50 — p50 ✔, p95 ✘ |
| switch to week | 50 / 57 | 48 / 55 | |
| week next | 28 / 79 | 36 / 73 | |
| visibility toggle | 30 / 39 | 28 / 35 | < 30 — p50 ✔, p95 ✘ |
| open editor | 31 / 35 | 32 / 33 | < 50 ✔ |
| create event (optimistic) | 28 / 40 | 16 / 32 | immediate ✔ |
| search | 14 / 47 | 15 / 51 | < 100 ✔ |
| drag proxy | 57 fps | 55 fps | 60 fps — close |
| idle CPU | 0.07 % | 0.07 % | ≈ 0 ✔ |

At 5,000 events, week view is still paint-bound: 146 ms to switch to week and 14 fps for the drag proxy. This matches Milestone 1, so it is not caused by storage.

### Interpretation

- **Storage is not the bottleneck.** Even the 3-month prefetch at 5,000 events (20 ms) runs off the UI thread. The visible-range round trip is under 3 ms.
- **Navigation p50 rose by 10–25 ms at 1,000 events.** Data now arrives after an IPC round trip, so the adapter re-feeds SVAR during the measured frames. The mock used to resolve within the same microtask. Navigation never waits for data: SVAR renders immediately from the prefetched window.
- **Known costs to optimise later, if measurements on real hardware call for it:**
  - every query re-expands each series from its DTSTART, so long-running daily series cost more over time (a per-series expansion cache is the planned fix);
  - SVAR receives the whole prefetch window again after each load.

## Results: Milestone 1 (in-memory mock), 2026-10-29, VM baseline

Raw data: [`benchmarks/20261029-153738-vm-xvfb.json`](benchmarks/20261029-153738-vm-xvfb.json)

| | |
|---|---|
| Machine | QEMU virtual CPU, 8 vCPU, 7.8 GiB RAM, Ubuntu 24.04.1, kernel 6.8 |
| Web engine | WebKitGTK 2.52.6, Tauri 2.12.5 |
| Display | **Xvfb with software rendering (llvmpipe, no GPU, no DRI3)**, 1280×820 window, DPR 1 |

Treat these numbers as a **pessimistic baseline**. They don't predict a KDE Plasma desktop with a GPU. Re-run `scripts/bench.py --label <machine>` there.

### 1,000 events (acceptance level)

| Interaction | n | p50 ms | p95 ms | JS p50 | JS+layout p50 | Target |
|---|---:|---:|---:|---:|---:|---|
| month next | 12 | 14 | 51 | 3 | 5 | < 50 — p50 ✔, p95 ✘ by 1 ms |
| month previous | 12 | 27 | 54 | 5 | 13 | < 50 — p50 ✔, p95 ✘ |
| 10 rapid month steps (burst) | 1 | 37 | — | 13 | 15 | |
| today | 1 | 38 | — | 2 | 4 | |
| switch to week | 5 | 50 | 57 | 4 | 6 | |
| switch to day | 5 | 21 | 24 | 4 | 5 | |
| switch to month | 5 | 60 | 64 | 3 | 4 | |
| week next / previous | 10 | 28 / 33 | 79 / 59 | 6 / 7 | 9 / 10 | |
| visibility toggle | 30 | 30 | 39 | 5 | 11 | < 30 — borderline ✘ |
| open editor | 10 | 31 | 35 | 2 | 5 | < 50 ✔ |
| close editor | 10 | 19 | 21 | 1 | 1 | |
| create event (optimistic) | 5 | 28 | 40 | 4 | 8 | immediate ✔ |
| rename event | 5 | 9 | 16 | 3 | 4 | |
| search (filter applied) | 5 | 14 | 47 | 3 | 6 | < 100 ✔ (debounce of 150 ms not included) |
| range query (mock source) | 20×100 | 0.00 | 0.01 | | | |
| drag proxy | | 57.3 fps, p95 frame 21 ms, 1 / 115 frames > 25 ms | | | | 60 fps — close |

### 100 and 5,000 events

| Interaction (p50 / p95 ms) | 100 events | 5,000 events |
|---|---|---|
| month next | 20 / 34 | 27 / 103 |
| month previous | 17 / 23 | 43 / 135 |
| switch to week | 32 / 61 | **159 / 170** |
| switch to day | 14 / 16 | 60 / 62 |
| switch to month | 30 / 41 | 89 / 113 |
| week next | 10 / 14 | **105 / 178** |
| visibility toggle | 5 / 10 | 52 / 63 |
| open editor | 21 / 59 | 45 / 54 |
| search | 9 / 16 | 17 / 87 |
| create event | 5 / 10 | 16 / 21 |
| drag proxy | 99 fps | **9.5 fps** |
| dataset load (generate + first render) | 31 | 100 |
| DOM elements in month / week / day | 270 / 245 / 142 | 724 / 684 / 224 |

### Idle and memory

| | Measured |
|---|---|
| Idle CPU, 30 s after an 8 s settle | **0.07 % of one core** (0.02 CPU-seconds), app + 2 WebKit processes |
| Idle RSS, 1,000 events | 361 MiB total |
| RSS after the 100 / 1,000 / 5,000 benchmark runs | 429 / 457 / 507 MiB |

The app process itself stays at about 136 MiB; the rest is the WebKit web and network processes. Software rendering inflates the web-process figure.

## Interpretation

- **The application code isn't the bottleneck.** Even at 5,000 events, our JS and Svelte work is ≤ 28 ms p50 in the worst case (week navigation, which re-feeds SVAR a new window) and usually 2–6 ms. Forced layout adds ≤ 12 ms. The mock range query costs microseconds.
- **Paint dominates, and it scales with how many events are on screen.** The remaining 30–130 ms happens after layout. Two experiments narrowed this down:
  1. `WEBKIT_DISABLE_COMPOSITING_MODE=1` made no difference.
  2. Feeding SVAR only the visible range, instead of the ±1-month window, made no difference. The cost therefore doesn't depend on the size of SVAR's internal store.

  Even interactions that change no calendar data, such as closing the editor, cost 9 / 19 / 34 ms at 100 / 1,000 / 5,000 events. This fits a full-window software repaint, with no GPU layer caching, whose cost grows with the number of event boxes and text runs drawn.
- **At 1,000 events the UI stays responsive** on this pessimistic setup. Most interactions meet their targets at p50; month navigation p95 and visibility toggle sit right at the limit.
- **5,000 events in week view exceed the targets on this VM.** On the generator's busy days a single day column holds 20+ events, many of them overlapping.

## Known bottlenecks and next steps

1. **Re-measure on real hardware** (KDE Plasma, GPU, HiDPI) before optimising. The paint-bound result may largely be an artefact of software rendering.
2. If week-view paint remains expensive on real hardware, try in order:
   - lighter custom `eventContent` markup;
   - CSS `contain: layout paint` on event boxes and day columns;
   - capping visible overlap columns with a "+N" overflow;
   - feeding SVAR only the visible range plus a small margin, with the prefetch window held in our store only.
3. Add a true pointer-driven drag benchmark, using XTest under Xvfb, alongside the proxy.
4. In Milestone 2, add query benchmarks against SQLite and measure Tauri IPC serialisation cost for typical ranges.
