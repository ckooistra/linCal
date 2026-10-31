/**
 * Scripted UI benchmark. Drives the real app through its public controls
 * and records interaction-to-paint latency for each dataset size.
 * Started with `UC_BENCH=1` (Tauri) or `?bench` (browser); see docs/performance.md.
 */
import type { CalendarViewKind, DateRange } from "../types/calendar";
import {
  measureInteraction,
  nextPaint,
  observeLongTasks,
  PerfLog,
  sampleFrames,
  summarize,
  type FrameStats,
  type Stats,
} from "../services/perf";
import { sampleMemory, type MemorySample } from "../services/bridge";

export interface BenchHarness {
  /** Replace the dataset and wait until the visible range is rendered. */
  loadDataset(size: number): Promise<void>;
  setView(view: CalendarViewKind): void;
  navigate(direction: "previous" | "next" | "today"): void;
  goToDate(date: Date): void;
  calendarIds(): string[];
  toggleCalendar(id: string): void;
  /** Editable events in the visible range, for editor and drag scenarios. */
  visibleEditableIds(): string[];
  openEditor(id: string): void;
  closeEditor(): void;
  /** Settings and the command palette open and close (UI only). */
  setSettingsOpen(open: boolean): void;
  setPaletteOpen(open: boolean): void;
  /** Shows the events of exactly these calendars, as a calendar group does. */
  showOnly(calendarIds: string[]): void;
  /** Resolves once the results are set, with the search's own duration (IPC + SQLite) in ms. */
  setSearch(query: string): void | Promise<number>;
  createEvent(): string | null;
  /** Move an event by `minutes` through the same path as a completed drag. */
  moveEvent(id: string, minutes: number): void;
  renameEvent(id: string, title: string): void;
  /** Time `count` consecutive data-source range queries; total ms. */
  timeQueries(range: DateRange, count: number): Promise<number>;
  visibleRange(): DateRange;
  anchor: Date;
}

export interface ScenarioResult {
  size: number;
  interactions: Stats[];
  dragProxy: FrameStats;
  idleFrames: FrameStats;
  queryMs: Stats;
  /** DOM element count per view at the anchor date (rendering cost driver). */
  domNodes: Record<CalendarViewKind, number>;
  longTasks: number | null;
  memoryAfter: MemorySample | null;
}

export interface BenchReport {
  startedAt: string;
  userAgent: string;
  devicePixelRatio: number;
  viewport: { width: number; height: number };
  hardwareConcurrency: number;
  results: ScenarioResult[];
}

const QUERY_BATCH = 10;

async function repeat(log: PerfLog, name: string, times: number, action: () => void | Promise<unknown>) {
  for (let i = 0; i < times; i++) await measureInteraction(name, action, log);
}

export async function runBenchmarks(
  h: BenchHarness,
  sizes: readonly number[],
  progress: (message: string) => void,
): Promise<BenchReport> {
  const report: BenchReport = {
    startedAt: new Date().toISOString(),
    userAgent: navigator.userAgent,
    devicePixelRatio: window.devicePixelRatio,
    viewport: { width: window.innerWidth, height: window.innerHeight },
    hardwareConcurrency: navigator.hardwareConcurrency,
    results: [],
  };

  for (const size of sizes) {
    progress(`Benchmark: ${size} events`);
    const log = new PerfLog();
    let longTasks = 0;
    const stopLongTasks = observeLongTasks(() => longTasks++);

    await measureInteraction("dataset-load", () => h.loadDataset(size), log);
    h.setView("month");
    h.goToDate(h.anchor);
    await nextPaint();

    // Month navigation: forward then back over the cached + prefetched range.
    await repeat(log, "month-next", 12, () => h.navigate("next"));
    await repeat(log, "month-prev", 12, () => h.navigate("previous"));
    // Rapid navigation: no waiting between steps, measured as one burst.
    await measureInteraction("month-rapid-10", () => {
      for (let i = 0; i < 10; i++) h.navigate("next");
    }, log);
    await measureInteraction("today", () => h.navigate("today"), log);
    h.goToDate(h.anchor);
    await nextPaint();

    for (let i = 0; i < 5; i++) {
      await measureInteraction("view-week", () => h.setView("week"), log);
      await measureInteraction("view-day", () => h.setView("day"), log);
      await measureInteraction("view-month", () => h.setView("month"), log);
    }

    h.setView("week");
    await nextPaint();
    await repeat(log, "week-next", 10, () => h.navigate("next"));
    await repeat(log, "week-prev", 10, () => h.navigate("previous"));

    h.setView("month");
    await nextPaint();
    const ids = h.calendarIds();
    // 15 hide/show pairs = 30 toggles; every calendar ends visible again.
    for (let i = 0; i < 15; i++) {
      const id = ids[i % ids.length]!;
      await measureInteraction("visibility-toggle", () => h.toggleCalendar(id), log);
      await measureInteraction("visibility-toggle", () => h.toggleCalendar(id), log);
    }

    // Group switching: half the calendars, then all again.
    const all = h.calendarIds();
    for (let i = 0; i < 10; i++) {
      await measureInteraction("activate-group", () => h.showOnly(all.slice(0, Math.ceil(all.length / 2))), log);
      await measureInteraction("activate-group", () => h.showOnly(all), log);
    }

    for (let i = 0; i < 10; i++) {
      await measureInteraction("open-settings", () => h.setSettingsOpen(true), log);
      await measureInteraction("close-settings", () => h.setSettingsOpen(false), log);
      await measureInteraction("open-palette", () => h.setPaletteOpen(true), log);
      await measureInteraction("close-palette", () => h.setPaletteOpen(false), log);
    }

    const editable = h.visibleEditableIds();
    for (let i = 0; i < 10 && editable.length; i++) {
      const id = editable[i % editable.length]!;
      await measureInteraction("open-editor", () => h.openEditor(id), log);
      await measureInteraction("close-editor", () => h.closeEditor(), log);
    }

    for (let i = 0; i < 5; i++) {
      await measureInteraction("create-event", () => void h.createEvent(), log);
    }
    for (let i = 0; i < 5 && editable.length; i++) {
      const id = editable[i]!;
      await measureInteraction("rename-event", () => h.renameEvent(id, `Renamed ${i}`), log);
    }

    // "search": query to painted results; "search-query": the data-source
    // part of it; "search-clear": closing the results.
    for (const query of ["stand", "review", "zzz-no-match", "gym"]) {
      let queryMs = 0;
      await measureInteraction(
        "search",
        async () => {
          queryMs = (await h.setSearch(query)) ?? 0;
        },
        log,
      );
      log.record("search-query", { script: queryMs, layout: queryMs, paint: queryMs });
      await measureInteraction("search-clear", () => h.setSearch(""), log);
    }

    // Drag proxy: move one event every frame through the completed-drag path.
    h.setView("week");
    h.goToDate(h.anchor);
    await nextPaint();
    const dragId = h.visibleEditableIds()[0];
    const dragProxy = await sampleFrames(2000, (frame) => {
      if (dragId) h.moveEvent(dragId, frame % 2 === 0 ? 15 : -15);
    });
    const idleFrames = await sampleFrames(1000);

    const domNodes = { month: 0, week: 0, day: 0 };
    for (const view of ["month", "week", "day"] as const) {
      h.setView(view);
      h.goToDate(h.anchor);
      await nextPaint();
      domNodes[view] = document.getElementsByTagName("*").length;
    }

    // Timers are clamped to 1 ms, so time batches of queries and divide.
    const window = h.visibleRange();
    const queries = [];
    for (let i = 0; i < 20; i++) {
      const total = await h.timeQueries(window, QUERY_BATCH);
      queries.push({ script: total / QUERY_BATCH, paint: total / QUERY_BATCH });
    }

    stopLongTasks?.();
    report.results.push({
      size,
      interactions: log.stats(),
      dragProxy,
      idleFrames,
      queryMs: summarize("query-visible-range", queries),
      domNodes,
      longTasks: stopLongTasks ? longTasks : null,
      memoryAfter: await sampleMemory(),
    });
  }
  return report;
}
