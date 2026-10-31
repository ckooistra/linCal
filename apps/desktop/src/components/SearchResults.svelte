<!--
  Results of a search over every stored event: upcoming matches first, then
  earlier ones (most recent first). Re-runs when the query changes or the
  stored data does (e.g. after a sync).
-->
<script lang="ts">
  import { onDestroy, untrack } from "svelte";
  import AgendaList from "./AgendaList.svelte";
  import { agendaDays, type AgendaDay } from "../services/agenda";
  import { isAbortError, searchTerms } from "../services/eventSource";
  import { prefers12Hour, toIsoDate } from "../services/time";
  import type { CalendarsStore } from "../stores/calendars.svelte";
  import type { EventStore } from "../stores/events.svelte";
  import type { CalendarEvent, DateRange } from "../types/calendar";

  interface Props {
    query: string;
    store: EventStore;
    calendars: CalendarsStore;
    onopen: (event: CalendarEvent) => void;
    /** Called with the time the search took, for instrumentation. */
    onsearched?: (ms: number) => void;
  }

  let { query, store, calendars, onopen, onsearched }: Props = $props();

  /** How far results reach from today. */
  const YEARS_AHEAD = 2;
  const YEARS_BACK = 2;
  const UPCOMING_LIMIT = 200;
  const EARLIER_LIMIT = 50;
  /** Results drawn at first and per "Show more" (drawing is the slow part). */
  const PAGE = { upcoming: 50, earlier: 20 };
  const hour12 = prefers12Hour();

  interface Results {
    query: string;
    upcoming: CalendarEvent[];
    earlier: CalendarEvent[];
  }
  let results = $state.raw<Results | null>(null);
  let error = $state<string | null>(null);
  let controller: AbortController | null = null;
  let today = $state(toIsoDate(new Date()));
  let shown = $state({ ...PAGE });

  async function run(text: string): Promise<void> {
    controller?.abort();
    const terms = searchTerms(text);
    if (terms.length === 0) {
      results = null;
      return;
    }
    const current = (controller = new AbortController());
    const now = new Date();
    today = toIsoDate(now);
    const midnight = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
    const ahead = new Date(midnight);
    ahead.setFullYear(ahead.getFullYear() + YEARS_AHEAD);
    const back = new Date(midnight);
    back.setFullYear(back.getFullYear() - YEARS_BACK);
    const started = performance.now();
    try {
      const signal = current.signal;
      const [upcoming, earlier] = await Promise.all([
        store.search(text, { start: midnight, end: ahead.getTime() }, { limit: UPCOMING_LIMIT, signal }),
        store.search(text, { start: back.getTime(), end: midnight }, { limit: EARLIER_LIMIT, latest: true, signal }),
      ]);
      if (current.signal.aborted) return;
      // An event running over midnight appears in both: keep it upcoming.
      const upcomingIds = new Set(upcoming.map((e) => e.id));
      results = { query: text, upcoming, earlier: earlier.filter((e) => !upcomingIds.has(e.id)) };
      shown = { ...PAGE };
      error = null;
      onsearched?.(performance.now() - started);
    } catch (e) {
      if (isAbortError(e) || current.signal.aborted) return;
      error = e instanceof Error ? e.message : "Search failed.";
    }
  }

  $effect(() => {
    const text = query;
    untrack(() => void run(text));
  });

  const unsubscribe = untrack(() => store).subscribe((change) => {
    if (change.type === "invalidated" || change.type === "reset") void run(query);
  });

  onDestroy(() => {
    controller?.abort();
    unsubscribe();
  });

  function visible(events: CalendarEvent[]): CalendarEvent[] {
    const hidden = calendars.hidden;
    return events.filter((e) => !hidden.has(e.calendarId));
  }

  const everything: DateRange = { start: -8.64e15, end: 8.64e15 };
  const upcoming = $derived(results ? visible(results.upcoming) : []);
  // Ascending; the most recent are shown first.
  const earlier = $derived(results ? visible(results.earlier) : []);
  const upcomingDays = $derived<AgendaDay[]>(
    agendaDays(upcoming.slice(0, shown.upcoming), everything, { spread: false, hour12 }),
  );
  const earlierDays = $derived<AgendaDay[]>(
    agendaDays(earlier.slice(-shown.earlier), everything, { spread: false, hour12 }).reverse(),
  );
  const count = $derived(upcoming.length + earlier.length);
  const truncated = $derived(
    results !== null && (results.upcoming.length >= UPCOMING_LIMIT || results.earlier.length >= EARLIER_LIMIT),
  );
</script>

<div class="results" role="region" aria-label="Search results" data-agenda>
  {#if error}
    <p class="note error" role="alert">{error}</p>
  {:else if results}
    <p class="note" role="status">
      {#if count === 0}
        No events match “{results.query}”.
      {:else}
        {count}{truncated ? "+" : ""} {count === 1 ? "event matches" : "events match"} “{results.query}”. Select one to open it.
      {/if}
      {#if calendars.hidden.size > 0}Hidden calendars are not searched.{/if}
    </p>
    {#if upcomingDays.length > 0}
      <h2 class="group">Upcoming</h2>
      <AgendaList days={upcomingDays} {calendars} {today} {onopen} />
      {#if upcoming.length > shown.upcoming}
        <button type="button" class="more" onclick={() => (shown.upcoming += PAGE.upcoming)}>
          Show more upcoming ({upcoming.length - shown.upcoming})
        </button>
      {/if}
    {/if}
    {#if earlierDays.length > 0}
      <h2 class="group">Earlier</h2>
      <AgendaList days={earlierDays} {calendars} {today} {onopen} />
      {#if earlier.length > shown.earlier}
        <button type="button" class="more" onclick={() => (shown.earlier += PAGE.earlier)}>
          Show more earlier ({earlier.length - shown.earlier})
        </button>
      {/if}
    {/if}
  {/if}
</div>

<style>
  .results {
    height: 100%;
    overflow-y: auto;
  }
  .note {
    margin: 0;
    padding: 0.6rem 1rem;
    color: var(--uc-text-muted);
    font-size: 0.85rem;
  }
  .more {
    margin: 0.5rem 1rem;
  }
  .error {
    color: var(--uc-danger);
  }
  .group {
    margin: 0;
    padding: 0.75rem 1rem 0.25rem;
    font-size: 0.75rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--uc-text-muted);
  }
</style>
