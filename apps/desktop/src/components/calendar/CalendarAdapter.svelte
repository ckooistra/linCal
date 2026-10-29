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
  import type { CalendarViewController, CalendarViewState, EditorRequest, MoveRequest } from "./types";

  interface Props {
    store: EventStore;
    calendars: CalendarsStore;
    theme: Theme;
    initialView: CalendarViewKind;
    initialDate: Date;
    /** Case-insensitive title filter; empty shows everything. */
    search: string;
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
    search,
    onready,
    onstatechange,
    oneditor,
    onmove,
    onreject,
  }: Props = $props();

  // Snapshot initial props: SVAR re-initialises if these ever change.
  const startView = untrack(() => initialView);
  const startDate = untrack(() => initialDate);
  const noEvents: SvarEvent[] = [];

  /** Marks actions issued by this adapter so the intercepts let them through. */
  const INTERNAL = Symbol("uc-internal");
  type Payload = Record<string | symbol, unknown>;

  let api: CalendarInstanceApi | null = null;
  let visibleRange: DateRange | null = null;
  let providedWindow: DateRange | null = null;
  let wantedWindow: DateRange | null = null;
  const unsubscribers: (() => void)[] = [];

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

  function mount(instance: CalendarInstanceApi): void {
    init(instance);
    const state = instance.getReactiveState();
    let view: CalendarViewKind = startView;
    let label = "";
    let range: DateRange = { start: 0, end: 0 };
    const publish = () => onstatechange({ view, label, range });
    // SVAR's typings say `void`, but its stores return an unsubscriber.
    const watch = <T,>(s: { subscribe: (fn: (v: T) => void) => void }, fn: (v: T) => void) =>
      s.subscribe(fn) as unknown as () => void;
    unsubscribers.push(
      watch(state.currentView, (v) => {
        if (isViewKind(v)) view = v;
        publish();
      }),
      watch(state.rangeLabel, (v) => {
        label = v;
        publish();
      }),
      watch(state.visibleDateRange, (v) => {
        range = { start: v.start.getTime(), end: v.end.getTime() };
        publish();
      }),
      store.subscribe(applyChange),
    );
    // The initial request-data fires before init(); load the first range now.
    void syncRange(range);
    onready({
      setView: (v) => exec("navigate-to", { view: v }),
      navigate: (direction) =>
        exec("navigate-time", { direction: direction === "today" ? "now" : direction }),
      goToDate: (date) => exec("navigate-to", { date }),
    });
  }

  // Visibility and search are SVAR view filters: they only re-run layout.
  $effect(() => {
    const hidden = calendars.hidden;
    exec("filter-events", {
      tag: "visibility",
      filter: hidden.size ? (e: SvarEvent) => !hidden.has(e.calendarId) : null,
    });
  });

  $effect(() => {
    const query = search.trim().toLowerCase();
    exec("filter-events", {
      tag: "search",
      filter: query ? (e: SvarEvent) => e.text.toLowerCase().includes(query) : null,
    });
  });

  onDestroy(() => {
    for (const unsubscribe of unsubscribers) unsubscribe();
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

<div class="adapter wx-theme" class:wx-willow-theme={theme === "light"} class:wx-willow-dark-theme={theme === "dark"}>
  {@html `<style>${colorCss}</style>`}
  <Calendar
    events={noEvents}
    view={startView}
    date={startDate}
    toolbar={null}
    {eventCss}
    init={mount}
  />
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
  .adapter :global(.uc-ro) {
    cursor: default;
  }
</style>
