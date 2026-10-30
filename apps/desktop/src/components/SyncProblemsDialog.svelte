<script lang="ts">
  import { onMount } from "svelte";
  import type { Resolution, SyncProblem } from "../types/calendar";
  import { modal } from "../services/modal";

  interface Props {
    /** null while loading. */
    problems: SyncProblem[] | null;
    onresolve: (problem: SyncProblem, resolution: Resolution) => void;
    onclose: () => void;
  }

  let { problems, onresolve, onclose }: Props = $props();
  let close: HTMLButtonElement;
  onMount(() => close.focus());

  /** Button labels for the two choices of a problem. */
  function choices(p: SyncProblem): [Resolution, string][] {
    if (p.conflict) {
      return p.deletion
        ? [
            ["keep-local", "Delete anyway"],
            ["use-server", "Keep the server's event"],
          ]
        : [
            ["keep-local", "Keep mine"],
            ["use-server", "Use server version"],
          ];
    }
    return [
      ["retry", "Try again"],
      ["use-server", p.deletion ? "Restore the event" : "Discard my change"],
    ];
  }
</script>

<div class="backdrop" role="presentation" onmousedown={(e) => e.target === e.currentTarget && onclose()}>
  <div
    class="dialog"
    role="dialog"
    aria-modal="true"
    aria-labelledby="problems-title"
    tabindex="-1"
    use:modal
    onkeydown={(e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        onclose();
      }
    }}
  >
    <h2 id="problems-title">Changes that need you</h2>
    {#if problems === null}
      <p role="status">Loading…</p>
    {:else if problems.length === 0}
      <p role="status">Nothing to resolve.</p>
    {:else}
      <ul>
        {#each problems as p (p.calendarId + p.href)}
          <li>
            <strong>{p.title}</strong>
            {#if p.deletion}<span class="tag">deleted here</span>{/if}
            <p>{p.message}</p>
            <div class="actions">
              {#each choices(p) as [resolution, label] (resolution)}
                <button type="button" onclick={() => onresolve(p, resolution)}>{label}</button>
              {/each}
            </div>
          </li>
        {/each}
      </ul>
    {/if}
    <div class="footer">
      <button type="button" bind:this={close} onclick={onclose}>Close</button>
    </div>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: var(--uc-backdrop);
    display: grid;
    place-items: center;
    z-index: 55;
  }
  .dialog {
    width: min(32rem, calc(100vw - 2rem));
    max-height: calc(100vh - 4rem);
    overflow: auto;
    background: var(--uc-surface);
    border: 1px solid var(--uc-border);
    border-radius: 10px;
    padding: 1.25rem;
    box-shadow: 0 12px 40px rgb(0 0 0 / 0.3);
  }
  h2 {
    margin: 0 0 0.75rem;
    font-size: 1.05rem;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  li {
    padding: 0.6rem 0;
    border-top: 1px solid var(--uc-border);
  }
  li p {
    margin: 0.25rem 0 0.5rem;
    color: var(--uc-text-muted);
    font-size: 0.9rem;
  }
  .tag {
    margin-left: 0.4rem;
    font-size: 0.75rem;
    color: var(--uc-text-muted);
  }
  .actions,
  .footer {
    display: flex;
    gap: 0.5rem;
    flex-wrap: wrap;
  }
  .footer {
    justify-content: flex-end;
    margin-top: 0.75rem;
  }
</style>
