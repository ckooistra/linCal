<!--
  Content of one event in SVAR's grid, given to SVAR as `eventContent`.
  Shows what SVAR's default content shows (time and title), but as a
  focusable button with a full label, so events can be reached with Tab
  and opened with Enter or Space. Mouse handling (click, drag, resize)
  stays with SVAR.
-->
<script lang="ts">
  import { getContext } from "svelte";
  import { EVENT_CONTEXT, eventLabel, formatEventTime as time, type EventContext, type SvarEvent } from "./svarMapping";

  let { event, mode }: { event: SvarEvent; mode: string } = $props();

  const context = getContext<EventContext>(EVENT_CONTEXT);
  const sameDay = $derived(
    event.start.getFullYear() === event.end.getFullYear() &&
      event.start.getMonth() === event.end.getMonth() &&
      event.start.getDate() === event.end.getDate(),
  );

  function onkeydown(e: KeyboardEvent): void {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      e.stopPropagation();
      context.open(event.id);
    }
  }
</script>

<div
  class="uc-event"
  role="button"
  tabindex="0"
  aria-label={eventLabel(event, context.calendarName(event.calendarId))}
  {onkeydown}
>
  {#if mode === "boxes"}
    <div class="box-time">{time(event.start)} – {time(event.end)}</div>
    <div class="box-title">{event.text}</div>
  {:else if mode === "grid" && !event.allDay && sameDay}
    <span class="marker"></span>
    <span class="bar-time">{time(event.start)}</span>
    <span class="bar-title">{event.text}</span>
  {:else}
    <span class="bar-title">{event.text}</span>
  {/if}
</div>

<style>
  /* SVAR's own content styles are scoped to its components: mirrored here. */
  .uc-event {
    display: flex;
    align-items: center;
    min-width: 0;
    width: 100%;
    height: 100%;
    outline-offset: -2px;
  }
  .uc-event:has(.box-time) {
    display: block;
  }
  .box-time {
    font-weight: var(--wx-font-weight-md);
    font-size: 12px;
    margin-bottom: 2px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .box-title {
    overflow: hidden;
  }
  .marker {
    flex: none;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background-color: var(--wx-color-primary);
    margin-left: 4px;
  }
  .bar-time {
    flex: none;
    font-weight: var(--wx-font-weight-md);
    font-size: 12px;
    margin-left: 4px;
  }
  .bar-title {
    padding: 0 6px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    font-size: var(--wx-font-size-sm);
    line-height: 1.2;
  }
  .uc-event:focus-visible {
    outline: 2px solid var(--uc-focus);
  }
</style>
