<!--
  Command palette (Ctrl+K): type to filter, ↑/↓ to choose, Enter to run,
  Esc to close. A combobox with a listbox of commands.
-->
<script lang="ts">
  import { onMount, tick } from "svelte";
  import { filterCommands, type Command } from "../services/commands";
  import { modal } from "../services/modal";

  let { commands, onclose }: { commands: readonly Command[]; onclose: () => void } = $props();

  let query = $state("");
  let active = $state(0);
  let input: HTMLInputElement;
  let list = $state<HTMLUListElement>();

  const results = $derived(filterCommands(commands, query));
  const current = $derived(results[Math.min(active, results.length - 1)]);

  function run(command: Command | undefined): void {
    if (!command) return;
    // Close first, so focus is back on the calendar when the command runs.
    onclose();
    command.run();
  }

  async function move(by: number): Promise<void> {
    if (results.length === 0) return;
    active = (Math.min(active, results.length - 1) + by + results.length) % results.length;
    await tick();
    list?.querySelector<HTMLElement>(`[data-index="${active}"]`)?.scrollIntoView({ block: "nearest" });
  }

  function onkeydown(e: KeyboardEvent): void {
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
        e.preventDefault();
        run(current);
        break;
      case "Escape":
        e.preventDefault();
        e.stopPropagation();
        onclose();
        break;
    }
  }

  onMount(() => input.focus());
</script>

<div class="backdrop" role="presentation" onmousedown={(e) => e.target === e.currentTarget && onclose()}>
  <div class="palette" role="dialog" aria-modal="true" aria-label="Command palette" use:modal>
    <input
      bind:this={input}
      bind:value={query}
      oninput={() => (active = 0)}
      {onkeydown}
      type="text"
      role="combobox"
      aria-expanded="true"
      aria-controls="palette-results"
      aria-autocomplete="list"
      aria-activedescendant={current ? `palette-${current.id}` : undefined}
      aria-label="Type a command"
      placeholder="Type a command…"
      autocomplete="off"
      spellcheck="false"
    />
    <!-- Options are chosen with the keyboard in the input (aria-activedescendant); clicks are a shortcut. -->
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <ul id="palette-results" role="listbox" aria-label="Commands" bind:this={list}>
      {#each results as command, i (command.id)}
        <li
          id={`palette-${command.id}`}
          role="option"
          aria-selected={command === current}
          data-index={i}
          onmousedown={(e) => e.preventDefault()}
          onclick={() => run(command)}
          onmousemove={() => (active = i)}
        >
          <span class="title">{command.title}</span>
          <span class="section">{command.section}</span>
          {#if command.shortcut}<kbd>{command.shortcut}</kbd>{/if}
        </li>
      {:else}
        <li class="none" role="presentation">No matching command</li>
      {/each}
    </ul>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 0.15);
    display: flex;
    justify-content: center;
    align-items: flex-start;
    padding-top: 12vh;
    z-index: 58;
  }
  .palette {
    width: min(34rem, calc(100vw - 2rem));
    background: var(--uc-surface);
    color: var(--uc-text);
    border: 1px solid var(--uc-border);
    border-radius: 10px;
    box-shadow: 0 16px 48px rgb(0 0 0 / 0.35);
    overflow: hidden;
  }
  input {
    width: 100%;
    border: none;
    border-bottom: 1px solid var(--uc-border);
    border-radius: 0;
    padding: 0.7rem 0.9rem;
    font-size: 1rem;
    background: transparent;
  }
  input:focus-visible {
    outline: none;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0.3rem;
    max-height: min(22rem, 60vh);
    overflow-y: auto;
  }
  li {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    padding: 0.4rem 0.6rem;
    border-radius: 6px;
    cursor: pointer;
  }
  li[aria-selected="true"] {
    background: var(--uc-accent);
    color: var(--uc-accent-text);
  }
  .title {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .section {
    font-size: 0.75rem;
    opacity: 0.8;
  }
  kbd {
    font: inherit;
    font-size: 0.75rem;
    padding: 0 0.3rem;
    border: 1px solid currentColor;
    border-radius: 4px;
    opacity: 0.8;
  }
  .none {
    color: var(--uc-text-muted);
    cursor: default;
  }
</style>
