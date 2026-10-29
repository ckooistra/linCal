<script lang="ts">
  import { onDestroy, onMount, tick, untrack } from "svelte";
  import CalendarAdapter from "./components/calendar/CalendarAdapter.svelte";
  import type {
    CalendarViewController,
    CalendarViewState,
    EditorRequest,
    MoveRequest,
  } from "./components/calendar/types";
  import EventEditor from "./components/EventEditor.svelte";
  import PerfPanel from "./components/PerfPanel.svelte";
  import Sidebar from "./components/Sidebar.svelte";
  import Toasts from "./components/Toasts.svelte";
  import Toolbar from "./components/Toolbar.svelte";
  import { runBenchmarks, type BenchHarness } from "./bench/runner";
  import type { Backend } from "./services/backend";
  import { getBenchConfig, submitBenchReport } from "./services/bridge";
  import { measureInteraction, nextPaint, observeLongTasks } from "./services/perf";
  import { resolveShortcut } from "./services/shortcuts";
  import { parseThemePreference, resolveTheme, watchSystemDark, type ThemePreference } from "./services/theme";
  import { HOUR, localTimeZone, parseIsoDate } from "./services/time";
  import { CalendarsStore } from "./stores/calendars.svelte";
  import { EventStore, type SeriesScope } from "./stores/events.svelte";
  import { Notifications } from "./stores/notifications.svelte";
  import type { CalendarEvent, CalendarViewKind, EventDraft, EventTiming } from "./types/calendar";

  const BENCH_ANCHOR = "2026-06-15";
  const SEARCH_DEBOUNCE_MS = 150;

  let { backend }: { backend: Backend } = $props();

  // --- data -----------------------------------------------------------------
  const notifications = new Notifications();
  const calendars = new CalendarsStore();
  const initialSettings = untrack(() => backend.settings);
  let datasetSize = $state(typeof initialSettings.datasetSize === "number" ? initialSettings.datasetSize : 1000);
  let latencyMs = $state(0);
  let startupError = $state(untrack(() => backend.startupError));
  const store = new EventStore(untrack(() => backend.source), {
    isReadOnly: (id) => calendars.isReadOnly(id),
    onError: (message) => notifications.push(message),
  });

  async function reloadCalendars(): Promise<void> {
    try {
      const { accounts, calendars: list } = await backend.loadCalendars();
      calendars.load(accounts, list);
    } catch (error) {
      startupError = error instanceof Error ? error.message : String(error);
    }
  }

  let datasetBusy = false;
  async function switchDataset(size: number, anchor?: string): Promise<void> {
    if (datasetBusy) return;
    datasetBusy = true;
    try {
      const source = await backend.loadDataset(size, anchor);
      datasetSize = size;
      void backend.saveSetting("datasetSize", size);
      await reloadCalendars();
      store.reset(source);
    } catch (error) {
      notifications.push(error instanceof Error ? error.message : "The dataset could not be loaded.");
    } finally {
      datasetBusy = false;
    }
  }

  function setCalendarColor(id: string, color: string): void {
    const previous = calendars.get(id)?.color;
    calendars.setColor(id, color);
    backend.setCalendarColor(id, color).catch((error: unknown) => {
      if (previous) calendars.setColor(id, previous);
      notifications.push(error instanceof Error ? error.message : "The colour could not be saved.");
    });
  }

  // Visibility is a per-device preference: persist it, debounced.
  let hiddenTimer: ReturnType<typeof setTimeout> | undefined;
  function toggleCalendar(id: string): void {
    calendars.toggle(id);
    clearTimeout(hiddenTimer);
    hiddenTimer = setTimeout(() => void backend.saveSetting("hiddenCalendars", [...calendars.hidden]), 300);
  }

  // --- view -----------------------------------------------------------------
  let controller: CalendarViewController | null = null;
  let viewState = $state<CalendarViewState>({ view: "month", label: "", range: { start: 0, end: 0 } });
  let searchInput = $state<HTMLInputElement>();
  let searchText = $state("");
  let searchQuery = $state("");
  let searchTimer: ReturnType<typeof setTimeout> | undefined;

  function navigate(direction: "previous" | "next" | "today") {
    void measureInteraction(`nav-${direction}`, () => controller?.navigate(direction));
  }

  function setView(view: CalendarViewKind) {
    if (view === viewState.view) return;
    void measureInteraction(`view-${view}`, () => controller?.setView(view));
  }

  function onSearchInput(text: string) {
    searchText = text;
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => {
      void measureInteraction("search", () => {
        searchQuery = text;
      });
    }, SEARCH_DEBOUNCE_MS);
  }

  // --- theme ----------------------------------------------------------------
  let themePreference = $state<ThemePreference>(parseThemePreference(initialSettings.theme));
  let systemDark = $state(false);
  const theme = $derived(resolveTheme(themePreference, systemDark));
  $effect(() => {
    document.documentElement.dataset.theme = theme;
  });

  function setThemePreference(preference: ThemePreference) {
    themePreference = preference;
    void backend.saveSetting("theme", preference);
  }

  // --- editor ---------------------------------------------------------------
  interface EditorState {
    key: number;
    event: CalendarEvent | null;
    timing: EventTiming;
  }
  let editor = $state.raw<EditorState | null>(null);
  let editorKey = 0;

  function defaultTiming(): EventTiming {
    // Next full hour today, or 09:00 on the first visible day if today is off-screen.
    const now = new Date();
    const visible = viewState.range;
    let start: Date;
    if (visible.end > visible.start && (now.getTime() < visible.start || now.getTime() >= visible.end)) {
      start = new Date(visible.start);
      if (viewState.view === "month") start.setDate(start.getDate() + 7);
      start.setHours(9, 0, 0, 0);
    } else {
      start = new Date(now);
      start.setHours(now.getHours() + 1, 0, 0, 0);
    }
    return { kind: "timed", start: start.getTime(), end: start.getTime() + HOUR, timeZone: localTimeZone() };
  }

  function openEditor(request: EditorRequest) {
    if (request.mode === "edit") {
      const event = store.get(request.id);
      if (!event) return;
      editor = { key: ++editorKey, event, timing: event.timing };
    } else {
      editor = { key: ++editorKey, event: null, timing: request.timing };
    }
  }

  function closeEditor() {
    editor = null;
  }

  function saveEditor(draft: EventDraft): string | null {
    if (!editor) return null;
    // The editor edits whole series; dragging moves single occurrences.
    const result = editor.event ? store.update(editor.event.id, draft, "series") : store.create(draft);
    if (!result.ok) return result.message;
    editor = null;
    return null;
  }

  function deleteFromEditor(scope: SeriesScope) {
    if (!editor?.event) return;
    const result = store.remove(editor.event.id, scope);
    if (!result.ok) notifications.push(result.message);
    editor = null;
  }

  function draftOf(event: CalendarEvent): EventDraft {
    const { calendarId, title, description, location, timing, rrule } = event;
    return { calendarId, title, description, location, timing, rrule };
  }

  function moveEvent({ id, timing }: MoveRequest) {
    const event = store.get(id);
    if (!event) return;
    const result = store.update(id, { ...draftOf(event), timing }, "occurrence");
    if (!result.ok) notifications.push(result.message);
  }

  const defaultCalendarId = $derived(
    calendars.writable.find((c) => !calendars.hidden.has(c.id))?.id ?? calendars.writable[0]?.id ?? null,
  );

  // --- keyboard -------------------------------------------------------------
  let perfOpen = $state(false);

  function onKeydown(e: KeyboardEvent) {
    if (editor) return; // The dialog handles its own keys (Esc, Tab).
    const action = resolveShortcut(e);
    if (!action) return;
    e.preventDefault();
    switch (action) {
      case "today":
        return navigate("today");
      case "previous":
        return navigate("previous");
      case "next":
        return navigate("next");
      case "new-event":
        return void measureInteraction("open-editor", () => openEditor({ mode: "create", timing: defaultTiming() }));
      case "view-month":
        return setView("month");
      case "view-week":
        return setView("week");
      case "view-day":
        return setView("day");
      case "search":
        searchInput?.focus();
        searchInput?.select();
        return;
      case "toggle-perf":
        perfOpen = !perfOpen;
        return;
    }
  }

  // --- lifecycle ------------------------------------------------------------
  let longTasks = $state(0);
  let longTasksSupported = $state(false);
  let benchStatus = $state<string | null>(null);
  let cachedCount = $state(0);
  const cleanups: (() => void)[] = [];

  onMount(() => {
    cleanups.push(watchSystemDark((dark) => (systemDark = dark)));
    window.addEventListener("keydown", onKeydown);
    cleanups.push(() => window.removeEventListener("keydown", onKeydown));
    const stopLongTasks = observeLongTasks(() => longTasks++);
    longTasksSupported = stopLongTasks !== null;
    if (stopLongTasks) cleanups.push(stopLongTasks);
    // Cheap counter for the perf panel; updated at most once per frame.
    let pending = false;
    cleanups.push(
      store.subscribe(() => {
        if (pending) return;
        pending = true;
        requestAnimationFrame(() => {
          pending = false;
          cachedCount = store.size;
        });
      }),
    );
    const hidden = initialSettings.hiddenCalendars;
    void reloadCalendars().then(() => {
      if (Array.isArray(hidden)) for (const id of hidden) if (typeof id === "string") calendars.setVisible(id, false);
    });
    void startBenchIfRequested();
  });

  onDestroy(() => {
    for (const cleanup of cleanups) cleanup();
    clearTimeout(searchTimer);
    clearTimeout(hiddenTimer);
    notifications.dispose();
  });

  // --- benchmark ------------------------------------------------------------
  async function untilVisibleLoaded(): Promise<void> {
    // Wait for the adapter's visible-range load to finish, then a paint.
    while (store.loading || !store.covers(viewState.range)) {
      await new Promise((resolve) => setTimeout(resolve, 5));
    }
    await nextPaint();
  }

  const harness: BenchHarness = {
    anchor: parseIsoDate(BENCH_ANCHOR),
    async loadDataset(size) {
      await switchDataset(size, BENCH_ANCHOR);
      await tick();
      await untilVisibleLoaded();
    },
    setView: (view) => controller?.setView(view),
    navigate: (direction) => controller?.navigate(direction),
    goToDate: (date) => controller?.goToDate(date),
    calendarIds: () => calendars.calendars.map((c) => c.id),
    toggleCalendar: (id) => calendars.toggle(id),
    visibleEditableIds: () =>
      store
        .eventsIn(viewState.range)
        .filter((e) => !calendars.isReadOnly(e.calendarId) && e.timing.kind === "timed")
        .map((e) => e.id),
    openEditor: (id) => openEditor({ mode: "edit", id }),
    closeEditor,
    setSearch: (query) => {
      searchText = query;
      searchQuery = query;
    },
    createEvent: () => {
      const id = defaultCalendarId;
      if (!id) return null;
      const result = store.create({
        calendarId: id,
        title: "Benchmark event",
        description: "",
        location: "",
        timing: defaultTiming(),
      });
      return result.ok ? result.id : null;
    },
    moveEvent: (id, minutes) => {
      const event = store.get(id);
      if (!event || event.timing.kind !== "timed") return;
      const delta = minutes * 60_000;
      moveEvent({
        id,
        timing: { ...event.timing, start: event.timing.start + delta, end: event.timing.end + delta },
      });
    },
    renameEvent: (id, title) => {
      const event = store.get(id);
      if (event) store.update(id, { ...draftOf(event), title });
    },
    timeQueries: async (range, count) => {
      // Full round trip: IPC + SQLite + recurrence expansion (desktop).
      const start = performance.now();
      for (let i = 0; i < count; i++) await backend.source.queryRange(range);
      return performance.now() - start;
    },
    visibleRange: () => viewState.range,
  };

  async function startBenchIfRequested(): Promise<void> {
    let config;
    try {
      config = await getBenchConfig();
    } catch {
      return;
    }
    if (!config.enabled) return;
    try {
      const report = await runBenchmarks(harness, config.sizes, (m) => (benchStatus = m));
      benchStatus = "Benchmark finished";
      await submitBenchReport(report);
    } catch (error) {
      benchStatus = `Benchmark failed: ${String(error)}`;
      await submitBenchReport({ error: String(error) }).catch(() => {});
    }
  }

  const statusLine = $derived(
    startupError ??
      store.loadError ??
      (benchStatus ||
        `${datasetSize.toLocaleString()} simulated events · ${calendars.calendars.length} calendars · ${
          backend.kind === "desktop" ? "saved locally" : "browser preview (not saved)"
        }`),
  );
</script>

<div class="app">
  <Sidebar
    {calendars}
    {datasetSize}
    {latencyMs}
    {themePreference}
    simulationControls={backend.mock !== null}
    ontogglecalendar={(id) => void measureInteraction("visibility-toggle", () => toggleCalendar(id))}
    oncolor={setCalendarColor}
    ondataset={(size) => void measureInteraction("dataset-load", () => switchDataset(size))}
    onlatency={(ms) => {
      latencyMs = ms;
      if (backend.mock) backend.mock.latencyMs = ms;
    }}
    ontheme={setThemePreference}
    onfailnext={() => {
      if (!backend.mock) return;
      backend.mock.failNextWrites++;
      notifications.push("The next save will fail (simulated).", "info");
    }}
  />
  <main>
    <Toolbar
      label={viewState.label}
      view={viewState.view}
      loading={store.loading}
      search={searchText}
      bind:searchInput
      onnavigate={navigate}
      onview={setView}
      onnew={() => void measureInteraction("open-editor", () => openEditor({ mode: "create", timing: defaultTiming() }))}
      onsearch={onSearchInput}
    />
    <div class="calendar">
      <CalendarAdapter
        {store}
        {calendars}
        {theme}
        initialView="month"
        initialDate={new Date()}
        search={searchQuery}
        onready={(c) => (controller = c)}
        onstatechange={(s) => (viewState = s)}
        oneditor={(request) => void measureInteraction("open-editor", () => openEditor(request))}
        onmove={moveEvent}
        onreject={(message) => notifications.push(message)}
      />
    </div>
    <footer class="status" role="status">{statusLine}</footer>
  </main>
</div>

{#if editor}
  {#key editor.key}
    <EventEditor
      event={editor.event}
      timing={editor.timing}
      calendars={calendars.calendars}
      {defaultCalendarId}
      readOnly={editor.event ? calendars.isReadOnly(editor.event.calendarId) : false}
      onsave={saveEditor}
      ondelete={deleteFromEditor}
      onclose={closeEditor}
    />
  {/key}
{/if}

{#if perfOpen}
  <PerfPanel
    eventCount={datasetSize}
    {cachedCount}
    {longTasksSupported}
    {longTasks}
    onclose={() => (perfOpen = false)}
  />
{/if}

<Toasts {notifications} />

<style>
  .app {
    display: grid;
    grid-template-columns: 15rem 1fr;
    height: 100vh;
  }
  main {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }
  .calendar {
    flex: 1;
    min-height: 0;
  }
  .status {
    padding: 0.25rem 0.75rem;
    font-size: 0.75rem;
    color: var(--uc-text-muted);
    border-top: 1px solid var(--uc-border);
  }
  @media (max-width: 720px) {
    .app {
      grid-template-columns: 1fr;
    }
    .app > :global(.sidebar) {
      display: none;
    }
  }
</style>
