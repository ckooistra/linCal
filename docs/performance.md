# Performance

Responsiveness is a first-class requirement. This page records targets, method, the **measured** results for Milestone 1, and what they do and don't show.

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

## Results: 2026-10-29, VM baseline

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
