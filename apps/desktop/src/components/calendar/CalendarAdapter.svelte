<!--
  The only component that knows about SVAR Svelte Calendar.

  Data flow: the EventStore is the source of truth. This adapter feeds SVAR
  the visible range plus a one-month prefetch window, applies incremental
  store changes one event at a time, and intercepts SVAR's own mutations
  (drag, resize, drag-create, click) so they go through the store instead of
  mutating SVAR's internal copy. SVAR's `events` prop is passed once and never
  changed, because changing it rebuilds SVAR's whole store.
-->
<script lang="ts">
  import { onDestroy, untrack } from "svelte";
  import { Calendar, Willow, WillowDark, type CalendarInstanceApi } from "@svar-ui/svelte-calendar";
  import type { EventStore, StoreChange } from "../../stores/events.svelte";
  import type { CalendarsStore } from "../../stores/calendars.svelte";
  import type { CalendarViewKind, DateRange } from "../../types/calendar";
  import type { Theme } from "../../services/theme";
  import { overlaps, eventBounds } from "../../services/time";
  import {
    calendarColorCss,
    eventClass,
    isViewKind,
    prefetchWindow,
    shiftTiming,
    timingFromWidget,
    toSvarEvent,
    type SvarEvent,
  } from "./svarMapping";
  import {
    DAY_HOURS,
    type CalendarViewController,
    type CalendarViewState,
    type DayHours,
    type EditorRequest,
    type MoveRequest,
  } from "./types";

  interface Props {
    store: EventStore;
    calendars: CalendarsStore;
    theme: Theme;
    initialView: CalendarViewKind;
    initialDate: Date;
    /** Week/day time grid: working hours only, or the whole day. */
    hours: DayHours;
    onready: (controller: CalendarViewController) => void;
    onstatechange: (state: CalendarViewState) => void;
    oneditor: (request: EditorRequest) => void;
    onmove: (request: MoveRequest) => void;
    onreject: (message: string) => void;
  }

  let {
    store,
    calendars,
    theme,
    initialView,
    initialDate,
    hours,
    onready,
    onstatechange,
    oneditor,
    onmove,
    onreject,
  }: Props = $props();

  const noEvents: SvarEvent[] = [];

  /** Hour shown at the top when week/day view opens in full-day mode. */
  const MORNING_HOUR = 7;
  const viewsFor = (h: DayHours) => {
    const sections = { timeGrid: { yScale: { ...DAY_HOURS[h], ui: { minUnitHeight: 60 } } } };
    return [{ id: "day", sections }, { id: "week", sections }, "month"];
  };
  // SVAR reads its view configuration only when created, so a change of
  // hours re-creates it (keyed below) at the current view and date.
  const views = $derived(viewsFor(hours));
  // Where the current SVAR instance is; a re-created instance opens here.
  // Deliberately not reactive: it is only read when SVAR is (re)created.
  const last: { view: CalendarViewKind; date: Date } = untrack(() => ({ view: initialView, date: initialDate }));
  /** Bumped per SVAR instance so filters are re-applied to a new one. */
  let instance = $state(0);
  let root: HTMLDivElement;

  /** Scrolls the time grid so MORNING_HOUR is at the top (full-day mode). */
  function scrollToMorning(): void {
    if (untrack(() => hours) !== "full") return;
    const scroller = root?.querySelector<HTMLElement>(".wx-sections");
    const sections = scroller?.querySelectorAll<HTMLElement>(".wx-section");
    const grid = sections?.[sections.length - 1];
    if (!scroller || !grid) return;
    const header = scroller.querySelector<HTMLElement>(".wx-x-headers-row")?.offsetHeight ?? 0;
    const gridTop = grid.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop;
    scroller.scrollTop = Math.max(0, gridTop + (grid.offsetHeight * MORNING_HOUR) / 24 - header);
  }

  /** Marks actions issued by this adapter so the intercepts let them through. */
  const INTERNAL = Symbol("uc-internal");
  type Payload = Record<string | symbol, unknown>;

  let api: CalendarInstanceApi | null = null;
  let visibleRange: DateRange | null = null;
  let providedWindow: DateRange | null = null;
  let wantedWindow: DateRange | null = null;
  /** Subscriptions to the current SVAR instance's state. */
  let unsubscribers: (() => void)[] = [];
  const unsubscribeStore = untrack(() => store).subscribe((change) => applyChange(change));

  const colorCss = $derived(calendarColorCss(calendars.calendars));

  function exec(name: string, payload: Payload): void {
    payload[INTERNAL] = true;
    // SVAR's exec is typed per action; payloads here follow its StoreActions.
    void api?.exec(name as never, payload as never);
  }

  function svarEvent(id: string) {
    return api?.getEvent(id);
  }

  function mapEvent(event: Parameters<typeof toSvarEvent>[0]): SvarEvent {
    return toSvarEvent(event, calendars.isReadOnly(event.calendarId));
  }

  function provide(window: DateRange): void {
    providedWindow = window;
    exec("provide-data", {
      data: { events: store.eventsIn(window).map(mapEvent) },
      reset: true,
    });
  }

  async function syncRange(range: DateRange): Promise<void> {
    visibleRange = range;
    const window = prefetchWindow(range);
    wantedWindow = window;
    const current = () => wantedWindow === window;
    if (store.covers(window)) {
      if (!sameRange(window, providedWindow)) provide(window);
      return;
    }
    if (store.covers(range)) {
      provide(window);
    } else {
      if (!(await store.load(range, "visible")) || !current()) return;
      provide(window);
    }
    if ((await store.load(window, "prefetch")) && current()) provide(window);
  }

  function applyChange(change: StoreChange): void {
    if (!api) return;
    switch (change.type) {
      case "upsert": {
        const inWindow = providedWindow !== null && overlaps(eventBounds(change.event), providedWindow);
        const existing = svarEvent(change.event.id);
        if (inWindow) {
          const mapped = mapEvent(change.event);
          if (existing) exec("update-event", { id: mapped.id, event: { ...mapped } });
          else exec("add-event", { event: { ...mapped } });
        } else if (existing) {
          exec("delete-event", { id: change.event.id });
        }
        break;
      }
      case "remove":
        if (svarEvent(change.id)) exec("delete-event", { id: change.id });
        break;
      case "invalidated":
        // Keep showing current data until the reload replaces it.
        providedWindow = null;
        wantedWindow = null;
        if (visibleRange) void syncRange(visibleRange);
        break;
      case "reset":
        providedWindow = null;
        wantedWindow = null;
        exec("provide-data", { data: { events: [] }, reset: true });
        if (visibleRange) void syncRange(visibleRange);
        break;
      case "loaded":
        // Handled by syncRange, which knows whether the load is still wanted.
        break;
    }
  }

  function readOnlyEvent(id: unknown): boolean {
    const event = svarEvent(id as string) as SvarEvent | undefined;
    return event?.readOnly ?? false;
  }

  function handleMove(payload: Payload): boolean | void {
    if (payload[INTERNAL]) return;
    const id = String(payload.id);
    const changes = (payload.event ?? {}) as Partial<SvarEvent>;
    const domain = store.get(id);
    if (!domain) return false;
    if (readOnlyEvent(id)) {
      onreject("This event belongs to a read-only calendar.");
      return false;
    }
    const allDay = changes.allDay ?? domain.timing.kind === "all-day";
    let timing;
    if (changes.start && changes.end) {
      timing = timingFromWidget(domain.timing, changes.start, changes.end, allDay);
    } else if (changes.start) {
      timing = shiftTiming(domain.timing, changes.start, allDay);
    } else {
      return false;
    }
    onmove({ id, timing });
    // The store's upsert updates SVAR; block SVAR's own mutation.
    return false;
  }

  function init(instance: CalendarInstanceApi): void {
    api = instance;
    instance.intercept("select-event", (payload) => {
      const p = payload as Payload;
      if (p[INTERNAL]) return;
      if (p.id != null) oneditor({ mode: "edit", id: String(p.id) });
      return false;
    });
    instance.intercept("add-event", (payload) => {
      const p = payload as Payload;
      if (p[INTERNAL]) return;
      const ev = (p.event ?? {}) as Partial<SvarEvent>;
      if (ev.start instanceof Date && ev.end instanceof Date) {
        oneditor({ mode: "create", timing: timingFromWidget(null, ev.start, ev.end, ev.allDay ?? false) });
      }
      return false;
    });
    instance.intercept("move-event", (payload) => handleMove(payload as Payload));
    instance.intercept("update-event", (payload) => handleMove(payload as Payload));
    instance.intercept("delete-event", (payload) => ((payload as Payload)[INTERNAL] ? undefined : false));
    instance.on("request-data", (payload) => {
      const p = payload as { startDate: Date; endDate: Date };
      // Defer out of SVAR's reactive update; microtasks still run before paint.
      queueMicrotask(() => void syncRange({ start: p.startDate.getTime(), end: p.endDate.getTime() }));
    });
  }

  function mount(svar: CalendarInstanceApi): void {
    for (const unsubscribe of unsubscribers) unsubscribe();
    unsubscribers = [];
    providedWindow = null;
    wantedWindow = null;
    init(svar);
    const state = svar.getReactiveState();
    let view: CalendarViewKind = last.view;
    let label = "";
    let range: DateRange = { start: 0, end: 0 };
    const publish = () => onstatechange({ view, label, range });
    // SVAR's typings say `void`, but its stores return an unsubscriber.
    const watch = <T,>(s: { subscribe: (fn: (v: T) => void) => void }, fn: (v: T) => void) =>
      s.subscribe(fn) as unknown as () => void;
    unsubscribers.push(
      watch(state.currentView, (v) => {
        if (isViewKind(v)) view = last.view = v;
        publish();
        if (v === "week" || v === "day") {
          // After SVAR has rendered the new view.
          requestAnimationFrame(() => requestAnimationFrame(scrollToMorning));
        }
      }),
      watch(state.rangeLabel, (v) => {
        label = v;
        publish();
      }),
      watch(state.visibleDateRange, (v) => {
        range = { start: v.start.getTime(), end: v.end.getTime() };
        publish();
      }),
      watch(state.currentDate, (v) => {
        last.date = v;
      }),
    );
    if (last.view === "week" || last.view === "day") {
      requestAnimationFrame(() => requestAnimationFrame(scrollToMorning));
    }
    instance++;
    // The initial request-data fires before init(); load the first range now.
    void syncRange(range);
    onready({
      setView: (v) => exec("navigate-to", { view: v }),
      navigate: (direction) =>
        exec("navigate-time", { direction: direction === "today" ? "now" : direction }),
      goToDate: (date) => exec("navigate-to", { date }),
    });
  }

  // Visibility is a SVAR view filter: it only re-runs layout.
  $effect(() => {
    void instance;
    const hidden = calendars.hidden;
    exec("filter-events", {
      tag: "visibility",
      filter: hidden.size ? (e: SvarEvent) => !hidden.has(e.calendarId) : null,
    });
  });

  onDestroy(() => {
    for (const unsubscribe of unsubscribers) unsubscribe();
    unsubscribeStore();
    api = null;
  });

  function sameRange(a: DateRange | null, b: DateRange | null): boolean {
    return a !== null && b !== null && a.start === b.start && a.end === b.end;
  }

  const eventCss = (ctx: { event: Record<string, unknown> }) => eventClass(ctx.event as unknown as SvarEvent);
</script>

<!-- Load both theme stylesheets once; switching theme only swaps a class. -->
<div hidden>
  <Willow fonts={false}>{#snippet children()}{/snippet}</Willow>
  <WillowDark fonts={false}>{#snippet children()}{/snippet}</WillowDark>
</div>

<div bind:this={root} class="adapter wx-theme" class:wx-willow-theme={theme === "light"} class:wx-willow-dark-theme={theme === "dark"}>
  {@html `<style>${colorCss}</style>`}
  {#key hours}
    <Calendar
      events={noEvents}
      view={last.view}
      date={last.date}
      toolbar={null}
      {views}
      {eventCss}
      init={mount}
    />
  {/key}
</div>

<style>
  .adapter {
    height: 100%;
    min-height: 0;
    font-family: inherit;
  }
  .adapter :global(.uc-st-pending-local) {
    opacity: 0.75;
    background-image: repeating-linear-gradient(
      -45deg,
      transparent 0 6px,
      rgb(255 255 255 / 0.18) 6px 12px
    );
  }
  .adapter :global(.uc-st-pending-sync) {
    background-image: repeating-linear-gradient(
      -45deg,
      transparent 0 6px,
      rgb(255 255 255 / 0.14) 6px 12px
    );
  }
  .adapter :global(.uc-st-sync-failed),
  .adapter :global(.uc-st-conflict) {
    outline: 2px dashed var(--uc-danger);
    outline-offset: -2px;
  }
  .adapter :global(.uc-ro) {
    cursor: default;
  }
</style>
