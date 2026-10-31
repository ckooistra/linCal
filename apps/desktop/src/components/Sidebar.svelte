<!--
  The main sidebar: a calendar-group selector and the calendars chosen for
  the sidebar (in Settings), each with a toggle for its events. Everything
  else lives in Settings; account problems show as one line at the bottom.
-->
<script lang="ts">
  import { accountsNeedingAttention } from "../services/accountStatus";
  import type { CalendarsStore } from "../stores/calendars.svelte";
  import type { GroupSelection, GroupsStore } from "../stores/groups.svelte";
  import type { CalendarId } from "../types/calendar";

  interface Props {
    calendars: CalendarsStore;
    groups: GroupsStore;
    /** The group whose calendars are shown, "all", or "custom". */
    selection: GroupSelection;
    onselectgroup: (selection: "all" | string) => void;
    ontogglecalendar: (id: CalendarId) => void;
    /** Opens Settings (optionally at a tab). */
    onsettings: (tab?: "calendars" | "accounts") => void;
  }

  let { calendars, groups, selection, onselectgroup, ontogglecalendar, onsettings }: Props = $props();

  const attention = $derived(accountsNeedingAttention(calendars.statuses).length);
  const syncing = $derived(calendars.syncing.size > 0);
</script>

<nav class="sidebar" aria-label="Calendars">
  <label class="group">
    <span class="sr-only">Calendar group</span>
    <select
      value={selection}
      onchange={(e) => {
        const value = e.currentTarget.value;
        if (value !== "custom") onselectgroup(value);
      }}
    >
      <option value="all">All calendars</option>
      {#each groups.groups as g (g.id)}
        <option value={g.id}>{g.name}</option>
      {/each}
      {#if selection === "custom"}
        <option value="custom" disabled>Custom selection</option>
      {/if}
    </select>
  </label>

  {#if calendars.sidebarCalendars.length > 0}
    <ul aria-label="Calendars in the sidebar">
      {#each calendars.sidebarCalendars as cal (cal.id)}
        <li>
          <label class="cal" title={cal.readOnly ? `${cal.name} (read-only)` : cal.name}>
            <span class="dot" style:background={cal.color} aria-hidden="true"></span>
            <span class="name">{cal.name}</span>
            <input
              type="checkbox"
              checked={!calendars.hidden.has(cal.id)}
              onchange={() => ontogglecalendar(cal.id)}
              aria-label={`Show events of ${cal.name}`}
              style:accent-color={cal.color}
            />
          </label>
        </li>
      {/each}
    </ul>
  {:else}
    <p class="empty">
      No calendars in the sidebar.
      <button type="button" class="link" onclick={() => onsettings("calendars")}>Choose some in Settings</button>
    </p>
  {/if}

  <div class="footer">
    {#if attention > 0}
      <button type="button" class="attention" onclick={() => onsettings("accounts")}>
        <span aria-hidden="true">⚠</span>
        {attention === 1 ? "1 account needs attention" : `${attention} accounts need attention`}
      </button>
    {/if}
    <button type="button" class="settings" onclick={() => onsettings()} title="Settings (Ctrl+,)">
      <span aria-hidden="true">⚙</span> Settings
      {#if syncing}<span class="syncing" role="status">Syncing…</span>{/if}
    </button>
  </div>
</nav>

<style>
  .sidebar {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    padding: 0.75rem 0.6rem;
    overflow-y: auto;
    border-right: 1px solid var(--uc-border);
    background: var(--uc-surface-alt);
    font-size: 0.9rem;
  }
  .group select {
    width: 100%;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 0.05rem;
  }
  .cal {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.25rem 0.3rem;
    border-radius: 6px;
    cursor: pointer;
    min-width: 0;
  }
  .cal:hover {
    background: var(--uc-surface);
  }
  .dot {
    flex: none;
    width: 0.7rem;
    height: 0.7rem;
    border-radius: 50%;
  }
  .name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .empty {
    margin: 0.25rem 0.3rem;
    color: var(--uc-text-muted);
    font-size: 0.85rem;
  }
  .link {
    display: inline;
    min-height: 0;
    border: none;
    background: none;
    padding: 0;
    color: var(--uc-accent);
    text-decoration: underline;
    font: inherit;
  }
  .footer {
    margin-top: auto;
    display: grid;
    gap: 0.3rem;
  }
  .footer button {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    width: 100%;
    border: none;
    background: transparent;
    text-align: left;
  }
  .footer button:hover {
    background: var(--uc-surface);
  }
  .attention {
    color: var(--uc-danger);
    font-size: 0.85rem;
  }
  .syncing {
    margin-left: auto;
    font-size: 0.75rem;
    color: var(--uc-text-muted);
  }
</style>
