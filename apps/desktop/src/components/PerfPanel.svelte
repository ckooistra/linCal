<script lang="ts">
  import { onDestroy } from "svelte";
  import { perfLog, type Stats } from "../services/perf";

  interface Props {
    eventCount: number;
    cachedCount: number;
    longTasksSupported: boolean;
    longTasks: number;
    onclose: () => void;
  }

  let { eventCount, cachedCount, longTasksSupported, longTasks, onclose }: Props = $props();

  let stats = $state.raw<Stats[]>(perfLog.stats());
  // Coalesce updates to one per frame while interactions are being recorded.
  let scheduled = false;
  const unsubscribe = perfLog.subscribe(() => {
    if (scheduled) return;
    scheduled = true;
    requestAnimationFrame(() => {
      scheduled = false;
      stats = perfLog.stats();
    });
  });
  onDestroy(unsubscribe);

  const fmt = (n: number) => n.toFixed(1);
</script>

<aside class="perf" aria-label="Performance">
  <header>
    <strong>Performance</strong>
    <button type="button" onclick={() => perfLog.clear()}>Clear</button>
    <button type="button" onclick={onclose} aria-label="Close performance panel">✕</button>
  </header>
  <p>Dataset {eventCount.toLocaleString()} · cached {cachedCount.toLocaleString()}</p>
  <p>Long tasks: {longTasksSupported ? longTasks : "not supported by this engine"}</p>
  <table>
    <thead>
      <tr><th>interaction</th><th>n</th><th>p50</th><th>p95</th><th>max</th><th>js p50</th></tr>
    </thead>
    <tbody>
      {#each stats as s (s.name)}
        <tr>
          <td>{s.name}</td><td>{s.count}</td><td>{fmt(s.p50)}</td><td>{fmt(s.p95)}</td><td>{fmt(s.max)}</td><td
            >{fmt(s.scriptP50)}</td
          >
        </tr>
      {:else}
        <tr><td colspan="6">Interact with the calendar to record timings (ms).</td></tr>
      {/each}
    </tbody>
  </table>
</aside>

<style>
  .perf {
    position: fixed;
    right: 0.75rem;
    bottom: 0.75rem;
    z-index: 40;
    background: var(--uc-surface);
    border: 1px solid var(--uc-border);
    border-radius: 8px;
    box-shadow: 0 6px 24px rgb(0 0 0 / 0.2);
    padding: 0.5rem 0.75rem;
    font-size: 0.75rem;
    max-height: 50vh;
    overflow: auto;
    font-variant-numeric: tabular-nums;
  }
  header {
    display: flex;
    gap: 0.5rem;
    align-items: center;
  }
  header strong {
    flex: 1;
  }
  p {
    margin: 0.3rem 0;
    color: var(--uc-text-muted);
  }
  table {
    border-collapse: collapse;
  }
  th,
  td {
    padding: 0.1rem 0.4rem;
    text-align: right;
  }
  th:first-child,
  td:first-child {
    text-align: left;
  }
</style>
