<script lang="ts">
  import type { Notifications } from "../stores/notifications.svelte";

  let { notifications }: { notifications: Notifications } = $props();
</script>

<!-- Errors are announced at once (alert); other messages when convenient (status). -->
<div class="toasts">
  {#each [{ kind: "info", role: "status", live: "polite" }, { kind: "error", role: "alert", live: "assertive" }] as region (region.kind)}
    <div class="region" role={region.role} aria-live={region.live as "polite" | "assertive"}>
      {#each notifications.items.filter((n) => (n.kind === "error") === (region.kind === "error")) as n (n.id)}
        <div class="toast" class:error={n.kind === "error"}>
          <span>{n.message}</span>
          <button type="button" onclick={() => notifications.dismiss(n.id)} aria-label="Dismiss">✕</button>
        </div>
      {/each}
    </div>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    left: 50%;
    bottom: 1rem;
    transform: translateX(-50%);
    display: grid;
    gap: 0.5rem;
    z-index: 60;
  }
  .region {
    display: grid;
    gap: 0.5rem;
  }
  .toast {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    padding: 0.5rem 0.75rem;
    border-radius: 8px;
    background: var(--uc-surface);
    border: 1px solid var(--uc-border);
    box-shadow: 0 6px 24px rgb(0 0 0 / 0.2);
  }
  .toast.error {
    border-color: var(--uc-danger);
  }
  button {
    border: 0;
    background: none;
    padding: 0 0.25rem;
  }
</style>
