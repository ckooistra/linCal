<!--
  The app's own window title bar (the native frame is off by default so the
  title bar can follow the app theme). Dragging the bar moves the window,
  double-clicking it maximizes; window edges still resize.
-->
<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { windowControls } from "../services/bridge";

  let { title }: { title: string } = $props();

  let maximized = $state(false);
  let stop: (() => void) | null = null;

  onMount(() => {
    void windowControls.onResize((m) => (maximized = m)).then((unlisten) => (stop = unlisten));
  });
  onDestroy(() => stop?.());
</script>

<header class="titlebar" data-tauri-drag-region>
  <svg class="logo" viewBox="0 0 16 16" aria-hidden="true" data-tauri-drag-region>
    <rect x="1.5" y="2.5" width="13" height="12" rx="2" fill="var(--uc-accent)" />
    <rect x="1.5" y="2.5" width="13" height="3.5" rx="1.5" fill="var(--uc-accent-strong)" />
    <rect x="4" y="1" width="1.6" height="3" rx="0.8" fill="currentColor" />
    <rect x="10.4" y="1" width="1.6" height="3" rx="0.8" fill="currentColor" />
  </svg>
  <span class="title" data-tauri-drag-region>{title}</span>
  <div class="controls">
    <button type="button" aria-label="Minimize" title="Minimize" onclick={() => void windowControls.minimize()}>
      <svg viewBox="0 0 10 10" aria-hidden="true"><path d="M1 5.5h8" /></svg>
    </button>
    <button
      type="button"
      aria-label={maximized ? "Restore" : "Maximize"}
      title={maximized ? "Restore" : "Maximize"}
      onclick={() => void windowControls.toggleMaximize()}
    >
      {#if maximized}
        <svg viewBox="0 0 10 10" aria-hidden="true"><path d="M2.5 3.5h5v5h-5z M3.5 1.5h5v5" /></svg>
      {:else}
        <svg viewBox="0 0 10 10" aria-hidden="true"><path d="M1.5 1.5h7v7h-7z" /></svg>
      {/if}
    </button>
    <button type="button" class="close" aria-label="Close" title="Close" onclick={() => void windowControls.close()}>
      <svg viewBox="0 0 10 10" aria-hidden="true"><path d="M2 2l6 6M8 2l-6 6" /></svg>
    </button>
  </div>
</header>

<style>
  .titlebar {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    height: 2.1rem;
    padding-left: 0.6rem;
    background: var(--uc-titlebar);
    color: var(--uc-text);
    border-bottom: 1px solid var(--uc-border);
    user-select: none;
    -webkit-user-select: none;
    flex-shrink: 0;
  }
  .logo {
    width: 1rem;
    height: 1rem;
    color: var(--uc-text);
  }
  .title {
    flex: 1;
    font-size: 0.85rem;
    color: var(--uc-text-muted);
  }
  .controls {
    display: flex;
    height: 100%;
  }
  .controls button {
    width: 2.6rem;
    height: 100%;
    min-height: 0;
    padding: 0;
    border: 0;
    border-radius: 0;
    background: transparent;
    display: grid;
    place-items: center;
    color: var(--uc-text-muted);
  }
  .controls button:hover {
    background: var(--uc-surface-alt);
    color: var(--uc-text);
  }
  .controls button.close:hover {
    background: #c42b1c;
    color: #fff;
  }
  svg path {
    fill: none;
    stroke: currentColor;
    stroke-width: 1;
  }
  .controls svg {
    width: 0.7rem;
    height: 0.7rem;
  }
</style>
