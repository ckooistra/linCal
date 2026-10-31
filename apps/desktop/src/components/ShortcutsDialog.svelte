<script lang="ts">
  import { onMount } from "svelte";
  import { modal } from "../services/modal";
  import { SHORTCUT_HELP } from "../services/shortcuts";

  let { onclose }: { onclose: () => void } = $props();
  let close: HTMLButtonElement;
  onMount(() => close.focus());
</script>

<div class="backdrop" role="presentation" onmousedown={(e) => e.target === e.currentTarget && onclose()}>
  <div
    class="dialog"
    role="dialog"
    aria-modal="true"
    aria-labelledby="shortcuts-title"
    tabindex="-1"
    use:modal
    onkeydown={(e) => {
      if (e.key === "Escape" || e.key === "?") {
        e.preventDefault();
        e.stopPropagation();
        onclose();
      }
    }}
  >
    <h2 id="shortcuts-title">Keyboard shortcuts</h2>
    <p class="hint">Single-letter shortcuts work whenever you are not typing in a field.</p>
    <table>
      <tbody>
        {#each SHORTCUT_HELP as s (s.action)}
          <tr>
            <th scope="row">
              {#each s.keys as key, i (key)}{#if i > 0}<span class="plus">{s.either ? "or" : "+"}</span>{/if}<kbd>{key}</kbd>{/each}
            </th>
            <td>{s.action}</td>
          </tr>
        {/each}
      </tbody>
    </table>
    <div class="actions">
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
    max-height: calc(100vh - 2rem);
    overflow: auto;
    background: var(--uc-surface);
    border: 1px solid var(--uc-border);
    border-radius: 10px;
    padding: 1.25rem;
    box-shadow: 0 12px 40px rgb(0 0 0 / 0.3);
  }
  h2 {
    margin: 0 0 0.25rem;
    font-size: 1.05rem;
  }
  .hint {
    margin: 0 0 0.75rem;
    color: var(--uc-text-muted);
    font-size: 0.85rem;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.9rem;
  }
  th,
  td {
    text-align: left;
    padding: 0.3rem 0.4rem;
    border-top: 1px solid var(--uc-border);
    vertical-align: top;
  }
  th {
    white-space: nowrap;
    font-weight: normal;
  }
  kbd {
    display: inline-block;
    min-width: 1.6em;
    padding: 0.05rem 0.35rem;
    border: 1px solid var(--uc-control-border);
    border-bottom-width: 2px;
    border-radius: 4px;
    font: inherit;
    font-size: 0.85em;
    text-align: center;
    background: var(--uc-surface-alt);
  }
  .plus {
    margin: 0 0.2rem;
    color: var(--uc-text-muted);
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    margin-top: 1rem;
  }
</style>
