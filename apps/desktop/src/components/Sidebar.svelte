<script lang="ts">
  import type { CalendarsStore } from "../stores/calendars.svelte";
  import type { ThemePreference } from "../services/theme";
  import { DATASET_SIZES } from "../services/mock/generator";

  interface Props {
    calendars: CalendarsStore;
    datasetSize: number;
    latencyMs: number;
    themePreference: ThemePreference;
    /** Show latency/failure injection (in-memory preview only). */
    simulationControls?: boolean;
    ontogglecalendar: (id: string) => void;
    oncolor: (id: string, color: string) => void;
    ondataset: (size: number) => void;
    onlatency: (ms: number) => void;
    ontheme: (preference: ThemePreference) => void;
    onfailnext: () => void;
    /** Account actions; omitted in the browser preview. */
    onaddaccount?: () => void;
    onremoveaccount?: (accountId: string) => void;
    onsyncnow?: () => void;
  }

  let {
    calendars,
    datasetSize,
    latencyMs,
    themePreference,
    simulationControls = true,
    ontogglecalendar,
    oncolor,
    ondataset,
    onlatency,
    ontheme,
    onfailnext,
    onaddaccount,
    onremoveaccount,
    onsyncnow,
  }: Props = $props();

  function ago(ms: number): string {
    const s = Math.max(0, Math.round((Date.now() - ms) / 1000));
    if (s < 60) return "just now";
    if (s < 3600) return `${Math.round(s / 60)} min ago`;
    if (s < 86_400) return `${Math.round(s / 3600)} h ago`;
    return new Date(ms).toLocaleDateString();
  }

  /** One-line sync summary for a remote account. */
  function summary(accountId: string): { text: string; tone: "ok" | "busy" | "warn" | "error" } | null {
    const st = calendars.statuses[accountId];
    if (!st?.remote) return null;
    if (calendars.syncing.has(accountId)) return { text: "Syncing…", tone: "busy" };
    if (st.conflicts) return { text: `${st.conflicts} conflict${st.conflicts > 1 ? "s" : ""} to resolve`, tone: "error" };
    if (st.error) return { text: st.error, tone: st.error.startsWith("Offline") ? "warn" : "error" };
    if (st.failed) return { text: `${st.failed} change${st.failed > 1 ? "s" : ""} could not be saved`, tone: "error" };
    if (st.pending) return { text: `${st.pending} change${st.pending > 1 ? "s" : ""} waiting to sync`, tone: "warn" };
    return { text: st.lastSyncedAt ? `Synced ${ago(st.lastSyncedAt)}` : "Not synced yet", tone: "ok" };
  }

  const groups = $derived(
    calendars.accounts.map((account) => ({
      account,
      items: calendars.calendars.filter((c) => c.accountId === account.id),
    })),
  );
</script>

<nav class="sidebar" aria-label="Calendars and settings">
  {#each groups as { account, items } (account.id)}
    {@const sync = summary(account.id)}
    <section>
      <div class="account">
        <h3 title={account.name}>{account.name}</h3>
        {#if onremoveaccount && account.provider !== "simulated" && account.provider !== "local"}
          <button
            type="button"
            class="small"
            title="Remove this account from Universal Calendar"
            onclick={() => onremoveaccount(account.id)}>Remove</button
          >
        {/if}
      </div>
      {#if sync}
        <p class={`sync ${sync.tone}`} role="status">{sync.text}</p>
      {/if}
      <ul>
        {#each items as cal (cal.id)}
          <li>
            <label class="cal">
              <input
                type="checkbox"
                checked={!calendars.hidden.has(cal.id)}
                onchange={() => ontogglecalendar(cal.id)}
                style:accent-color={cal.color}
              />
              <span class="name">{cal.name}</span>
              {#if cal.readOnly}<span class="badge" title="Read-only">RO</span>{/if}
            </label>
            <input
              class="swatch"
              type="color"
              value={cal.color}
              aria-label={`Colour for ${cal.name}`}
              onchange={(e) => oncolor(cal.id, e.currentTarget.value)}
            />
          </li>
        {/each}
      </ul>
    </section>
  {/each}

  {#if onaddaccount}
    <section class="accounts-actions">
      <button type="button" onclick={onaddaccount}>Add CalDAV account…</button>
      {#if onsyncnow && Object.values(calendars.statuses).some((s) => s.remote)}
        <button type="button" onclick={onsyncnow} title="Sync all accounts now">Sync now</button>
      {/if}
    </section>
  {/if}

  <section class="settings">
    <h3>Settings</h3>
    <label>
      <span>Theme</span>
      <select value={themePreference} onchange={(e) => ontheme(e.currentTarget.value as ThemePreference)}>
        <option value="system">System</option>
        <option value="light">Light</option>
        <option value="dark">Dark</option>
      </select>
    </label>
  </section>

  <section class="settings">
    <h3>Simulation</h3>
    <label>
      <span>Dataset (replaces all events)</span>
      <select value={datasetSize} onchange={(e) => ondataset(Number(e.currentTarget.value))}>
        {#each DATASET_SIZES as size (size)}
          <option value={size}>{size.toLocaleString()} events</option>
        {/each}
      </select>
    </label>
    {#if simulationControls}
    <label>
      <span>Storage latency</span>
      <select value={latencyMs} onchange={(e) => onlatency(Number(e.currentTarget.value))}>
        <option value={0}>None</option>
        <option value={50}>50 ms</option>
        <option value={300}>300 ms</option>
        <option value={1500}>1.5 s</option>
      </select>
    </label>
    <button type="button" onclick={onfailnext}>Fail next save</button>
    {/if}
  </section>
</nav>

<style>
  .sidebar {
    display: flex;
    flex-direction: column;
    gap: 1rem;
    padding: 0.75rem;
    overflow-y: auto;
    border-right: 1px solid var(--uc-border);
    background: var(--uc-surface-alt);
    font-size: 0.9rem;
  }
  .account {
    display: flex;
    align-items: baseline;
    gap: 0.25rem;
  }
  .account h3 {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .small {
    min-height: 0;
    padding: 0 0.4rem;
    font-size: 0.7rem;
  }
  .sync {
    margin: 0 0 0.3rem;
    font-size: 0.75rem;
    color: var(--uc-text-muted);
  }
  .sync.warn {
    color: #b7791f;
  }
  .sync.error {
    color: var(--uc-danger);
  }
  .accounts-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
  }
  h3 {
    margin: 0 0 0.35rem;
    font-size: 0.75rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--uc-text-muted);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  li {
    display: flex;
    align-items: center;
    gap: 0.25rem;
  }
  .cal {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.2rem 0;
    cursor: pointer;
    min-width: 0;
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .badge {
    font-size: 0.65rem;
    padding: 0 0.3rem;
    border-radius: 4px;
    border: 1px solid var(--uc-border);
    color: var(--uc-text-muted);
  }
  .swatch {
    width: 1.4rem;
    height: 1.4rem;
    padding: 0;
    border: 0;
    background: none;
    cursor: pointer;
  }
  .settings label {
    display: grid;
    gap: 0.2rem;
    margin-bottom: 0.5rem;
  }
  .settings label span {
    color: var(--uc-text-muted);
    font-size: 0.8rem;
  }
</style>
