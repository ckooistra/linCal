<!--
  Events grouped by day, as a list. Used by the agenda view and for search
  results. Each event is a button that opens it.
-->
<script lang="ts">
  import type { AgendaDay } from "../services/agenda";
  import { dayHeading } from "../services/agenda";
  import type { CalendarsStore } from "../stores/calendars.svelte";
  import type { CalendarEvent, IsoDate } from "../types/calendar";

  interface Props {
    days: AgendaDay[];
    calendars: CalendarsStore;
    today: IsoDate;
    onopen: (event: CalendarEvent) => void;
  }

  let { days, calendars, today, onopen }: Props = $props();

  const STATUS_LABELS: Partial<Record<CalendarEvent["syncStatus"], string>> = {
    "pending-local": "Saving…",
    "pending-sync": "Not synced yet",
    "sync-failed": "Sync failed",
    conflict: "Conflict",
  };

  /** Up/Down move between the events of the surrounding list (agenda or results). */
  function onkeydown(e: KeyboardEvent): void {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    const current = e.currentTarget as HTMLElement;
    const items = [...(current.closest("[data-agenda]") ?? document).querySelectorAll<HTMLElement>("button.item")];
    const next = items[items.indexOf(current) + (e.key === "ArrowDown" ? 1 : -1)];
    if (!next) return;
    e.preventDefault();
    next.focus();
    next.scrollIntoView({ block: "nearest" });
  }
</script>

{#each days as day (day.date)}
  <section class="day" class:today={day.date === today} data-date={day.date} aria-labelledby={`agenda-${day.date}`}>
    <h2 id={`agenda-${day.date}`}>
      {dayHeading(day.date)}{#if day.date === today}<span class="badge">Today</span>{/if}
    </h2>
    <ul>
      {#each day.items as item (item.event.id)}
        {@const calendar = calendars.get(item.event.calendarId)}
        {@const status = STATUS_LABELS[item.event.syncStatus]}
        <li>
          <button type="button" class="item" onclick={() => onopen(item.event)} {onkeydown}>
            <span class="when">{item.when}</span>
            <span class="swatch" style:background={calendar?.color ?? "var(--uc-border)"} aria-hidden="true"></span>
            <span class="text">
              <span class="title">{item.event.title || "(No title)"}</span>
              {#if item.event.location}<span class="meta">{item.event.location}</span>{/if}
            </span>
            <span class="meta right">
              {#if item.event.seriesId}<span title="Repeats" aria-label="Repeats">↻</span>{/if}
              {#if status}<span class="status">{status}</span>{/if}
              <span class="sr-only">, {calendar?.name ?? "Unknown calendar"}</span>
            </span>
          </button>
        </li>
      {/each}
    </ul>
  </section>
{/each}

<style>
  .day {
    padding: 0.25rem 0;
  }
  h2 {
    position: sticky;
    top: 0;
    margin: 0;
    padding: 0.4rem 1rem;
    font-size: 0.85rem;
    font-weight: 600;
    background: var(--uc-surface-alt);
    border-bottom: 1px solid var(--uc-border);
    z-index: 1;
  }
  .today h2 {
    color: var(--uc-accent);
  }
  .badge {
    margin-left: 0.5rem;
    padding: 0 0.4rem;
    border-radius: 999px;
    font-size: 0.7rem;
    background: var(--uc-accent);
    color: var(--uc-accent-text);
    vertical-align: middle;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .item {
    display: grid;
    grid-template-columns: 10.5rem 0.75rem 1fr auto;
    align-items: center;
    gap: 0.6rem;
    width: 100%;
    padding: 0.45rem 1rem;
    border: none;
    border-radius: 0;
    background: transparent;
    text-align: left;
    font: inherit;
    color: inherit;
  }
  .item:hover {
    background: var(--uc-surface-alt);
  }
  .when {
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
    color: var(--uc-text-muted);
    font-size: 0.85rem;
  }
  .swatch {
    width: 0.75rem;
    height: 0.75rem;
    border-radius: 50%;
  }
  .text {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .title,
  .meta {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta {
    font-size: 0.8rem;
    color: var(--uc-text-muted);
  }
  .right {
    display: flex;
    gap: 0.5rem;
  }
  .status {
    color: var(--uc-danger);
  }
  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
    white-space: nowrap;
  }
  @media (max-width: 720px) {
    .item {
      grid-template-columns: 7.5rem 0.75rem 1fr auto;
    }
  }
</style>
