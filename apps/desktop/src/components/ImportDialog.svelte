<!--
  Import events from an .ics file into a writable calendar: choose a file
  and a calendar, import, then a summary of what was imported, skipped or
  changed.
-->
<script lang="ts">
  import { onMount, tick, untrack } from "svelte";
  import { modal } from "../services/modal";
  import type { CalendarsStore } from "../stores/calendars.svelte";
  import type { CalendarId, ImportReport, OpenedFile } from "../types/calendar";

  interface Props {
    calendars: CalendarsStore;
    /** Preselected calendar (the default for new events). */
    defaultCalendarId: CalendarId | null;
    /** A file opened from the file manager, already read. */
    opened?: OpenedFile | null;
    /** Imports the file's text; rejects with a message when nothing could be imported. */
    onimport: (calendarId: CalendarId, text: string) => Promise<ImportReport>;
    onclose: () => void;
  }

  let { calendars, defaultCalendarId, opened = null, onimport, onclose }: Props = $props();

  /** Same limit as the engine; checked here to avoid reading huge files. */
  const MAX_BYTES = 20 * 1024 * 1024;

  const byAccount = $derived(
    calendars.accounts
      .map((account) => ({ account, items: calendars.writable.filter((c) => c.accountId === account.id) }))
      .filter((a) => a.items.length > 0),
  );

  /** The file to import: picked here, or opened from the file manager. */
  interface Source {
    name: string;
    size: number;
    read: () => Promise<string>;
  }
  let file = $state<Source | null>(
    untrack(() => (opened ? { name: opened.name, size: opened.text.length, read: async () => opened!.text } : null)),
  );
  const fromFileManager = untrack(() => opened !== null);
  let calendarId = $state<CalendarId>("");
  let busy = $state(false);
  let error = $state<string | null>(null);
  let report = $state<ImportReport | null>(null);
  let fileInput: HTMLInputElement | undefined = $state();
  let calendarSelect: HTMLSelectElement | undefined = $state();
  let doneButton: HTMLButtonElement | undefined = $state();

  const target = $derived(calendarId ? calendars.label(calendarId) : "");
  const problems = $derived(report ? report.failed.length + report.adjusted.length + report.unlisted : 0);

  // The calendars may still be loading when the app was opened with a file.
  $effect(() => {
    if (!calendarId || !calendars.writable.some((c) => c.id === calendarId))
      calendarId = defaultCalendarId ?? calendars.writable[0]?.id ?? "";
  });

  onMount(() => {
    // With a file already chosen, what is left to decide is the calendar.
    (fromFileManager ? calendarSelect : fileInput)?.focus();
  });

  function pick(e: Event & { currentTarget: HTMLInputElement }) {
    const picked = e.currentTarget.files?.[0];
    file = picked ? { name: picked.name, size: picked.size, read: () => picked.text() } : null;
    error = null;
  }

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    if (busy) return;
    if (!file) {
      error = "Choose an .ics file to import.";
      fileInput?.focus();
      return;
    }
    if (!calendarId) {
      error = "Choose the calendar to import into.";
      return;
    }
    if (file.size > MAX_BYTES) {
      error = "This file is too large to import (the limit is 20 MB).";
      return;
    }
    busy = true;
    error = null;
    try {
      report = await onimport(calendarId, await file.read());
      await tick();
      doneButton?.focus();
    } catch (err) {
      error = err instanceof Error ? err.message : "The file could not be imported.";
    } finally {
      busy = false;
    }
  }

  function summary(r: ImportReport): string {
    const parts: string[] = [];
    if (r.imported > 0) parts.push(`Imported ${r.imported} ${r.imported === 1 ? "event" : "events"} into ${target}.`);
    if (r.alreadyPresent > 0)
      parts.push(`${r.alreadyPresent} ${r.alreadyPresent === 1 ? "event was" : "events were"} already in this calendar.`);
    if (r.failed.length > 0)
      parts.push(`${r.failed.length} ${r.failed.length === 1 ? "event" : "events"} could not be imported.`);
    if (parts.length === 0) parts.push("The file contains no events.");
    return parts.join(" ");
  }
</script>

<div class="backdrop" role="presentation" onmousedown={(e) => e.target === e.currentTarget && !busy && onclose()}>
  <div
    class="dialog"
    role="dialog"
    aria-modal="true"
    aria-labelledby="import-title"
    tabindex="-1"
    use:modal
    onkeydown={(e) => {
      if (e.key === "Escape" && !busy) {
        e.preventDefault();
        e.stopPropagation();
        onclose();
      }
    }}
  >
    <h2 id="import-title">Import events</h2>
    {#if report}
      <p class="result" role="status">{summary(report)}</p>
      {#if problems > 0}
        <ul class="problems" aria-label="Events skipped or changed">
          {#each report.failed as p, i (i)}
            <li><strong>{p.title || "(Unreadable event)"}</strong>: not imported, {p.reason}</li>
          {/each}
          {#each report.adjusted as p, i (i)}
            <li><strong>{p.title}</strong>: imported, {p.reason}</li>
          {/each}
          {#if report.unlisted > 0}
            <li>…and {report.unlisted} more.</li>
          {/if}
        </ul>
      {/if}
      <div class="actions">
        <span class="spacer"></span>
        <button type="button" class="primary" bind:this={doneButton} onclick={onclose}>Done</button>
      </div>
    {:else}
      <form onsubmit={submit} novalidate>
        <p class="hint">
          Copies the events of an iCalendar file (<code>.ics</code>) into one of your calendars. In a synchronized
          calendar they are then uploaded to the server. Events already imported are skipped.
        </p>
        <fieldset disabled={busy}>
          {#if fromFileManager && file}
            <p class="opened">File: <strong>{file.name}</strong></p>
          {/if}
          <label>
            <span>{fromFileManager ? "Or choose another file" : "File"}</span>
            <input bind:this={fileInput} type="file" accept=".ics,.ical,.ifb,.icalendar,text/calendar" onchange={pick} />
          </label>
          <label>
            <span>Import into</span>
            <select bind:this={calendarSelect} bind:value={calendarId}>
              {#each byAccount as { account, items } (account.id)}
                <optgroup label={account.name}>
                  {#each items as cal (cal.id)}
                    <option value={cal.id}>{cal.name}</option>
                  {/each}
                </optgroup>
              {/each}
            </select>
          </label>
        </fieldset>
        {#if byAccount.length === 0}
          <p class="error" role="alert">There is no calendar to import into: every calendar is read-only.</p>
        {/if}
        {#if error}
          <p class="error" role="alert">{error}</p>
        {/if}
        <div class="actions">
          {#if busy}<span class="status" role="status">Importing…</span>{/if}
          <span class="spacer"></span>
          <button type="button" onclick={onclose} disabled={busy}>Cancel</button>
          <button type="submit" class="primary" disabled={busy || byAccount.length === 0}>Import</button>
        </div>
      </form>
    {/if}
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
    width: min(30rem, calc(100vw - 2rem));
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
    margin: 0 0 0.5rem;
    font-size: 1.1rem;
  }
  fieldset {
    border: 0;
    padding: 0;
    margin: 0.75rem 0;
    display: grid;
    gap: 0.6rem;
  }
  label {
    display: grid;
    gap: 0.25rem;
    font-size: 0.85rem;
  }
  label > span,
  .hint,
  .status {
    color: var(--uc-text-muted);
    font-size: 0.8rem;
  }
  .hint {
    margin: 0;
  }
  .opened {
    margin: 0;
    overflow-wrap: anywhere;
  }
  .result {
    margin: 0.5rem 0;
  }
  .problems {
    margin: 0.5rem 0;
    padding-left: 1.2rem;
    font-size: 0.85rem;
    max-height: 14rem;
    overflow: auto;
  }
  .error {
    color: var(--uc-danger);
    margin: 0.75rem 0 0;
  }
  .actions {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    margin-top: 1rem;
  }
  .spacer {
    flex: 1;
  }
</style>
