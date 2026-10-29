<script lang="ts">
  import { onMount, untrack } from "svelte";
  import type { Calendar, CalendarEvent, EventDraft, EventTiming } from "../types/calendar";
  import type { SeriesScope } from "../stores/events.svelte";
  import { repeatOptions } from "../services/recurrence";
  import TimeField from "./TimeField.svelte";
  import {
    addIsoDays,
    combineDateTime,
    localTimeZone,
    parseIsoDate,
    toIsoDate,
    toTimeString,
    validateTiming,
  } from "../services/time";

  interface Props {
    /** Existing event, or null when creating. */
    event: CalendarEvent | null;
    /** Initial timing for a new event. */
    timing: EventTiming;
    calendars: readonly Calendar[];
    defaultCalendarId: string | null;
    readOnly: boolean;
    onsave: (draft: EventDraft) => string | null;
    /** For recurring events: delete just this occurrence or the whole series. */
    ondelete: (scope: SeriesScope) => void;
    onclose: () => void;
  }

  let { event, timing, calendars, defaultCalendarId, readOnly, onsave, ondelete, onclose }: Props = $props();

  // The form edits a local copy initialised once from the props; the parent
  // re-creates this component (keyed) for every editor session.
  const init = untrack(() => {
    const t = event?.timing ?? timing;
    if (t.kind === "all-day") {
      return {
        allDay: true,
        startDate: t.startDate,
        // Show the inclusive last day; the model stores an exclusive end.
        endDate: addIsoDays(t.endDate, -1),
        startTime: "09:00",
        endTime: "10:00",
        title: event?.title ?? "",
        calendarId: event?.calendarId ?? defaultCalendarId ?? calendars.find((c) => !c.readOnly)?.id ?? "",
        location: event?.location ?? "",
        description: event?.description ?? "",
        rrule: event?.rrule ?? null,
      };
    }
    const s = new Date(t.start);
    const e = new Date(t.end);
    return {
      allDay: false,
      startDate: toIsoDate(s),
      endDate: toIsoDate(e),
      startTime: toTimeString(s),
      endTime: toTimeString(e),
      title: event?.title ?? "",
      calendarId: event?.calendarId ?? defaultCalendarId ?? calendars.find((c) => !c.readOnly)?.id ?? "",
      location: event?.location ?? "",
      description: event?.description ?? "",
      rrule: event?.rrule ?? null,
    };
  });

  let title = $state(init.title);
  let calendarId = $state(init.calendarId);
  let location = $state(init.location);
  let description = $state(init.description);
  let rrule = $state<string | null>(init.rrule);
  const recurring = untrack(() => event?.seriesId != null);
  const repeatChoices = repeatOptions(init.rrule);
  let allDay = $state(init.allDay);
  let startDate = $state(init.startDate);
  let endDate = $state(init.endDate);
  let startTime = $state(init.startTime);
  let endTime = $state(init.endTime);
  let error = $state<string | null>(null);

  let dialog: HTMLDivElement;
  let titleInput: HTMLInputElement;

  /** Current start/end as instants (timed) or day numbers (all-day); null while invalid. */
  function span(): { start: number; end: number } | null {
    if (allDay) {
      try {
        return { start: parseIsoDate(startDate).getTime(), end: parseIsoDate(endDate).getTime() };
      } catch {
        return null;
      }
    }
    const start = combineDateTime(startDate, startTime);
    const end = combineDateTime(endDate, endTime);
    return start && end ? { start: start.getTime(), end: end.getTime() } : null;
  }

  /** Moving the start keeps the event's length, as calendar apps usually do. */
  function moveStart(apply: () => void) {
    const before = span();
    apply();
    if (!before || before.end < before.start) return;
    if (allDay) {
      const days = Math.round((before.end - before.start) / 86_400_000);
      try {
        endDate = addIsoDays(startDate, days);
      } catch {
        // Start date still being typed.
      }
      return;
    }
    const start = combineDateTime(startDate, startTime);
    if (!start) return;
    const end = new Date(start.getTime() + (before.end - before.start));
    endDate = toIsoDate(end);
    endTime = toTimeString(end);
  }

  const choices = $derived(readOnly ? calendars : calendars.filter((c) => !c.readOnly || c.id === calendarId));
  const heading = $derived(event ? (readOnly ? "Event details" : "Edit event") : "New event");

  function buildTiming(): EventTiming | string {
    if (allDay) {
      try {
        parseIsoDate(startDate);
        return { kind: "all-day", startDate, endDate: addIsoDays(endDate, 1) };
      } catch {
        return "Enter dates as YYYY-MM-DD.";
      }
    }
    const start = combineDateTime(startDate, startTime);
    const end = combineDateTime(endDate, endTime);
    if (!start || !end) return "Enter dates as YYYY-MM-DD and times as HH:MM.";
    const zone = event?.timing.kind === "timed" ? event.timing.timeZone : localTimeZone();
    return { kind: "timed", start: start.getTime(), end: end.getTime(), timeZone: zone };
  }

  function submit(e: SubmitEvent) {
    e.preventDefault();
    if (readOnly) return;
    const t = buildTiming();
    if (typeof t === "string") {
      error = t;
      return;
    }
    const invalid = validateTiming(t);
    if (invalid) {
      error = invalid;
      return;
    }
    error = onsave({ calendarId, title: title.trim(), description, location, timing: t, rrule });
  }

  function keydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onclose();
    } else if (e.key === "Tab") {
      trapFocus(e);
    }
  }

  function trapFocus(e: KeyboardEvent) {
    const focusable = dialog.querySelectorAll<HTMLElement>(
      "input:not([disabled]), select:not([disabled]), textarea:not([disabled]), button:not([disabled])",
    );
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (!first || !last) return;
    if (e.shiftKey && document.activeElement === first) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && document.activeElement === last) {
      e.preventDefault();
      first.focus();
    }
  }

  onMount(() => {
    const previous = document.activeElement as HTMLElement | null;
    (readOnly ? dialog : titleInput)?.focus();
    return () => previous?.focus?.();
  });
</script>

<div class="backdrop" role="presentation" onmousedown={(e) => e.target === e.currentTarget && onclose()}>
  <div
    class="dialog"
    role="dialog"
    aria-modal="true"
    aria-labelledby="editor-title"
    tabindex="-1"
    bind:this={dialog}
    onkeydown={keydown}
  >
    <form onsubmit={submit} novalidate>
      <h2 id="editor-title">{heading}</h2>
      {#if readOnly}
        <p class="note" role="note">This calendar is read-only.</p>
      {:else if recurring}
        <p class="note" role="note">
          Saving applies to every event in this series. Drag an event in the calendar to move just that one.
        </p>
      {/if}
      <fieldset disabled={readOnly}>
        <label class="full">
          <span>Title</span>
          <input bind:this={titleInput} bind:value={title} placeholder="(No title)" autocomplete="off" />
        </label>
        <label class="full">
          <span>Calendar</span>
          <select bind:value={calendarId}>
            {#each choices as c (c.id)}
              <option value={c.id}>{c.name}</option>
            {/each}
          </select>
        </label>
        <label class="check">
          <input type="checkbox" bind:checked={allDay} />
          <span>All day</span>
        </label>
        <div class="when">
          <label>
            <span>Starts</span>
            <input
              type="date"
              value={startDate}
              required
              oninput={(e) => {
                const v = e.currentTarget.value;
                moveStart(() => (startDate = v));
              }}
            />
          </label>
          {#if !allDay}
            <TimeField label="Start time" value={startTime} onchange={(v) => moveStart(() => (startTime = v))} />
          {/if}
          <label>
            <span>Ends</span>
            <input type="date" bind:value={endDate} required />
          </label>
          {#if !allDay}
            <TimeField label="End time" bind:value={endTime} relativeTo={startDate === endDate ? startTime : null} />
          {/if}
        </div>
        <label class="full">
          <span>Repeat</span>
          <select bind:value={rrule}>
            {#each repeatChoices as choice (choice.rrule)}
              <option value={choice.rrule}>{choice.label}</option>
            {/each}
          </select>
        </label>
        <label class="full">
          <span>Location</span>
          <input bind:value={location} autocomplete="off" />
        </label>
        <label class="full">
          <span>Description</span>
          <textarea bind:value={description} rows="3"></textarea>
        </label>
      </fieldset>
      {#if error}
        <p class="error" role="alert">{error}</p>
      {/if}
      <div class="actions">
        {#if event && !readOnly}
          {#if recurring}
            <button type="button" class="danger" onclick={() => ondelete("occurrence")}>Delete this event</button>
            <button type="button" class="danger" onclick={() => ondelete("series")}>Delete series</button>
          {:else}
            <button type="button" class="danger" onclick={() => ondelete("occurrence")}>Delete</button>
          {/if}
        {/if}
        <span class="spacer"></span>
        <button type="button" onclick={onclose}>{readOnly ? "Close" : "Cancel"}</button>
        {#if !readOnly}
          <button type="submit" class="primary">Save</button>
        {/if}
      </div>
    </form>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: var(--uc-backdrop);
    display: grid;
    place-items: center;
    z-index: 50;
  }
  .dialog {
    width: min(34rem, calc(100vw - 2rem));
    max-height: calc(100vh - 2rem);
    overflow: auto;
    background: var(--uc-surface);
    color: var(--uc-text);
    border: 1px solid var(--uc-border);
    border-radius: 10px;
    box-shadow: 0 12px 40px rgb(0 0 0 / 0.3);
    padding: 1.25rem;
    outline: none;
  }
  h2 {
    margin: 0 0 0.75rem;
    font-size: 1.1rem;
  }
  fieldset {
    border: 0;
    padding: 0;
    margin: 0;
    display: grid;
    gap: 0.6rem;
  }
  label {
    display: grid;
    gap: 0.25rem;
    font-size: 0.85rem;
  }
  label > span {
    color: var(--uc-text-muted);
  }
  .check {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }
  .when {
    display: grid;
    grid-template-columns: 1fr auto;
    gap: 0.5rem;
    align-items: end;
  }
  .note {
    margin: 0 0 0.75rem;
    color: var(--uc-text-muted);
  }
  .error {
    color: var(--uc-danger);
    margin: 0.75rem 0 0;
  }
  .actions {
    display: flex;
    gap: 0.5rem;
    margin-top: 1rem;
  }
  .spacer {
    flex: 1;
  }
  .actions button {
    white-space: nowrap;
  }
</style>
