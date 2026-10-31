<!--
  Agenda: the events of one month as a list, by day. Loads its range through
  the EventStore like the calendar grid, and implements the same controller
  so the toolbar and shortcuts work unchanged.
-->
<script lang="ts">
  import { onDestroy, onMount, tick, untrack } from "svelte";
  import AgendaList from "./AgendaList.svelte";
  import { agendaDays, type AgendaDay } from "../services/agenda";
  import { monthStart, prefers12Hour, toIsoDate } from "../services/time";
  import type { CalendarsStore } from "../stores/calendars.svelte";
  import type { EventStore } from "../stores/events.svelte";
  import type { CalendarEvent, DateRange } from "../types/calendar";
  import type { CalendarViewController, CalendarViewState } from "./calendar/types";

  interface Props {
    store: EventStore;
    calendars: CalendarsStore;
    initialDate: Date;
    onready: (controller: CalendarViewController) => void;
    onstatechange: (state: CalendarViewState) => void;
    onopen: (event: CalendarEvent) => void;
  }

  let { store, calendars, initialDate, onready, onstatechange, onopen }: Props = $props();

  const hour12 = prefers12Hour();
  let month = $state(monthStart(untrack(() => initialDate)));
  const range = $derived<DateRange>({ start: month.getTime(), end: monthStart(month, 1).getTime() });
  let days = $state.raw<AgendaDay[]>([]);
  let loaded = $state(false);
  let today = $state(toIsoDate(new Date()));
  let list: HTMLDivElement;
  /** Scroll to today once the next load finishes (opening at, or jumping to, today). */
  let scrollPending = untrack(() => monthStart(initialDate).getTime() === monthStart(new Date()).getTime());

  function rebuild(): void {
    const hidden = calendars.hidden;
    const events = store.eventsIn(range).filter((e) => !hidden.has(e.calendarId));
    days = agendaDays(events, range, { spread: true, hour12 });
  }

  // Store changes arrive one event at a time; rebuild at most once per frame.
  let frame = 0;
  function rebuildSoon(): void {
    if (frame) return;
    frame = requestAnimationFrame(() => {
      frame = 0;
      rebuild();
    });
  }
  const unsubscribe = untrack(() => store).subscribe((change) => {
    if (change.type === "invalidated" || change.type === "reset") void load(range);
    else rebuildSoon();
  });

  async function load(wanted: DateRange): Promise<void> {
    if (!store.covers(wanted)) {
      loaded = false;
      const ok = await store.load(wanted, "visible");
      if (!ok || wanted !== range) return;
    }
    rebuild();
    loaded = true;
    if (scrollPending) {
      scrollPending = false;
      void scrollToToday();
    }
  }

  $effect(() => {
    const r = range;
    void calendars.hidden;
    untrack(() => {
      onstatechange({
        view: "agenda",
        label: month.toLocaleDateString(undefined, { month: "long", year: "numeric" }),
        range: r,
        // Today when it is in this month, else the month's first day.
        date: Date.now() >= r.start && Date.now() < r.end ? Date.now() : r.start,
      });
      void load(r);
    });
  });

  async function scrollToToday(): Promise<void> {
    await tick();
    if (!list) return;
    // Today, or the next day with events.
    const target = [...list.querySelectorAll<HTMLElement>("[data-date]")].find((d) => (d.dataset.date ?? "") >= today);
    list.scrollTop = target ? target.offsetTop : 0;
  }

  function goTo(date: Date, focusToday = false): void {
    today = toIsoDate(new Date());
    scrollPending = focusToday;
    const next = monthStart(date);
    if (next.getTime() !== month.getTime()) month = next; // the effect loads it
    else if (focusToday) void load(range);
    if (!focusToday) list.scrollTop = 0;
  }

  onMount(() => {
    onready({
      setView: () => {},
      navigate: (direction) => {
        if (direction === "today") goTo(new Date(), true);
        else goTo(monthStart(month, direction === "next" ? 1 : -1));
      },
      goToDate: (date) => goTo(date),
    });
  });

  onDestroy(() => {
    unsubscribe();
    cancelAnimationFrame(frame);
  });
</script>

<div class="agenda" bind:this={list} aria-busy={!loaded} data-agenda>
  {#if days.length > 0}
    <AgendaList {days} {calendars} {today} {onopen} />
  {:else if loaded}
    <p class="empty">No events this month.</p>
  {/if}
</div>

<style>
  .agenda {
    position: relative;
    height: 100%;
    overflow-y: auto;
  }
  .empty {
    padding: 2rem;
    text-align: center;
    color: var(--uc-text-muted);
  }
</style>
