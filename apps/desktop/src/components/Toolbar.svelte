<script lang="ts">
  import type { CalendarViewKind } from "../types/calendar";
  import type { DayHours } from "./calendar/types";

  interface Props {
    label: string;
    view: CalendarViewKind;
    loading: boolean;
    search: string;
    hours: DayHours;
    onhours: (hours: DayHours) => void;
    onnavigate: (direction: "previous" | "next" | "today") => void;
    onview: (view: CalendarViewKind) => void;
    onnew: () => void;
    onsearch: (query: string) => void;
    searchInput?: HTMLInputElement | undefined;
  }

  let {
    label,
    view,
    loading,
    search,
    hours,
    onhours,
    onnavigate,
    onview,
    onnew,
    onsearch,
    searchInput = $bindable(),
  }: Props = $props();

  const views: { id: CalendarViewKind; label: string; key: string }[] = [
    { id: "month", label: "Month", key: "M" },
    { id: "week", label: "Week", key: "W" },
    { id: "day", label: "Day", key: "D" },
  ];
</script>

<header class="toolbar">
  <div class="group">
    <button type="button" onclick={() => onnavigate("today")} title="Today (T)">Today</button>
    <button type="button" class="icon" onclick={() => onnavigate("previous")} aria-label="Previous" title="Previous (K)">‹</button>
    <button type="button" class="icon" onclick={() => onnavigate("next")} aria-label="Next" title="Next (J)">›</button>
    <h1 aria-live="polite">{label}</h1>
    {#if loading}<span class="loading" role="status">Loading…</span>{/if}
  </div>
  <div class="group">
    <input
      class="search"
      type="search"
      placeholder="Search (Ctrl+F)"
      aria-label="Search events"
      value={search}
      bind:this={searchInput}
      oninput={(e) => onsearch(e.currentTarget.value)}
      onkeydown={(e) => {
        if (e.key === "Escape") {
          onsearch("");
          e.currentTarget.value = "";
          e.currentTarget.blur();
        }
      }}
    />
    {#if view !== "month"}
      <button
        type="button"
        class="hours"
        aria-pressed={hours === "work"}
        title={hours === "work" ? "Showing working hours (08–18). Show the full day (H)" : "Showing the full day. Show working hours only (H)"}
        onclick={() => onhours(hours === "work" ? "full" : "work")}>{hours === "work" ? "Work hours" : "24 h"}</button
      >
    {/if}
    <div class="segmented" role="group" aria-label="View">
      {#each views as v (v.id)}
        <button
          type="button"
          aria-pressed={view === v.id}
          title={`${v.label} (${v.key})`}
          onclick={() => onview(v.id)}>{v.label}</button
        >
      {/each}
    </div>
    <button type="button" class="primary" onclick={onnew} title="New event (N)">New event</button>
  </div>
</header>

<style>
  .toolbar {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 0.75rem;
    padding: 0.5rem 0.75rem;
    border-bottom: 1px solid var(--uc-border);
    flex-wrap: wrap;
  }
  .group {
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }
  h1 {
    font-size: 1.1rem;
    font-weight: 600;
    margin: 0 0 0 0.5rem;
    white-space: nowrap;
  }
  .icon {
    width: 2rem;
    font-size: 1.1rem;
    line-height: 1;
  }
  .loading {
    font-size: 0.8rem;
    color: var(--uc-text-muted);
  }
  .search {
    width: 12rem;
  }
  .segmented {
    display: flex;
  }
  .hours {
    min-width: 6.5rem;
  }
  .segmented button {
    border-radius: 0;
  }
  .segmented button:first-child {
    border-radius: 6px 0 0 6px;
  }
  .segmented button:last-child {
    border-radius: 0 6px 6px 0;
  }
  .segmented button + button {
    margin-left: -1px;
  }
  .segmented button[aria-pressed="true"] {
    background: var(--uc-accent);
    border-color: var(--uc-accent);
    color: var(--uc-accent-text);
  }
</style>
