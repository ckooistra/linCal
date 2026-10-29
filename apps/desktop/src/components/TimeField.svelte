<!--
  Time-of-day input with a dropdown of times (WebKitGTK's native time input
  has no picker). Typing is accepted in many forms (see parseTimeInput);
  the value is always `HH:MM` (24 h).
-->
<script lang="ts">
  import { tick } from "svelte";
  import {
    formatDuration,
    formatTimeLabel,
    minutesOfDay,
    parseTimeInput,
    prefers12Hour,
    timeFromMinutes,
  } from "../services/time";

  interface Props {
    value: string;
    /** Accessible name of the field. */
    label: string;
    /** Start time; when set, options after it show the resulting duration. */
    relativeTo?: string | null;
    stepMinutes?: number;
    disabled?: boolean;
    onchange?: (value: string) => void;
  }

  let { value = $bindable(), label, relativeTo = null, stepMinutes = 15, disabled = false, onchange }: Props =
    $props();

  const hour12 = prefers12Hour();
  const uid = `time-${Math.random().toString(36).slice(2, 9)}`;

  let text = $state(formatTimeLabel(value, hour12));
  let editing = $state(false);
  let open = $state(false);
  let highlighted = $state(0);
  let input: HTMLInputElement;
  let list = $state<HTMLUListElement>();

  const options = $derived.by(() => {
    const base = relativeTo ? minutesOfDay(relativeTo) : null;
    const result: { value: string; label: string }[] = [];
    for (let m = 0; m < 1440; m += stepMinutes) {
      const v = timeFromMinutes(m);
      let optionLabel = formatTimeLabel(v, hour12);
      if (base !== null && m > base) optionLabel += ` (${formatDuration(m - base)})`;
      result.push({ value: v, label: optionLabel });
    }
    return result;
  });

  // Show the formatted value unless the user is typing.
  $effect(() => {
    const formatted = formatTimeLabel(value, hour12);
    if (!editing) text = formatted;
  });

  function nearestIndex(v: string): number {
    return Math.min(options.length - 1, Math.round(minutesOfDay(v) / stepMinutes));
  }

  async function show() {
    if (disabled || open) return;
    open = true;
    highlighted = nearestIndex(value);
    await tick();
    list?.querySelector<HTMLElement>(`[data-index="${highlighted}"]`)?.scrollIntoView?.({ block: "center" });
  }

  function commit(v: string) {
    if (v !== value) {
      value = v;
      onchange?.(v);
    }
  }

  function choose(index: number) {
    const option = options[index];
    if (!option) return;
    editing = false;
    commit(option.value);
    text = formatTimeLabel(option.value, hour12);
    open = false;
  }

  function oninput(e: Event) {
    editing = true;
    text = (e.currentTarget as HTMLInputElement).value;
    const parsed = parseTimeInput(text);
    if (parsed) {
      commit(parsed);
      highlighted = nearestIndex(parsed);
    }
  }

  function finishTyping() {
    editing = false;
    text = formatTimeLabel(value, hour12);
  }

  async function move(delta: number) {
    if (!open) return show();
    highlighted = Math.max(0, Math.min(options.length - 1, highlighted + delta));
    await tick();
    list?.querySelector<HTMLElement>(`[data-index="${highlighted}"]`)?.scrollIntoView?.({ block: "nearest" });
  }

  function onkeydown(e: KeyboardEvent) {
    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        void move(1);
        break;
      case "ArrowUp":
        e.preventDefault();
        void move(-1);
        break;
      case "Enter":
        if (open) {
          // Pick the highlighted time instead of submitting the form.
          e.preventDefault();
          choose(highlighted);
        } else {
          finishTyping();
        }
        break;
      case "Escape":
        if (open) {
          // Close only the list, not the surrounding dialog.
          e.preventDefault();
          e.stopPropagation();
          open = false;
        }
        break;
      case "Tab":
        open = false;
        finishTyping();
        break;
    }
  }
</script>

<div class="time-field">
  <input
    bind:this={input}
    type="text"
    inputmode="numeric"
    autocomplete="off"
    role="combobox"
    aria-label={label}
    aria-expanded={open}
    aria-controls={`${uid}-list`}
    aria-activedescendant={open ? `${uid}-${highlighted}` : undefined}
    aria-autocomplete="list"
    {disabled}
    value={text}
    onclick={show}
    onfocus={show}
    {oninput}
    {onkeydown}
    onblur={() => {
      open = false;
      finishTyping();
    }}
  />
  {#if open}
    <ul class="options" role="listbox" id={`${uid}-list`} aria-label={label} bind:this={list}>
      {#each options as option, i (option.value)}
        <li
          id={`${uid}-${i}`}
          role="option"
          data-index={i}
          aria-selected={option.value === value}
          class:highlighted={i === highlighted}
          onmousedown={(e) => {
            // Keep focus in the input so blur does not close the list first.
            e.preventDefault();
            choose(i);
          }}
          onmouseenter={() => (highlighted = i)}
        >
          {option.label}
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .time-field {
    position: relative;
    width: 9.5rem;
  }
  input {
    width: 100%;
  }
  .options {
    position: absolute;
    z-index: 70;
    top: calc(100% + 2px);
    left: 0;
    min-width: 100%;
    max-height: 14rem;
    overflow-y: auto;
    margin: 0;
    padding: 0.25rem 0;
    list-style: none;
    background: var(--uc-surface);
    border: 1px solid var(--uc-border);
    border-radius: 6px;
    box-shadow: 0 6px 20px rgb(0 0 0 / 0.2);
  }
  li {
    padding: 0.3rem 0.75rem;
    white-space: nowrap;
    cursor: pointer;
    font-variant-numeric: tabular-nums;
  }
  li.highlighted {
    background: var(--uc-surface-alt);
  }
  li[aria-selected="true"] {
    font-weight: 600;
    color: var(--uc-accent);
  }
</style>
