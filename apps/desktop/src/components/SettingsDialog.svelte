<!--
  Settings, in one dialog with tabs: calendars (sidebar, events, colours),
  calendar groups, accounts, appearance and general preferences. Changes
  apply at once; the parent persists them.
-->
<script lang="ts" module>
  export type SettingsTab = "calendars" | "groups" | "accounts" | "appearance" | "general";
</script>

<script lang="ts">
  import { onMount, tick, untrack } from "svelte";
  import { accountSummary } from "../services/accountStatus";
  import { modal } from "../services/modal";
  import { DATASET_SIZES } from "../services/mock/generator";
  import type { ThemePreference } from "../services/theme";
  import type { CalendarsStore } from "../stores/calendars.svelte";
  import type { CalendarGroup, GroupsStore } from "../stores/groups.svelte";
  import type { CalendarId, ProviderKind } from "../types/calendar";

  type AccountKind = "caldav" | "google" | "microsoft" | "ics";

  interface Props {
    calendars: CalendarsStore;
    groups: GroupsStore;
    initialTab?: SettingsTab;
    themePreference: ThemePreference;
    ontheme: (preference: ThemePreference) => void;
    /** Desktop only: native window frame instead of the app's title bar. */
    systemTitleBar?: boolean | undefined;
    onsystemtitlebar?: (on: boolean) => void;
    /** Desktop only: reminder notifications. */
    notifications?: boolean | undefined;
    onnotifications?: (on: boolean) => void;
    /** Show or hide a calendar's events. */
    ontogglecalendar: (id: CalendarId) => void;
    /** Show or hide a calendar in the sidebar. */
    onsidebar: (id: CalendarId, on: boolean) => void;
    oncolor: (id: CalendarId, color: string) => void;
    /** The calendar new events go into, and how to change it. */
    defaultCalendarId?: CalendarId | null;
    ondefaultcalendar?: (id: CalendarId) => void;
    /** The groups changed (to be saved). */
    ongroupschange: () => void;
    /** Account actions (desktop only). */
    onaddaccount?: (kind: AccountKind) => void;
    onremoveaccount?: (accountId: string) => void;
    onsignin?: (accountId: string) => void;
    onsyncnow?: () => void;
    onreviewproblems?: () => void;
    onshortcuts: () => void;
    /** Browser preview only: simulated data controls. */
    simulation?: {
      datasetSize: number;
      latencyMs: number;
      ondataset: (size: number) => void;
      onlatency: (ms: number) => void;
      onfailnext: () => void;
    } | null;
    onclose: () => void;
  }

  let {
    calendars,
    groups,
    initialTab = "calendars",
    themePreference,
    ontheme,
    systemTitleBar,
    onsystemtitlebar,
    notifications,
    onnotifications,
    ontogglecalendar,
    onsidebar,
    oncolor,
    defaultCalendarId = null,
    ondefaultcalendar,
    ongroupschange,
    onaddaccount,
    onremoveaccount,
    onsignin,
    onsyncnow,
    onreviewproblems,
    onshortcuts,
    simulation = null,
    onclose,
  }: Props = $props();

  const TABS: { id: SettingsTab; label: string }[] = [
    { id: "calendars", label: "Calendars" },
    { id: "groups", label: "Calendar groups" },
    { id: "accounts", label: "Accounts" },
    { id: "appearance", label: "Appearance" },
    { id: "general", label: "General" },
  ];
  let tab = $state<SettingsTab>(untrack(() => initialTab));
  let tablist: HTMLDivElement;

  /** Arrow keys, Home and End move between tabs (and select them). */
  function onTabKey(e: KeyboardEvent): void {
    const i = TABS.findIndex((t) => t.id === tab);
    const next =
      e.key === "ArrowDown" || e.key === "ArrowRight"
        ? (i + 1) % TABS.length
        : e.key === "ArrowUp" || e.key === "ArrowLeft"
          ? (i - 1 + TABS.length) % TABS.length
          : e.key === "Home"
            ? 0
            : e.key === "End"
              ? TABS.length - 1
              : -1;
    if (next < 0) return;
    e.preventDefault();
    tab = TABS[next]!.id;
    void tick().then(() => tablist.querySelector<HTMLElement>(`#settings-tab-${tab}`)?.focus());
  }

  const PROVIDERS: Partial<Record<ProviderKind, string>> = {
    local: "On this computer",
    caldav: "CalDAV",
    google: "Google",
    microsoft: "Microsoft",
    ics: "Subscription",
    simulated: "Demo data",
  };

  const byAccount = $derived(
    calendars.accounts
      .map((account) => ({ account, items: calendars.calendars.filter((c) => c.accountId === account.id) }))
      .filter((a) => a.items.length > 0),
  );

  // --- groups ----------------------------------------------------------------
  interface Draft {
    id: string | null;
    name: string;
    color: string;
    members: CalendarId[];
  }
  let draft = $state<Draft | null>(null);
  let draftError = $state<string | null>(null);
  let deleting = $state<CalendarGroup | null>(null);
  let nameInput = $state<HTMLInputElement>();
  let panel: HTMLDivElement;

  /**
   * After the group editor or a delete question closes, the control that
   * had focus is gone: focus the group's Edit button, or "New group…".
   */
  async function refocus(groupId: string | null = null): Promise<void> {
    await tick();
    const target =
      (groupId && panel.querySelector<HTMLElement>(`[data-edit="${groupId}"]`)) ||
      panel.querySelector<HTMLElement>("button.new");
    target?.focus();
  }

  function closeEditor(): void {
    const id = draft?.id ?? null;
    draft = null;
    void refocus(id);
  }

  async function editGroup(group: CalendarGroup | null): Promise<void> {
    deleting = null;
    draftError = null;
    draft = group
      ? { id: group.id, name: group.name, color: group.color, members: [...group.calendarIds] }
      : { id: null, name: "", color: "#3b5bdb", members: calendars.calendars.filter((c) => !calendars.hidden.has(c.id)).map((c) => c.id) };
    await tick();
    nameInput?.focus();
  }

  function saveGroup(e: SubmitEvent): void {
    e.preventDefault();
    if (!draft) return;
    const name = draft.name.trim();
    if (!name) {
      draftError = "Give the group a name.";
      return;
    }
    if (groups.groups.some((g) => g.id !== draft!.id && g.name.toLowerCase() === name.toLowerCase())) {
      draftError = "A group with this name exists already.";
      return;
    }
    let id: string;
    if (draft.id) {
      groups.update(draft.id, { name, color: draft.color, calendarIds: draft.members });
      id = draft.id;
    } else {
      id = groups.add(name, draft.color, draft.members).id;
    }
    draft = null;
    ongroupschange();
    void refocus(id);
  }

  function toggleMember(id: CalendarId): void {
    if (!draft) return;
    draft.members = draft.members.includes(id) ? draft.members.filter((m) => m !== id) : [...draft.members, id];
  }

  function confirmDelete(): void {
    if (!deleting) return;
    groups.remove(deleting.id);
    deleting = null;
    ongroupschange();
    void refocus();
  }

  function onDialogKey(e: KeyboardEvent): void {
    if (e.key !== "Escape") return;
    e.preventDefault();
    e.stopPropagation();
    // Escape first leaves a group being edited or deleted, then the dialog.
    if (draft) {
      closeEditor();
    } else if (deleting) {
      const id = deleting.id;
      deleting = null;
      void refocus(id);
    } else {
      onclose();
    }
  }

  onMount(() => {
    tablist.querySelector<HTMLElement>(`#settings-tab-${tab}`)?.focus();
  });
</script>

<div class="backdrop" role="presentation" onmousedown={(e) => e.target === e.currentTarget && onclose()}>
  <div
    class="dialog"
    role="dialog"
    aria-modal="true"
    aria-labelledby="settings-title"
    tabindex="-1"
    use:modal
    onkeydown={onDialogKey}
  >
    <header>
      <h2 id="settings-title">Settings</h2>
      <button type="button" class="close" aria-label="Close settings" title="Close (Esc)" onclick={onclose}>✕</button>
    </header>
    <div class="body">
      <div class="tabs" role="tablist" aria-label="Settings sections" aria-orientation="vertical" bind:this={tablist}>
        {#each TABS as t (t.id)}
          <button
            type="button"
            role="tab"
            id={`settings-tab-${t.id}`}
            aria-selected={tab === t.id}
            aria-controls={`settings-panel-${t.id}`}
            tabindex={tab === t.id ? 0 : -1}
            onclick={() => (tab = t.id)}
            onkeydown={onTabKey}>{t.label}</button
          >
        {/each}
      </div>

      <div class="panel" role="tabpanel" bind:this={panel} id={`settings-panel-${tab}`} aria-labelledby={`settings-tab-${tab}`}>
        {#if tab === "calendars"}
          <p class="hint">
            Choose which calendars appear in the sidebar, and whose events are shown. A calendar left out of the sidebar
            stays connected.
          </p>
          {#if byAccount.length === 0}
            <p class="empty">No calendars yet. Add an account under Accounts.</p>
          {/if}
          {#if ondefaultcalendar && calendars.writable.length > 0}
            <label class="field">
              <span>New events go to</span>
              <select value={defaultCalendarId ?? ""} onchange={(e) => ondefaultcalendar(e.currentTarget.value)}>
                {#each byAccount as { account, items } (account.id)}
                  {@const writable = items.filter((c) => !c.readOnly)}
                  {#if writable.length > 0}
                    <optgroup label={account.name}>
                      {#each writable as cal (cal.id)}
                        <option value={cal.id}>{cal.name}</option>
                      {/each}
                    </optgroup>
                  {/if}
                {/each}
              </select>
            </label>
          {/if}
          {#each byAccount as { account, items } (account.id)}
            <section class="group-of-calendars" aria-labelledby={`settings-account-${account.id}`}>
              <h3 id={`settings-account-${account.id}`}>{account.name}</h3>
              <table class="calendars">
                <thead>
                  <tr>
                    <th scope="col"><span class="sr-only">Colour</span></th>
                    <th scope="col">Calendar</th>
                    <th scope="col">In sidebar</th>
                    <th scope="col">Show events</th>
                  </tr>
                </thead>
                <tbody>
                  {#each items as cal (cal.id)}
                    <tr>
                      <td>
                        <input
                          class="swatch"
                          type="color"
                          value={cal.color}
                          aria-label={`Colour of ${cal.name}`}
                          onchange={(e) => oncolor(cal.id, e.currentTarget.value)}
                        />
                      </td>
                      <th scope="row" class="name">
                        {cal.name}
                        {#if cal.readOnly}<span class="badge">read-only</span>{/if}
                      </th>
                      <td>
                        <input
                          type="checkbox"
                          checked={calendars.inSidebar(cal.id)}
                          aria-label={`Show ${cal.name} in the sidebar`}
                          onchange={(e) => onsidebar(cal.id, e.currentTarget.checked)}
                        />
                      </td>
                      <td>
                        <input
                          type="checkbox"
                          checked={!calendars.hidden.has(cal.id)}
                          aria-label={`Show events of ${cal.name}`}
                          onchange={() => ontogglecalendar(cal.id)}
                        />
                      </td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            </section>
          {/each}
        {:else if tab === "groups"}
          <p class="hint">
            A group shows the events of its calendars together; choose one at the top of the sidebar. A calendar can be
            in several groups. Calendar colours stay as they are.
          </p>
          {#if draft}
            <form class="editor" onsubmit={saveGroup} novalidate aria-labelledby="group-editor-title">
              <h3 id="group-editor-title">{draft.id ? "Edit group" : "New group"}</h3>
              <div class="row">
                <label class="grow">
                  <span>Name</span>
                  <input bind:this={nameInput} bind:value={draft.name} placeholder="Work" autocomplete="off" />
                </label>
                <label>
                  <span>Colour</span>
                  <input class="swatch" type="color" bind:value={draft.color} />
                </label>
              </div>
              <fieldset>
                <legend>Calendars</legend>
                {#each calendars.calendars as cal (cal.id)}
                  <label class="check">
                    <input type="checkbox" checked={draft.members.includes(cal.id)} onchange={() => toggleMember(cal.id)} />
                    <span class="dot" style:background={cal.color} aria-hidden="true"></span>
                    <span>{calendars.label(cal.id)}</span>
                  </label>
                {/each}
              </fieldset>
              {#if draftError}<p class="error" role="alert">{draftError}</p>{/if}
              <div class="actions">
                <span class="spacer"></span>
                <button type="button" onclick={closeEditor}>Cancel</button>
                <button type="submit" class="primary">{draft.id ? "Save group" : "Create group"}</button>
              </div>
            </form>
          {:else}
            {#if groups.groups.length === 0}
              <p class="empty">No groups yet.</p>
            {:else}
              <ul class="groups">
                {#each groups.groups as g (g.id)}
                  {@const count = g.calendarIds.filter((id) => calendars.get(id)).length}
                  <li>
                    <span class="dot" style:background={g.color} aria-hidden="true"></span>
                    <span class="grow">
                      <span class="group-name">{g.name}</span>
                      <span class="meta">{count === 1 ? "1 calendar" : `${count} calendars`}</span>
                    </span>
                    {#if deleting?.id === g.id}
                      <span class="confirm" role="alert">Delete “{g.name}”?</span>
                      <button
                        type="button"
                        onclick={() => {
                          deleting = null;
                          void refocus(g.id);
                        }}>Keep</button
                      >
                      <button type="button" class="danger" onclick={confirmDelete}>Delete</button>
                    {:else}
                      <button type="button" data-edit={g.id} aria-label={`Edit ${g.name}`} onclick={() => void editGroup(g)}>Edit</button>
                      <button type="button" class="danger" aria-label={`Delete ${g.name}`} onclick={() => (deleting = g)}
                        >Delete</button
                      >
                    {/if}
                  </li>
                {/each}
              </ul>
            {/if}
            <button type="button" class="primary new" onclick={() => void editGroup(null)}>New group…</button>
          {/if}
        {:else if tab === "accounts"}
          <ul class="accounts">
            {#each calendars.accounts as account (account.id)}
              {@const summary = accountSummary(calendars.statuses[account.id], calendars.syncing.has(account.id))}
              <li>
                <div class="grow">
                  <span class="account-name">{account.name}</span>
                  <span class="meta">{PROVIDERS[account.provider] ?? account.provider}</span>
                  {#if summary}
                    <span class={`sync ${summary.tone}`}>{summary.text}</span>
                  {/if}
                </div>
                <div class="account-actions">
                  {#if summary?.review && onreviewproblems}
                    <button type="button" onclick={onreviewproblems}>Review…</button>
                  {/if}
                  {#if summary?.signIn && onsignin}
                    <button type="button" onclick={() => onsignin(account.id)}>Sign in again…</button>
                  {/if}
                  {#if onremoveaccount && account.provider !== "local" && account.provider !== "simulated"}
                    <button type="button" class="danger" aria-label={`Remove ${account.name}`} onclick={() => onremoveaccount(account.id)}
                      >Remove</button
                    >
                  {/if}
                </div>
              </li>
            {/each}
          </ul>
          {#if onaddaccount}
            <div class="add">
              <button type="button" onclick={() => onaddaccount("caldav")}>Add CalDAV account…</button>
              <button type="button" onclick={() => onaddaccount("google")}>Add Google account…</button>
              <button type="button" onclick={() => onaddaccount("microsoft")}>Add Microsoft account…</button>
              <button type="button" onclick={() => onaddaccount("ics")}>Subscribe to a calendar…</button>
            </div>
          {:else}
            <p class="hint">Accounts can be added in the desktop app.</p>
          {/if}
          {#if onsyncnow && Object.values(calendars.statuses).some((s) => s.remote)}
            <button type="button" class="sync-now" onclick={onsyncnow}>Sync all accounts now</button>
          {/if}
        {:else if tab === "appearance"}
          <label class="field">
            <span>Theme</span>
            <select value={themePreference} onchange={(e) => ontheme(e.currentTarget.value as ThemePreference)}>
              <option value="system">System</option>
              <option value="light">Light</option>
              <option value="dark">Dark</option>
            </select>
          </label>
          {#if onsystemtitlebar && systemTitleBar !== undefined}
            <label class="check">
              <input type="checkbox" checked={systemTitleBar} onchange={(e) => onsystemtitlebar(e.currentTarget.checked)} />
              <span>Use the desktop's title bar</span>
            </label>
          {/if}
        {:else}
          {#if onnotifications && notifications !== undefined}
            <label class="check">
              <input type="checkbox" checked={notifications} onchange={(e) => onnotifications(e.currentTarget.checked)} />
              <span>Reminder notifications</span>
            </label>
          {/if}
          <button type="button" onclick={onshortcuts}>Keyboard shortcuts…</button>
          {#if simulation}
            <h3>Simulated data (preview)</h3>
            <label class="field">
              <span>Dataset (replaces simulated events only)</span>
              <select value={simulation.datasetSize} onchange={(e) => simulation.ondataset(Number(e.currentTarget.value))}>
                {#each DATASET_SIZES as size (size)}
                  <option value={size}>{size.toLocaleString()} events</option>
                {/each}
              </select>
            </label>
            <label class="field">
              <span>Storage latency</span>
              <select value={simulation.latencyMs} onchange={(e) => simulation.onlatency(Number(e.currentTarget.value))}>
                <option value={0}>None</option>
                <option value={50}>50 ms</option>
                <option value={300}>300 ms</option>
                <option value={1500}>1.5 s</option>
              </select>
            </label>
            <button type="button" onclick={simulation.onfailnext}>Fail next save</button>
          {/if}
        {/if}
      </div>
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
    /* Below the dialogs opened from here (accounts, confirmations). */
    z-index: 45;
  }
  .dialog {
    display: flex;
    flex-direction: column;
    width: min(46rem, calc(100vw - 2rem));
    height: min(36rem, calc(100vh - 2rem));
    background: var(--uc-surface);
    color: var(--uc-text);
    border: 1px solid var(--uc-border);
    border-radius: 10px;
    box-shadow: 0 12px 40px rgb(0 0 0 / 0.3);
    outline: none;
    overflow: hidden;
  }
  header {
    display: flex;
    align-items: center;
    padding: 0.75rem 1rem;
    border-bottom: 1px solid var(--uc-border);
  }
  h2 {
    flex: 1;
    margin: 0;
    font-size: 1.1rem;
  }
  .close {
    border: none;
    background: transparent;
  }
  .body {
    display: grid;
    grid-template-columns: 11rem 1fr;
    flex: 1;
    min-height: 0;
  }
  .tabs {
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
    padding: 0.6rem;
    border-right: 1px solid var(--uc-border);
    background: var(--uc-surface-alt);
  }
  .tabs button {
    border: none;
    background: transparent;
    text-align: left;
  }
  .tabs button[aria-selected="true"] {
    background: var(--uc-accent);
    color: var(--uc-accent-text);
  }
  .panel {
    overflow-y: auto;
    padding: 0.9rem 1rem 1.2rem;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    align-items: flex-start;
  }
  .panel > * {
    max-width: 100%;
  }
  h3 {
    margin: 0.25rem 0 0.25rem;
    font-size: 0.9rem;
  }
  .hint,
  .meta,
  .empty {
    margin: 0;
    color: var(--uc-text-muted);
    font-size: 0.85rem;
  }
  .group-of-calendars {
    width: 100%;
  }
  table.calendars {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.9rem;
  }
  .calendars th,
  .calendars td {
    padding: 0.2rem 0.4rem;
    text-align: left;
    border-top: 1px solid var(--uc-border);
  }
  .calendars thead th {
    border-top: none;
    font-size: 0.75rem;
    font-weight: normal;
    color: var(--uc-text-muted);
  }
  .calendars td:nth-child(3),
  .calendars td:nth-child(4),
  .calendars thead th:nth-child(3),
  .calendars thead th:nth-child(4) {
    text-align: center;
    width: 6rem;
  }
  .calendars .name {
    font-weight: normal;
  }
  .badge {
    margin-left: 0.4rem;
    font-size: 0.7rem;
    color: var(--uc-text-muted);
  }
  .swatch {
    width: 1.8rem;
    height: 1.4rem;
    padding: 0;
    border: none;
    background: none;
  }
  .dot {
    flex: none;
    width: 0.7rem;
    height: 0.7rem;
    border-radius: 50%;
  }
  .groups,
  .accounts {
    list-style: none;
    margin: 0;
    padding: 0;
    width: 100%;
  }
  .groups li,
  .accounts li {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.5rem;
    padding: 0.5rem 0;
    border-top: 1px solid var(--uc-border);
  }
  .grow {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .group-name,
  .account-name {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .confirm {
    font-size: 0.85rem;
  }
  .sync {
    font-size: 0.8rem;
    color: var(--uc-text-muted);
  }
  .sync.warn {
    color: var(--uc-text);
  }
  .sync.error {
    color: var(--uc-danger);
  }
  .account-actions,
  .add {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
  }
  .editor {
    width: 100%;
    display: grid;
    gap: 0.6rem;
  }
  .row {
    display: flex;
    gap: 0.75rem;
    align-items: end;
  }
  .row label,
  .field {
    display: grid;
    gap: 0.25rem;
    font-size: 0.85rem;
  }
  .row label > span,
  .field > span {
    color: var(--uc-text-muted);
  }
  .row .grow {
    display: grid;
  }
  fieldset {
    border: 1px solid var(--uc-border);
    border-radius: 6px;
    padding: 0.5rem 0.75rem;
    margin: 0;
    display: grid;
    gap: 0.3rem;
    max-height: 14rem;
    overflow-y: auto;
  }
  legend {
    font-size: 0.85rem;
    color: var(--uc-text-muted);
  }
  .check {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }
  .error {
    margin: 0;
    color: var(--uc-danger);
  }
  .actions {
    display: flex;
    gap: 0.5rem;
  }
  .spacer {
    flex: 1;
  }
  @media (max-width: 640px) {
    .body {
      grid-template-columns: 1fr;
      grid-template-rows: auto 1fr;
    }
    .tabs {
      flex-direction: row;
      flex-wrap: wrap;
      border-right: none;
      border-bottom: 1px solid var(--uc-border);
    }
  }
</style>
