<script lang="ts">
  import { onMount, tick, untrack } from "svelte";
  import type { Calendar, CalendarEvent, EventDraft, EventTiming, Resolution } from "../types/calendar";
  import type { SeriesScope } from "../stores/events.svelte";
  import { buildRule, defaultForm, isPreset, parseRule, REPEAT_PRESETS, type RuleForm } from "../services/recurrence";
  import { MAX_REMINDERS, normalizeReminders, REMINDER_PRESETS, reminderLabel } from "../services/reminders";
  import RepeatEditor from "./RepeatEditor.svelte";
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
    /** Saves; for an occurrence of a series, `scope` says which events change. */
    onsave: (draft: EventDraft, scope: SeriesScope) => string | null;
    /** For recurring events: this occurrence, it and the later ones, or the whole series. */
    ondelete: (scope: SeriesScope) => void;
    onclose: () => void;
    /** Resolve a sync conflict or failure for this event. */
    onresolve?: (resolution: Resolution) => void;
    /** How many reminders a calendar holds (Outlook: one). */
    reminderLimit?: (calendarId: string) => number;
    /** The event changed (e.g. through sync) since the editor opened. */
    newer?: boolean;
    onshownewer?: () => void;
  }

  let {
    event,
    timing,
    calendars,
    defaultCalendarId,
    readOnly,
    onsave,
    ondelete,
    onclose,
    onresolve,
    reminderLimit = () => MAX_REMINDERS,
    newer = false,
    onshownewer,
  }: Props = $props();

  const syncIssue = $derived(
    event?.syncStatus === "conflict" ? "conflict" : event?.syncStatus === "sync-failed" ? "failed" : null,
  );

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
  const recurring = untrack(() => event?.seriesId != null);
  // Repeat: "none", a preset rule, "custom" (edited in RepeatEditor), or
  // "keep" for a rule the custom editor cannot show (kept as it is).
  const initStart = untrack(() => {
    const t = event?.timing ?? timing;
    return t.kind === "all-day" ? parseIsoDate(t.startDate) : new Date(t.start);
  });
  const initForm = parseRule(init.rrule, initStart);
  let repeat = $state(
    init.rrule === null ? "none" : isPreset(init.rrule) ? init.rrule : initForm ? "custom" : "keep",
  );
  let ruleForm = $state<RuleForm>(initForm ?? defaultForm(initStart));
  let reminders = $state<number[]>(untrack(() => [...(event?.reminders ?? [])]));
  const limit = $derived(reminderLimit(calendarId));
  const reminderChoices = $derived(REMINDER_PRESETS.filter((m) => !reminders.includes(m)));
  const keptLabel = init.rrule === "" ? "On specific dates" : `Custom (${init.rrule})`;
  let allDay = $state(init.allDay);
  let startDate = $state(init.startDate);
  let endDate = $state(init.endDate);
  let startTime = $state(init.startTime);
  let endTime = $state(init.endTime);
  /** The series start as currently entered, which custom rules are relative to. */
  const ruleStart = $derived(combineDateTime(startDate, allDay ? "00:00" : startTime) ?? initStart);
  let error = $state<string | null>(null);
  /** Asking which occurrences of the series a save or delete applies to. */
  let choosing = $state.raw<{ action: "save"; draft: EventDraft } | { action: "delete" } | null>(null);
  let scopeGroup = $state<HTMLDivElement>();

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
    // Fields shown as entered keep the original instant exactly: the form
    // has minute precision, and a wall time in a DST fall-back hour names
    // two instants. Only a field the user changed is converted again.
    const original = untrack(() => event?.timing);
    const kept = (date: string, time: string, which: "start" | "end"): number | null =>
      original?.kind === "timed" && !init.allDay && date === init[`${which}Date`] && time === init[`${which}Time`]
        ? original[which]
        : null;
    const start = kept(startDate, startTime, "start") ?? combineDateTime(startDate, startTime)?.getTime();
    const end = kept(endDate, endTime, "end") ?? combineDateTime(endDate, endTime)?.getTime();
    if (start == null || end == null) return "Enter dates as YYYY-MM-DD and times as HH:MM.";
    const zone = event?.timing.kind === "timed" ? event.timing.timeZone : localTimeZone();
    return { kind: "timed", start, end, timeZone: zone };
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
    if (reminders.length > limit) {
      error = limit === 1 ? "This calendar holds one reminder per event." : `This calendar holds at most ${limit} reminders per event.`;
      return;
    }
    const draft: EventDraft = {
      calendarId,
      title: title.trim(),
      description,
      location,
      timing: t,
      rrule: currentRule(t),
      reminders: normalizeReminders(reminders),
    };
    if (recurring && event) {
      const { calendarId: c, title: ti, description: d, location: l, timing: tm, rrule: r, reminders: rm } = event;
      const unchanged =
        JSON.stringify(draft) ===
        JSON.stringify({ calendarId: c, title: ti, description: d, location: l, timing: tm, rrule: r, reminders: rm });
      if (unchanged) return onclose();
      void ask({ action: "save", draft });
      return;
    }
    error = onsave(draft, "series");
  }

  function currentRule(t: EventTiming): string | null {
    switch (repeat) {
      case "none":
        return null;
      case "keep":
        return init.rrule;
      case "custom":
        return buildRule(ruleForm, ruleStart, t.kind === "all-day" ? "date" : t.timeZone === null ? "floating" : "zoned");
      default:
        return repeat;
    }
  }

  function chooseRepeat(value: string): void {
    // Opening the custom editor starts from the rule chosen so far.
    if (value === "custom" && repeat !== "keep") {
      ruleForm = parseRule(repeat === "none" ? null : repeat, ruleStart) ?? defaultForm(ruleStart);
    }
    repeat = value;
  }

  async function ask(question: NonNullable<typeof choosing>): Promise<void> {
    error = null;
    choosing = question;
    await tick();
    scopeGroup?.querySelector<HTMLButtonElement>("button:not([disabled])")?.focus();
  }

  /** "This event" only fits changes a single occurrence can hold. */
  const singleAllowed = $derived(
    choosing?.action !== "save" ||
      (choosing.draft.rrule === event?.rrule && choosing.draft.calendarId === event?.calendarId),
  );

  function choose(scope: SeriesScope): void {
    const question = choosing;
    if (!question) return;
    if (question.action === "delete") {
      ondelete(scope);
      return;
    }
    choosing = null;
    error = onsave(question.draft, scope);
  }

  function keydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      if (choosing) choosing = null;
      else onclose();
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
      {#if newer && onshownewer}
        <div class="banner info" role="status">
          <p>This event was changed elsewhere while you had it open. Saving now keeps your version.</p>
          <div class="banner-actions">
            <button type="button" onclick={onshownewer}>Show the new version</button>
          </div>
        </div>
      {/if}
      {#if syncIssue && onresolve}
        <div class="banner" role="alert">
          <p>
            {syncIssue === "conflict"
              ? "This event was also changed on the server. Choose which version to keep."
              : "Your change could not be saved to the server."}
          </p>
          <div class="banner-actions">
            <button type="button" onclick={() => onresolve(syncIssue === "conflict" ? "keep-local" : "retry")}
              >{syncIssue === "conflict" ? "Keep mine" : "Try again"}</button
            >
            <button type="button" onclick={() => onresolve("use-server")}>
              {syncIssue === "conflict" ? "Use server version" : "Discard my change"}
            </button>
          </div>
        </div>
      {/if}
      {#if readOnly}
        <p class="note" role="note">This calendar is read-only.</p>
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
          <select value={repeat} onchange={(e) => chooseRepeat(e.currentTarget.value)}>
            {#each REPEAT_PRESETS as preset (preset.rrule)}
              <option value={preset.rrule ?? "none"}>{preset.label}</option>
            {/each}
            <option value="custom">Custom…</option>
            {#if repeat === "keep" || init.rrule !== null && !isPreset(init.rrule) && !initForm}
              <option value="keep">{keptLabel}</option>
            {/if}
          </select>
        </label>
        {#if repeat === "custom"}
          <RepeatEditor bind:form={ruleForm} start={ruleStart} />
        {/if}
        <div class="reminders" role="group" aria-labelledby="reminders-label">
          <span id="reminders-label">Reminders</span>
          {#if reminders.length > 0}
            <ul>
              {#each normalizeReminders(reminders) as minutes (minutes)}
                <li>
                  <span>{reminderLabel(minutes)}</span>
                  <button
                    type="button"
                    class="remove"
                    aria-label={`Remove reminder ${reminderLabel(minutes)}`}
                    title="Remove"
                    onclick={() => (reminders = reminders.filter((m) => m !== minutes))}>×</button
                  >
                </li>
              {/each}
            </ul>
          {/if}
          {#if reminders.length < limit}
            <select
              aria-label="Add reminder"
              value=""
              onchange={(e) => {
                const value = e.currentTarget.value;
                e.currentTarget.value = "";
                if (value !== "") reminders = [...reminders, Number(value)];
              }}
            >
              <option value="" disabled>{reminders.length ? "Add another reminder…" : "Add a reminder…"}</option>
              {#each reminderChoices as minutes (minutes)}
                <option value={minutes}>{reminderLabel(minutes)}</option>
              {/each}
            </select>
          {/if}
          {#if limit === 1}
            <p class="hint">This calendar holds one reminder per event.</p>
          {/if}
        </div>
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
      {#if choosing}
        <div class="scope" role="group" aria-labelledby="scope-question" bind:this={scopeGroup}>
          <p id="scope-question">
            {choosing.action === "save" ? "This is a repeating event. Save the changes for:" : "This is a repeating event. Delete:"}
          </p>
          <div class="scope-choices">
            <button type="button" class:danger={choosing.action === "delete"} disabled={!singleAllowed} onclick={() => choose("occurrence")}>
              This event
            </button>
            <button type="button" class:danger={choosing.action === "delete"} onclick={() => choose("following")}>
              This and following events
            </button>
            <button type="button" class:danger={choosing.action === "delete"} onclick={() => choose("series")}>
              All events
            </button>
            <span class="spacer"></span>
            <button type="button" onclick={() => (choosing = null)}>Back</button>
          </div>
          {#if !singleAllowed}
            <p class="hint">A new repeat rule or calendar can't apply to a single event.</p>
          {/if}
        </div>
      {:else}
        <div class="actions">
          {#if event && !readOnly}
            <button
              type="button"
              class="danger"
              onclick={() => (recurring ? void ask({ action: "delete" }) : ondelete("occurrence"))}>Delete</button
            >
          {/if}
          <span class="spacer"></span>
          <button type="button" onclick={onclose}>{readOnly ? "Close" : "Cancel"}</button>
          {#if !readOnly}
            <button type="submit" class="primary">Save</button>
          {/if}
        </div>
      {/if}
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
  .banner {
    border: 1px solid var(--uc-danger);
    border-radius: 6px;
    padding: 0.5rem 0.75rem;
    margin: 0 0 0.75rem;
  }
  .banner.info {
    border-color: var(--uc-accent);
  }
  .banner p {
    margin: 0 0 0.5rem;
  }
  .banner-actions {
    display: flex;
    gap: 0.5rem;
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
  .reminders {
    display: grid;
    gap: 0.3rem;
    font-size: 0.85rem;
  }
  .reminders > span {
    color: var(--uc-text-muted);
  }
  .reminders ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-wrap: wrap;
    gap: 0.35rem;
  }
  .reminders li {
    display: flex;
    align-items: center;
    gap: 0.25rem;
    padding: 0.1rem 0.25rem 0.1rem 0.6rem;
    border: 1px solid var(--uc-border);
    border-radius: 999px;
    background: var(--uc-surface-alt);
  }
  .reminders .remove {
    border: none;
    background: transparent;
    padding: 0 0.35rem;
    line-height: 1.2;
  }
  .reminders .hint {
    margin: 0;
    color: var(--uc-text-muted);
  }
  .scope {
    margin-top: 1rem;
    padding: 0.75rem;
    border: 1px solid var(--uc-border);
    border-radius: 8px;
    background: var(--uc-surface-alt);
  }
  .scope p {
    margin: 0 0 0.6rem;
  }
  .scope-choices {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
  }
  .scope .hint {
    margin: 0.6rem 0 0;
    font-size: 0.8rem;
    color: var(--uc-text-muted);
  }
</style>
