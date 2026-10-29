<script lang="ts">
  import { onMount } from "svelte";

  interface Props {
    title: string;
    message: string;
    confirmLabel: string;
    onconfirm: () => void;
    oncancel: () => void;
  }

  let { title, message, confirmLabel, onconfirm, oncancel }: Props = $props();
  let cancel: HTMLButtonElement;
  onMount(() => cancel.focus());
</script>

<div class="backdrop" role="presentation" onmousedown={(e) => e.target === e.currentTarget && oncancel()}>
  <div
    class="dialog"
    role="alertdialog"
    aria-modal="true"
    aria-labelledby="confirm-title"
    aria-describedby="confirm-message"
    tabindex="-1"
    onkeydown={(e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        oncancel();
      }
    }}
  >
    <h2 id="confirm-title">{title}</h2>
    <p id="confirm-message">{message}</p>
    <div class="actions">
      <button type="button" bind:this={cancel} onclick={oncancel}>Cancel</button>
      <button type="button" class="danger" onclick={onconfirm}>{confirmLabel}</button>
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
    width: min(26rem, calc(100vw - 2rem));
    background: var(--uc-surface);
    border: 1px solid var(--uc-border);
    border-radius: 10px;
    padding: 1.25rem;
    box-shadow: 0 12px 40px rgb(0 0 0 / 0.3);
  }
  h2 {
    margin: 0 0 0.5rem;
    font-size: 1.05rem;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
  }
</style>
