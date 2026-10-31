<!--
  Fields for a custom repeat rule: interval, weekdays, monthly position and
  end. Edits `form` in place; the event editor turns it into an RRULE.
-->
<script lang="ts">
  import {
    describeRule,
    isLastInMonth,
    monthlyLabel,
    WEEKDAYS,
    weekdayName,
    weekdayOf,
    type Frequency,
    type RuleForm,
    type Weekday,
  } from "../services/recurrence";
  import { addDays, toIsoDate } from "../services/time";

  interface Props {
    form: RuleForm;
    /** Start of the series, which the rule is relative to. */
    start: Date;
  }

  let { form = $bindable(), start }: Props = $props();

  const units: { value: Frequency; one: string; many: string }[] = [
    { value: "DAILY", one: "day", many: "days" },
    { value: "WEEKLY", one: "week", many: "weeks" },
    { value: "MONTHLY", one: "month", many: "months" },
    { value: "YEARLY", one: "year", many: "years" },
  ];

  function toggleDay(day: Weekday): void {
    const has = form.weekdays.includes(day);
    // At least one day: the last one cannot be switched off.
    if (has && form.weekdays.length === 1) return;
    form.weekdays = has ? form.weekdays.filter((d) => d !== day) : [...form.weekdays, day];
  }

  function setEnd(kind: string): void {
    if (kind === "until") form.end = { kind, date: toIsoDate(addDays(start, 30)) };
    else if (kind === "count") form.end = { kind, count: 10 };
    else form.end = { kind: "never" };
  }

  // A weekly rule always includes at least the start's weekday when it has none.
  $effect(() => {
    if (form.frequency === "WEEKLY" && form.weekdays.length === 0) form.weekdays = [weekdayOf(start)];
    if (form.monthly === "last" && !isLastInMonth(start)) form.monthly = "nth";
  });
</script>

<div class="repeat" role="group" aria-label="Custom repeat">
  <div class="row">
    <label class="inline">
      <span>Every</span>
      <input
        type="number"
        min="1"
        max="999"
        value={form.interval}
        oninput={(e) => {
          const n = Math.floor(Number(e.currentTarget.value));
          if (n >= 1) form.interval = Math.min(n, 999);
        }}
        aria-label="Repeat every"
      />
    </label>
    <select bind:value={form.frequency} aria-label="Repeat unit">
      {#each units as u (u.value)}
        <option value={u.value}>{form.interval === 1 ? u.one : u.many}</option>
      {/each}
    </select>
  </div>

  {#if form.frequency === "WEEKLY"}
    <div class="days" role="group" aria-label="On days">
      {#each WEEKDAYS as day (day)}
        <button
          type="button"
          class="day"
          aria-pressed={form.weekdays.includes(day)}
          aria-label={weekdayName(day)}
          title={weekdayName(day)}
          onclick={() => toggleDay(day)}>{weekdayName(day).slice(0, 2)}</button
        >
      {/each}
    </div>
  {:else if form.frequency === "MONTHLY"}
    <select bind:value={form.monthly} aria-label="Day of the month">
      <option value="day">{monthlyLabel("day", start)}</option>
      <option value="nth">{monthlyLabel("nth", start)}</option>
      {#if isLastInMonth(start)}
        <option value="last">{monthlyLabel("last", start)}</option>
      {/if}
    </select>
  {/if}

  <div class="row">
    <label class="inline">
      <span>Ends</span>
      <select value={form.end.kind} onchange={(e) => setEnd(e.currentTarget.value)} aria-label="Repeat ends">
        <option value="never">Never</option>
        <option value="until">On a date</option>
        <option value="count">After a number of times</option>
      </select>
    </label>
    {#if form.end.kind === "until"}
      <input
        type="date"
        value={form.end.date}
        min={toIsoDate(start)}
        aria-label="Last day"
        oninput={(e) => {
          const date = e.currentTarget.value;
          if (/^\d{4}-\d{2}-\d{2}$/.test(date)) form.end = { kind: "until", date };
        }}
      />
    {:else if form.end.kind === "count"}
      <input
        type="number"
        min="1"
        max="9999"
        value={form.end.count}
        aria-label="Number of times"
        oninput={(e) => {
          const n = Math.floor(Number(e.currentTarget.value));
          if (n >= 1) form.end = { kind: "count", count: Math.min(n, 9999) };
        }}
      />
      <span class="unit">times</span>
    {/if}
  </div>
  <p class="summary" aria-live="polite">{describeRule(form, start)}</p>
</div>

<style>
  .repeat {
    display: grid;
    gap: 0.5rem;
    padding: 0.6rem 0.75rem;
    border: 1px solid var(--uc-border);
    border-radius: 8px;
    background: var(--uc-surface-alt);
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.5rem;
  }
  .inline {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.85rem;
  }
  .inline > span {
    color: var(--uc-text-muted);
  }
  input[type="number"] {
    width: 4.5rem;
  }
  .days {
    display: flex;
    flex-wrap: wrap;
    gap: 0.25rem;
  }
  .day {
    min-width: 2.5rem;
  }
  .day[aria-pressed="true"] {
    background: var(--uc-accent);
    border-color: var(--uc-accent);
    color: var(--uc-accent-text);
  }
  .unit,
  .summary {
    font-size: 0.85rem;
    color: var(--uc-text-muted);
  }
  .summary {
    margin: 0;
  }
</style>
