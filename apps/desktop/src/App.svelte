<script lang="ts">
  import { onDestroy, onMount, tick, untrack } from "svelte";
  import CalendarAdapter from "./components/calendar/CalendarAdapter.svelte";
  import type {
    CalendarViewController,
    CalendarViewState,
    EditorRequest,
    MoveRequest,
    DayHours,
  } from "./components/calendar/types";
  import { parseDayHours } from "./components/calendar/types";
  import AddAccountDialog from "./components/AddAccountDialog.svelte";
  import AgendaView from "./components/AgendaView.svelte";
  import CommandPalette from "./components/CommandPalette.svelte";
  import SettingsDialog, { type SettingsTab } from "./components/SettingsDialog.svelte";
  import OAuthAccountDialog from "./components/OAuthAccountDialog.svelte";
  import ConfirmDialog from "./components/ConfirmDialog.svelte";
  import EventEditor from "./components/EventEditor.svelte";
  import PerfPanel from "./components/PerfPanel.svelte";
  import SearchResults from "./components/SearchResults.svelte";
  import ShortcutsDialog from "./components/ShortcutsDialog.svelte";
  import SubscriptionDialog from "./components/SubscriptionDialog.svelte";
  import Sidebar from "./components/Sidebar.svelte";
  import SyncProblemsDialog from "./components/SyncProblemsDialog.svelte";
  import Toasts from "./components/Toasts.svelte";
  import TitleBar from "./components/TitleBar.svelte";
  import Toolbar from "./components/Toolbar.svelte";
  import { runBenchmarks, type BenchHarness } from "./bench/runner";
  import type { Backend } from "./services/backend";
  import { getBenchConfig, setWindowTheme, submitBenchReport, windowControls } from "./services/bridge";
  import { measureInteraction, nextPaint, observeLongTasks } from "./services/perf";
  import { resolveShortcut } from "./services/shortcuts";
  import { parseThemePreference, resolveTheme, watchSystemDark, type ThemePreference } from "./services/theme";
  import { HOUR, addDays, eventBounds, localTimeZone, parseIsoDate } from "./services/time";
  import { CalendarsStore } from "./stores/calendars.svelte";
  import { GroupsStore, parseGroups, selectionFor } from "./stores/groups.svelte";
  import type { Command } from "./services/commands";
  import { EventStore, type SeriesScope } from "./stores/events.svelte";
  import { Notifications } from "./stores/notifications.svelte";
  import type {
    AppView,
    CalendarEvent,
    CalendarViewKind,
    EventDraft,
    EventTiming,
    Resolution,
    SyncEvent,
    SyncProblem,
  } from "./types/calendar";

  const BENCH_ANCHOR = "2026-06-15";
  const SEARCH_DEBOUNCE_MS = 150;

  let { backend }: { backend: Backend } = $props();

  // --- data -----------------------------------------------------------------
  const notifications = new Notifications();
  const calendars = new CalendarsStore();
  const initialSettings = untrack(() => backend.settings);
  let datasetSize = $state(typeof initialSettings.datasetSize === "number" ? initialSettings.datasetSize : 1000);
  let latencyMs = $state(0);
  let startupError = $state(untrack(() => backend.startupError));
  const store = new EventStore(untrack(() => backend.source), {
    isReadOnly: (id) => calendars.isReadOnly(id),
    onError: (message) => notifications.push(message),
  });

  async function reloadCalendars(): Promise<void> {
    try {
      const { accounts, calendars: list } = await backend.loadCalendars();
      calendars.load(accounts, list);
    } catch (error) {
      startupError = error instanceof Error ? error.message : String(error);
    }
  }

  let datasetBusy = false;
  async function switchDataset(size: number, anchor?: string): Promise<void> {
    if (datasetBusy) return;
    datasetBusy = true;
    try {
      const source = await backend.loadDataset(size, anchor);
      datasetSize = size;
      void backend.saveSetting("datasetSize", size);
      await reloadCalendars();
      store.reset(source);
    } catch (error) {
      notifications.push(error instanceof Error ? error.message : "The dataset could not be loaded.");
    } finally {
      datasetBusy = false;
    }
  }

  function setCalendarColor(id: string, color: string): void {
    const previous = calendars.get(id)?.color;
    calendars.setColor(id, color);
    backend.setCalendarColor(id, color).catch((error: unknown) => {
      if (previous) calendars.setColor(id, previous);
      notifications.push(error instanceof Error ? error.message : "The colour could not be saved.");
    });
  }

  // --- accounts and sync ---------------------------------------------------
  /** Which "add account" dialog is open. */
  let addingAccount = $state<false | "caldav" | "google" | "microsoft" | "ics">(false);
  let builtInClientId = $state<string | null>(null);
  let removing = $state<{ id: string; name: string } | null>(null);
  let statusTimer: ReturnType<typeof setTimeout> | undefined;

  function refreshStatuses(): void {
    backend
      .accountStatuses()
      .then((list) => calendars.setStatuses(list))
      .catch(() => {});
  }

  /** Coalesces status refreshes after bursts of local edits. */
  function refreshStatusesSoon(): void {
    clearTimeout(statusTimer);
    statusTimer = setTimeout(refreshStatuses, 400);
  }

  async function addAccount(serverUrl: string, username: string, password: string): Promise<string | null> {
    try {
      await backend.addCaldavAccount(serverUrl, username, password);
      addingAccount = false;
      await reloadCalendars();
      refreshStatuses();
      notifications.push("Account added. Syncing…", "info");
      return null;
    } catch (error) {
      if (!addingAccount) return null; // cancelled: the dialog is gone
      return error instanceof Error ? error.message : "The account could not be added.";
    }
  }

  async function openAddAccount(kind: "caldav" | "google" | "microsoft" | "ics"): Promise<void> {
    builtInClientId =
      kind === "caldav" || kind === "ics" ? null : await backend.oauthClientId(kind).catch(() => null);
    addingAccount = kind;
  }

  async function subscribe(address: string): Promise<string | null> {
    try {
      await backend.addIcsSubscription(address);
      addingAccount = false;
      await reloadCalendars();
      refreshStatuses();
      notifications.push("Subscribed. Loading events…", "info");
      return null;
    } catch (error) {
      if (!addingAccount) return null; // cancelled: the dialog is gone
      return error instanceof Error ? error.message : "The calendar could not be subscribed to.";
    }
  }

  async function addOAuth(clientId: string, clientSecret: string | null): Promise<string | null> {
    const microsoft = addingAccount === "microsoft";
    try {
      if (microsoft) await backend.addMicrosoftAccount(clientId);
      else await backend.addGoogleAccount(clientId, clientSecret);
      addingAccount = false;
      await reloadCalendars();
      refreshStatuses();
      notifications.push(`${microsoft ? "Microsoft" : "Google"} account added. Syncing…`, "info");
      return null;
    } catch (error) {
      if (!addingAccount) return null; // cancelled: the dialog is gone
      return error instanceof Error ? error.message : "The account could not be added.";
    }
  }

  async function signInAgain(accountId: string): Promise<void> {
    notifications.push("Finish signing in in your web browser…", "info");
    try {
      await backend.signInAgain(accountId);
      refreshStatuses();
    } catch (error) {
      notifications.push(error instanceof Error ? error.message : "Signing in failed.");
    }
  }

  async function confirmRemoveAccount(): Promise<void> {
    if (!removing) return;
    const { id } = removing;
    removing = null;
    try {
      await backend.removeAccount(id);
      await reloadCalendars();
      store.invalidate();
      refreshStatuses();
    } catch (error) {
      notifications.push(error instanceof Error ? error.message : "The account could not be removed.");
    }
  }

  function onSync(event: SyncEvent): void {
    switch (event.type) {
      case "syncing":
        calendars.setSyncing(event.accountId, true);
        break;
      case "synced":
        calendars.setSyncing(event.accountId, false);
        refreshStatuses();
        if (event.changed) {
          // New or changed server data: reload calendars and the visible range.
          void reloadCalendars();
          store.invalidate();
        }
        break;
      case "failed":
        calendars.setSyncing(event.accountId, false);
        refreshStatuses();
        // A pass can upload or fail some changes before it stops: refresh
        // the sync status shown on events.
        store.invalidate();
        break;
    }
    if (problems !== null) void loadProblems();
  }

  async function resolveFromEditor(resolution: Resolution): Promise<void> {
    const event = editor?.event;
    if (!event) return;
    editor = null;
    try {
      await backend.resolveConflict(event.id, resolution);
      store.invalidate();
      refreshStatuses();
    } catch (error) {
      notifications.push(error instanceof Error ? error.message : "The conflict could not be resolved.");
    }
  }

  /** Failed/conflicting changes while the review dialog is open (null = closed). */
  let problems = $state<SyncProblem[] | null>(null);
  let reviewing = $state(false);

  async function loadProblems(): Promise<void> {
    try {
      problems = await backend.syncProblems();
    } catch (error) {
      notifications.push(error instanceof Error ? error.message : "Sync problems could not be loaded.");
    }
  }

  async function resolveProblem(problem: SyncProblem, resolution: Resolution): Promise<void> {
    try {
      await backend.resolveSyncProblem(problem.calendarId, problem.href, resolution);
      store.invalidate();
      refreshStatuses();
      await loadProblems();
    } catch (error) {
      notifications.push(error instanceof Error ? error.message : "The change could not be resolved.");
    }
  }

  // Display preferences are per device: applied at once, saved shortly
  // after (bursts of changes coalesce). A failed save keeps the change in
  // memory and says so.
  const pendingSaves = new Map<string, { timer: ReturnType<typeof setTimeout>; save: () => void }>();
  function persist(key: string, value: () => unknown, what: string): void {
    clearTimeout(pendingSaves.get(key)?.timer);
    const save = () => {
      pendingSaves.delete(key);
      backend.saveSetting(key, value()).catch(() => {
        notifications.push(`Your ${what} could not be saved; it applies until the app is closed.`);
      });
    };
    pendingSaves.set(key, { timer: setTimeout(save, 300), save });
  }

  /** Saves what is still waiting (the app is closing). */
  function flushSaves(): void {
    for (const { timer, save } of [...pendingSaves.values()]) {
      clearTimeout(timer);
      save();
    }
  }

  const saveVisibility = () => persist("calendarVisibility", () => calendars.choices(), "calendar visibility");

  function toggleCalendar(id: string): void {
    calendars.toggle(id);
    saveVisibility();
  }

  function setInSidebar(id: string, on: boolean): void {
    calendars.setInSidebar(id, on);
    persist("sidebarCalendars", () => calendars.sidebarChoices(), "sidebar choice");
  }

  // --- calendar groups --------------------------------------------------------
  const groups = new GroupsStore();
  groups.load(parseGroups(untrack(() => backend.settings.calendarGroups)));
  const saveGroups = () => persist("calendarGroups", () => groups.groups, "calendar groups");

  /** The group whose calendars are exactly the ones shown ("all"/"custom" otherwise). */
  const selection = $derived(
    selectionFor(
      groups.groups,
      calendars.calendars.map((c) => c.id),
      calendars.hidden,
    ),
  );

  function selectGroup(id: "all" | string): void {
    void measureInteraction("activate-group", () => {
      const ids = id === "all" ? calendars.calendars.map((c) => c.id) : (groups.get(id)?.calendarIds ?? []);
      calendars.setVisibleExactly(ids);
    });
    saveVisibility();
  }

  /** Saved visibility choices, migrating the older "hidden list" setting. */
  function savedVisibility(): Record<string, boolean> {
    const saved = initialSettings.calendarVisibility;
    if (saved && typeof saved === "object" && !Array.isArray(saved)) {
      return Object.fromEntries(
        Object.entries(saved as Record<string, unknown>).filter(
          (e): e is [string, boolean] => typeof e[1] === "boolean",
        ),
      );
    }
    const legacy = initialSettings.hiddenCalendars;
    return Array.isArray(legacy)
      ? Object.fromEntries(legacy.filter((id): id is string => typeof id === "string").map((id) => [id, false]))
      : {};
  }
  calendars.setChoices(savedVisibility());
  {
    const saved = initialSettings.sidebarCalendars;
    if (saved && typeof saved === "object" && !Array.isArray(saved)) {
      calendars.setSidebarChoices(
        Object.fromEntries(
          Object.entries(saved as Record<string, unknown>).filter((e): e is [string, boolean] => typeof e[1] === "boolean"),
        ),
      );
    }
  }

  // --- view -----------------------------------------------------------------
  let controller: CalendarViewController | null = null;
  let viewState = $state<CalendarViewState>({ view: "month", label: "", range: { start: 0, end: 0 }, date: Date.now() });
  let searchInput = $state<HTMLInputElement>();
  let searchText = $state("");
  let searchQuery = $state("");
  let searchTimer: ReturnType<typeof setTimeout> | undefined;

  function navigate(direction: "previous" | "next" | "today") {
    void measureInteraction(`nav-${direction}`, () => controller?.navigate(direction));
  }

  /** The agenda replaces the grid widget; these say where the grid reopens. */
  let agenda = $state(false);
  let gridView = $state<CalendarViewKind>("month");
  let viewDate = $state.raw(new Date());

  /** Where to open another view: today if it is shown, else the middle of what is shown. */
  function shownDate(): Date {
    if (viewState.view !== "month") return new Date(viewState.date);
    const { start, end } = viewState.range;
    const now = Date.now();
    if (end <= start || (now >= start && now < end)) return new Date(now);
    return new Date(start + (end - start) / 2);
  }

  function setView(view: AppView) {
    if (view === viewState.view) return;
    void measureInteraction(`view-${view}`, () => {
      if (view === "agenda") {
        viewDate = shownDate();
        agenda = true;
      } else if (agenda) {
        viewDate = shownDate();
        gridView = view;
        agenda = false;
      } else {
        controller?.setView(view);
      }
    });
  }

  /** Resolves when the results for the current query are set (benchmarks). */
  let searchShown: ((ms: number) => void) | null = null;

  function onSearchInput(text: string) {
    searchText = text;
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => {
      void measureInteraction("search", () => {
        searchQuery = text;
      });
    }, SEARCH_DEBOUNCE_MS);
  }

  function clearSearch(): void {
    clearTimeout(searchTimer);
    searchText = "";
    searchQuery = "";
  }

  /** Focus the first result once shown (after Enter in the search box). */
  let focusResults = false;

  function submitSearch(): void {
    if (!searchText.trim()) return;
    if (searchText === searchQuery && searchFocusFirst()) return;
    clearTimeout(searchTimer);
    focusResults = true;
    searchQuery = searchText;
  }

  function searchFocusFirst(): boolean {
    const first = document.querySelector<HTMLElement>(".search-overlay button.item");
    first?.focus();
    return first !== null;
  }

  /** Opens a search result: shows its date and opens it in the editor. */
  function revealEvent(event: CalendarEvent): void {
    clearSearch();
    controller?.goToDate(new Date(eventBounds(event).start));
    openEditor({ mode: "edit", id: event.id, event });
  }

  // --- theme ----------------------------------------------------------------
  let themePreference = $state<ThemePreference>(parseThemePreference(initialSettings.theme));
  let systemDark = $state(false);
  const theme = $derived(resolveTheme(themePreference, systemDark));
  $effect(() => {
    document.documentElement.dataset.theme = theme;
  });
  // The window frame follows the chosen theme; "System" leaves it to the
  // desktop (forcing it would also change what "system" detects).
  $effect(() => {
    const frame = themePreference === "system" ? null : theme;
    setWindowTheme(frame).catch(() => {});
  });

  // The app draws its own (themed) title bar unless the user prefers the
  // desktop's native frame.
  let systemTitleBar = $state(initialSettings.systemTitleBar === true);
  const customTitleBar = $derived(backend.kind === "desktop" && !systemTitleBar);
  $effect(() => {
    windowControls.setNativeFrame(systemTitleBar).catch(() => {});
  });

  // Desktop notifications for reminders (shown by the Rust side).
  let notificationsOn = $state(initialSettings.notifications !== false);
  function setNotifications(on: boolean) {
    notificationsOn = on;
    void backend.saveSetting("notifications", on);
  }

  function setSystemTitleBar(on: boolean) {
    systemTitleBar = on;
    void backend.saveSetting("systemTitleBar", on);
  }

  // Week/day grid: working hours or the full day (remembered).
  let dayHours = $state<DayHours>(parseDayHours(initialSettings.dayHours));
  function setDayHours(hours: DayHours) {
    if (hours === dayHours) return;
    void measureInteraction("toggle-hours", () => {
      dayHours = hours;
    });
    void backend.saveSetting("dayHours", hours);
  }

  function setThemePreference(preference: ThemePreference) {
    themePreference = preference;
    void backend.saveSetting("theme", preference);
  }

  // --- editor ---------------------------------------------------------------
  interface EditorState {
    key: number;
    event: CalendarEvent | null;
    timing: EventTiming;
  }
  let editor = $state.raw<EditorState | null>(null);
  let editorKey = 0;
  /** A newer stored version of the event being edited (e.g. from sync). */
  let editorNewer = $state.raw<CalendarEvent | null>(null);

  /** After the cache changed: does the open editor show an outdated event? */
  function checkEditorEvent(): void {
    const shown = editor?.event;
    if (!shown) return;
    const current = store.get(shown.id);
    if (!current || current === shown || current.syncStatus === "pending-local") return;
    const changed =
      current.title !== shown.title ||
      current.description !== shown.description ||
      current.location !== shown.location ||
      current.rrule !== shown.rrule ||
      current.syncStatus !== shown.syncStatus ||
      JSON.stringify(current.timing) !== JSON.stringify(shown.timing);
    editorNewer = changed ? current : null;
  }

  function showNewerVersion(): void {
    if (!editor || !editorNewer) return;
    editor = { key: ++editorKey, event: editorNewer, timing: editorNewer.timing };
    editorNewer = null;
  }

  function defaultTiming(): EventTiming {
    // Next full hour today, or 09:00 on the first visible day if today is off-screen.
    const now = new Date();
    const visible = viewState.range;
    let start: Date;
    if (visible.end > visible.start && (now.getTime() < visible.start || now.getTime() >= visible.end)) {
      start = new Date(visible.start);
      if (viewState.view === "month") start.setDate(start.getDate() + 7);
      start.setHours(9, 0, 0, 0);
    } else {
      start = new Date(now);
      start.setHours(now.getHours() + 1, 0, 0, 0);
    }
    return { kind: "timed", start: start.getTime(), end: start.getTime() + HOUR, timeZone: localTimeZone() };
  }

  function openEditor(request: EditorRequest) {
    editorNewer = null;
    if (request.mode === "edit") {
      const event = store.get(request.id) ?? request.event;
      if (!event) return;
      editor = { key: ++editorKey, event, timing: event.timing };
    } else {
      editor = { key: ++editorKey, event: null, timing: request.timing };
    }
  }

  function closeEditor() {
    editor = null;
  }

  function saveEditor(draft: EventDraft, scope: SeriesScope): string | null {
    if (!editor) return null;
    const result = editor.event ? store.update(editor.event.id, draft, scope) : store.create(draft);
    if (!result.ok) return result.message;
    editor = null;
    return null;
  }

  function deleteFromEditor(scope: SeriesScope) {
    if (!editor?.event) return;
    const result = store.remove(editor.event.id, scope);
    if (!result.ok) notifications.push(result.message);
    editor = null;
  }

  /** Outlook calendars hold one reminder per event. */
  function reminderLimit(calendarId: string): number {
    const accountId = calendars.get(calendarId)?.accountId;
    return calendars.accounts.find((a) => a.id === accountId)?.provider === "microsoft" ? 1 : 5;
  }

  function draftOf(event: CalendarEvent): EventDraft {
    const { calendarId, title, description, location, timing, rrule } = event;
    return { calendarId, title, description, location, timing, rrule };
  }

  function moveEvent({ id, timing }: MoveRequest) {
    const event = store.get(id);
    if (!event) return;
    const result = store.update(id, { ...draftOf(event), timing }, "occurrence");
    if (!result.ok) notifications.push(result.message);
  }

  /** The calendar chosen in Settings for new events (if it still exists and is writable). */
  let preferredCalendar = $state<string | null>(
    typeof initialSettings.defaultCalendar === "string" ? initialSettings.defaultCalendar : null,
  );
  const defaultCalendarId = $derived(
    (preferredCalendar && calendars.get(preferredCalendar) && !calendars.isReadOnly(preferredCalendar)
      ? preferredCalendar
      : null) ??
      calendars.writable.find((c) => !calendars.hidden.has(c.id))?.id ??
      calendars.writable[0]?.id ??
      null,
  );

  function setDefaultCalendar(id: string): void {
    preferredCalendar = id;
    persist("defaultCalendar", () => id, "default calendar");
  }

  // --- settings and command palette -----------------------------------------
  /** Open Settings tab, or null when closed. */
  let settingsTab = $state<SettingsTab | null>(null);
  let paletteOpen = $state(false);

  function openSettings(tab: SettingsTab = "calendars"): void {
    paletteOpen = false;
    void measureInteraction("open-settings", () => {
      settingsTab = tab;
    });
  }

  function openPalette(): void {
    void measureInteraction("open-palette", () => {
      paletteOpen = true;
    });
  }

  /** Moves the current date by a number of days or months, keeping the view. */
  function shift(days: number, months = 0): void {
    const date = new Date(viewState.date);
    if (months) date.setMonth(date.getMonth() + months);
    controller?.goToDate(addDays(date, days));
  }

  const desktop = untrack(() => backend.kind) === "desktop";
  const hasRemote = $derived(Object.values(calendars.statuses).some((s) => s.remote));

  /** Everything the palette offers; rebuilt when groups or calendars change. */
  const commands: Command[] = $derived.by(() => {
    const nav = "Navigation";
    const view = "View";
    const list: Command[] = [
      { id: "today", title: "Go to today", section: nav, shortcut: "T", run: () => navigate("today") },
      { id: "next-day", title: "Next day", section: nav, run: () => shift(1) },
      { id: "previous-day", title: "Previous day", section: nav, run: () => shift(-1) },
      { id: "next-week", title: "Next week", section: nav, run: () => shift(7) },
      { id: "previous-week", title: "Previous week", section: nav, run: () => shift(-7) },
      { id: "next-month", title: "Next month", section: nav, run: () => shift(0, 1) },
      { id: "previous-month", title: "Previous month", section: nav, run: () => shift(0, -1) },
      { id: "view-month", title: "Month view", keywords: "switch", section: view, shortcut: "M", run: () => setView("month") },
      { id: "view-week", title: "Week view", keywords: "switch", section: view, shortcut: "W", run: () => setView("week") },
      { id: "view-day", title: "Day view", keywords: "switch", section: view, shortcut: "D", run: () => setView("day") },
      { id: "view-agenda", title: "Agenda view", keywords: "list switch", section: view, shortcut: "A", run: () => setView("agenda") },
      {
        id: "new-event",
        title: "Create event",
        keywords: "new add",
        section: "Events",
        shortcut: "N",
        run: () => openEditor({ mode: "create", timing: defaultTiming() }),
      },
      {
        id: "search",
        title: "Search events",
        keywords: "find",
        section: "Events",
        shortcut: "Ctrl+F",
        run: () => {
          searchInput?.focus();
          searchInput?.select();
        },
      },
      { id: "settings", title: "Open settings", keywords: "preferences options", section: "Settings", shortcut: "Ctrl+,", run: () => openSettings() },
      { id: "manage-calendars", title: "Manage calendars", keywords: "colours sidebar", section: "Settings", run: () => openSettings("calendars") },
      { id: "manage-groups", title: "Manage calendar groups", keywords: "new edit", section: "Settings", run: () => openSettings("groups") },
      { id: "accounts", title: "Manage accounts", keywords: "sign in remove", section: "Settings", run: () => openSettings("accounts") },
      { id: "shortcuts", title: "Keyboard shortcuts", keywords: "help keys", section: "Help", shortcut: "?", run: () => (helpOpen = true) },
      { id: "group-all", title: "Show all calendars", keywords: "group everything", section: "Calendar groups", run: () => selectGroup("all") },
    ];
    for (const g of groups.groups) {
      list.push({ id: `group-${g.id}`, title: `Show group: ${g.name}`, keywords: "activate group", section: "Calendar groups", run: () => selectGroup(g.id) });
    }
    for (const c of calendars.calendars) {
      const shown = !calendars.hidden.has(c.id);
      list.push({
        id: `toggle-${c.id}`,
        title: `${shown ? "Hide" : "Show"} ${calendars.label(c.id)}`,
        keywords: "toggle calendar visibility events",
        section: "Calendars",
        run: () => void measureInteraction("visibility-toggle", () => toggleCalendar(c.id)),
      });
    }
    if (desktop && hasRemote) {
      list.push({ id: "sync", title: "Synchronize calendars now", keywords: "sync refresh", section: "Accounts", run: () => void backend.syncNow() });
    }
    if (desktop) {
      list.push(
        { id: "add-caldav", title: "Add CalDAV account…", keywords: "nextcloud", section: "Accounts", run: () => void openAddAccount("caldav") },
        { id: "add-google", title: "Add Google account…", section: "Accounts", run: () => void openAddAccount("google") },
        { id: "add-microsoft", title: "Add Microsoft account…", keywords: "outlook", section: "Accounts", run: () => void openAddAccount("microsoft") },
        { id: "add-ics", title: "Subscribe to a calendar…", keywords: "ics webcal feed", section: "Accounts", run: () => void openAddAccount("ics") },
      );
    }
    return list;
  });

  // --- keyboard -------------------------------------------------------------
  let perfOpen = $state(false);
  let helpOpen = $state(false);

  function onKeydown(e: KeyboardEvent) {
    // Dialogs handle their own keys.
    if (editor || addingAccount || removing || helpOpen || settingsTab || paletteOpen || reviewing) return;
    if (e.key === "Escape" && searchQuery) {
      clearSearch();
      searchInput?.focus();
      return;
    }
    const action = resolveShortcut(e);
    if (!action) return;
    e.preventDefault();
    switch (action) {
      case "today":
        return navigate("today");
      case "previous":
        return navigate("previous");
      case "next":
        return navigate("next");
      case "new-event":
        return void measureInteraction("open-editor", () => openEditor({ mode: "create", timing: defaultTiming() }));
      case "view-month":
        return setView("month");
      case "view-week":
        return setView("week");
      case "view-day":
        return setView("day");
      case "view-agenda":
        return setView("agenda");
      case "search":
        searchInput?.focus();
        searchInput?.select();
        return;
      case "toggle-perf":
        perfOpen = !perfOpen;
        return;
      case "help":
        helpOpen = true;
        return;
      case "palette":
        openPalette();
        return;
      case "settings":
        openSettings();
        return;
      case "toggle-hours":
        if (viewState.view === "week" || viewState.view === "day") setDayHours(dayHours === "work" ? "full" : "work");
        return;
    }
  }

  // --- lifecycle ------------------------------------------------------------
  let longTasks = $state(0);
  let longTasksSupported = $state(false);
  let benchStatus = $state<string | null>(null);
  let cachedCount = $state(0);
  const cleanups: (() => void)[] = [];

  onMount(() => {
    cleanups.push(watchSystemDark((dark) => (systemDark = dark)));
    window.addEventListener("keydown", onKeydown);
    cleanups.push(() => window.removeEventListener("keydown", onKeydown));
    // The window may close without components being destroyed.
    window.addEventListener("pagehide", flushSaves);
    cleanups.push(() => window.removeEventListener("pagehide", flushSaves));
    const stopLongTasks = observeLongTasks(() => longTasks++);
    longTasksSupported = stopLongTasks !== null;
    if (stopLongTasks) cleanups.push(stopLongTasks);
    // Cheap counter for the perf panel; updated at most once per frame.
    let pending = false;
    cleanups.push(
      store.subscribe(() => {
        if (pending) return;
        pending = true;
        requestAnimationFrame(() => {
          pending = false;
          cachedCount = store.size;
        });
      }),
    );
    cleanups.push(store.subscribe(() => refreshStatusesSoon()));
    cleanups.push(store.subscribe(() => checkEditorEvent()));
    let unlisten: (() => void) | null = null;
    let disposed = false;
    void backend.onSyncEvent(onSync).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    });
    cleanups.push(() => {
      disposed = true;
      unlisten?.();
    });
    refreshStatuses();
    void reloadCalendars();
    void startBenchIfRequested();
  });

  onDestroy(() => {
    for (const cleanup of cleanups) cleanup();
    clearTimeout(searchTimer);
    flushSaves();
    clearTimeout(statusTimer);
    notifications.dispose();
  });

  // --- benchmark ------------------------------------------------------------
  async function untilVisibleLoaded(): Promise<void> {
    // Wait for the adapter's visible-range load to finish, then a paint.
    while (store.loading || !store.covers(viewState.range)) {
      await new Promise((resolve) => setTimeout(resolve, 5));
    }
    await nextPaint();
  }

  const harness: BenchHarness = {
    anchor: parseIsoDate(BENCH_ANCHOR),
    async loadDataset(size) {
      await switchDataset(size, BENCH_ANCHOR);
      // Benchmarks measure the demo data, which is hidden by default.
      calendars.setChoices(Object.fromEntries(calendars.calendars.map((c) => [c.id, true])));
      await tick();
      await untilVisibleLoaded();
    },
    setView: (view) => controller?.setView(view),
    navigate: (direction) => controller?.navigate(direction),
    goToDate: (date) => controller?.goToDate(date),
    calendarIds: () => calendars.calendars.map((c) => c.id),
    toggleCalendar: (id) => calendars.toggle(id),
    visibleEditableIds: () =>
      store
        .eventsIn(viewState.range)
        .filter((e) => !calendars.isReadOnly(e.calendarId) && e.timing.kind === "timed")
        .map((e) => e.id),
    openEditor: (id) => openEditor({ mode: "edit", id }),
    closeEditor,
    setSettingsOpen: (open) => {
      settingsTab = open ? "calendars" : null;
    },
    setPaletteOpen: (open) => {
      paletteOpen = open;
    },
    showOnly: (ids) => calendars.setVisibleExactly(ids),
    setSearch: (query) => {
      searchText = query;
      searchQuery = query;
      if (!query) return;
      return new Promise<number>((resolve) => (searchShown = resolve));
    },
    createEvent: () => {
      const id = defaultCalendarId;
      if (!id) return null;
      const result = store.create({
        calendarId: id,
        title: "Benchmark event",
        description: "",
        location: "",
        timing: defaultTiming(),
      });
      return result.ok ? result.id : null;
    },
    moveEvent: (id, minutes) => {
      const event = store.get(id);
      if (!event || event.timing.kind !== "timed") return;
      const delta = minutes * 60_000;
      moveEvent({
        id,
        timing: { ...event.timing, start: event.timing.start + delta, end: event.timing.end + delta },
      });
    },
    renameEvent: (id, title) => {
      const event = store.get(id);
      if (event) store.update(id, { ...draftOf(event), title });
    },
    timeQueries: async (range, count) => {
      // Full round trip: IPC + SQLite + recurrence expansion (desktop).
      const start = performance.now();
      for (let i = 0; i < count; i++) await backend.source.queryRange(range);
      return performance.now() - start;
    },
    visibleRange: () => viewState.range,
  };

  async function startBenchIfRequested(): Promise<void> {
    let config;
    try {
      config = await getBenchConfig();
    } catch {
      return;
    }
    if (!config.enabled) return;
    try {
      const report = await runBenchmarks(harness, config.sizes, (m) => (benchStatus = m));
      benchStatus = "Benchmark finished";
      await submitBenchReport(report);
    } catch (error) {
      benchStatus = `Benchmark failed: ${String(error)}`;
      await submitBenchReport({ error: String(error) }).catch(() => {});
    }
  }

  const statusLine = $derived(
    startupError ??
      store.loadError ??
      (benchStatus ||
        (backend.kind === "desktop"
          ? `${calendars.calendars.length} ${calendars.calendars.length === 1 ? "calendar" : "calendars"} · saved locally`
          : `${datasetSize.toLocaleString()} simulated events · ${calendars.calendars.length} calendars · browser preview (not saved)`)),
  );
</script>

<div class="window" class:framed={customTitleBar}>
{#if customTitleBar}
  <TitleBar title="Universal Calendar" />
{/if}
<div class="app">
  <Sidebar
    {calendars}
    {groups}
    {selection}
    onselectgroup={selectGroup}
    ontogglecalendar={(id) => void measureInteraction("visibility-toggle", () => toggleCalendar(id))}
    onsettings={(tab) => openSettings(tab)}
  />
  <main>
    <Toolbar
      label={viewState.label}
      view={viewState.view}
      loading={store.loading}
      search={searchText}
      hours={dayHours}
      onhours={setDayHours}
      bind:searchInput
      onnavigate={navigate}
      onview={setView}
      onnew={() => void measureInteraction("open-editor", () => openEditor({ mode: "create", timing: defaultTiming() }))}
      onsearch={onSearchInput}
      onsearchsubmit={submitSearch}
      onhelp={() => (helpOpen = true)}
    />
    <div class="calendar" class:searching={searchQuery.trim() !== ""}>
      {#if agenda}
        <AgendaView
          {store}
          {calendars}
          initialDate={viewDate}
          onready={(c) => (controller = c)}
          onstatechange={(s) => (viewState = s)}
          onopen={(event) => void measureInteraction("open-editor", () => openEditor({ mode: "edit", id: event.id, event }))}
        />
      {:else}
        <CalendarAdapter
          {store}
          {calendars}
          {theme}
          initialView={gridView}
          initialDate={viewDate}
          hours={dayHours}
          onready={(c) => (controller = c)}
          onstatechange={(s) => (viewState = s)}
          oneditor={(request) => void measureInteraction("open-editor", () => openEditor(request))}
          onmove={moveEvent}
          onreject={(message) => notifications.push(message)}
        />
      {/if}
      {#if searchQuery.trim()}
        <div class="search-overlay">
          <SearchResults
            query={searchQuery}
            {store}
            {calendars}
            onopen={(event) => void measureInteraction("open-editor", () => revealEvent(event))}
            onsearched={(ms) => {
              searchShown?.(ms);
              searchShown = null;
              if (focusResults) {
                focusResults = false;
                void tick().then(searchFocusFirst);
              }
            }}
          />
        </div>
      {/if}
    </div>
    <footer class="status" role="status">{statusLine}</footer>
  </main>
</div>
</div>

{#if editor}
  {#key editor.key}
    <EventEditor
      event={editor.event}
      timing={editor.timing}
      calendars={calendars.calendars}
      accounts={calendars.accounts}
      {defaultCalendarId}
      readOnly={editor.event ? calendars.isReadOnly(editor.event.calendarId) : false}
      onsave={saveEditor}
      ondelete={deleteFromEditor}
      onclose={closeEditor}
      onresolve={backend.kind === "desktop" ? (resolution) => void resolveFromEditor(resolution) : undefined}
      onloadversions={backend.kind === "desktop" && editor.event
        ? () => backend.conflictVersions(editor!.event!.id)
        : undefined}
      reminderLimit={reminderLimit}
      newer={editorNewer !== null}
      onshownewer={showNewerVersion}
    />
  {/key}
{/if}

{#if addingAccount === "google" || addingAccount === "microsoft"}
  <OAuthAccountDialog
    provider={addingAccount}
    {builtInClientId}
    onsubmit={addOAuth}
    onclose={() => (addingAccount = false)}
    oncancel={() => void backend.cancelAddAccount()}
  />
{:else if addingAccount === "ics"}
  <SubscriptionDialog
    onsubmit={subscribe}
    onclose={() => (addingAccount = false)}
    oncancel={() => void backend.cancelAddAccount()}
  />
{:else if addingAccount === "caldav"}
  <AddAccountDialog
    onsubmit={addAccount}
    onclose={() => (addingAccount = false)}
    oncancel={() => void backend.cancelAddAccount()}
  />
{/if}

{#if reviewing}
  <SyncProblemsDialog
    {problems}
    onresolve={(p, r) => void resolveProblem(p, r)}
    onclose={() => {
      reviewing = false;
      problems = null;
    }}
  />
{/if}

{#if removing}
  <ConfirmDialog
    title="Remove account?"
    message={`“${removing.name}” and its calendars will be removed from this computer. Events on the server are not deleted. Changes that have not synced yet will be lost.`}
    confirmLabel="Remove account"
    onconfirm={() => void confirmRemoveAccount()}
    oncancel={() => (removing = null)}
  />
{/if}

{#if settingsTab}
  <SettingsDialog
    {calendars}
    {groups}
    initialTab={settingsTab}
    {themePreference}
    ontheme={setThemePreference}
    systemTitleBar={desktop ? systemTitleBar : undefined}
    onsystemtitlebar={setSystemTitleBar}
    notifications={desktop ? notificationsOn : undefined}
    onnotifications={setNotifications}
    ontogglecalendar={(id) => void measureInteraction("visibility-toggle", () => toggleCalendar(id))}
    onsidebar={setInSidebar}
    {defaultCalendarId}
    ondefaultcalendar={setDefaultCalendar}
    oncolor={setCalendarColor}
    ongroupschange={saveGroups}
    onaddaccount={desktop ? (kind) => void openAddAccount(kind) : undefined}
    onsignin={(id) => void signInAgain(id)}
    onremoveaccount={(id) => (removing = { id, name: calendars.accounts.find((a) => a.id === id)?.name ?? id })}
    onsyncnow={() => void backend.syncNow()}
    onreviewproblems={() => {
      reviewing = true;
      problems = null;
      void loadProblems();
    }}
    onshortcuts={() => (helpOpen = true)}
    simulation={backend.mock
      ? {
          datasetSize,
          latencyMs,
          ondataset: (size) => void measureInteraction("dataset-load", () => switchDataset(size)),
          onlatency: (ms) => {
            latencyMs = ms;
            if (backend.mock) backend.mock.latencyMs = ms;
          },
          onfailnext: () => {
            if (!backend.mock) return;
            backend.mock.failNextWrites++;
            notifications.push("The next save will fail (simulated).", "info");
          },
        }
      : null}
    onclose={() => (settingsTab = null)}
  />
{/if}

{#if paletteOpen}
  <CommandPalette {commands} onclose={() => (paletteOpen = false)} />
{/if}

{#if helpOpen}
  <ShortcutsDialog onclose={() => (helpOpen = false)} />
{/if}

{#if perfOpen}
  <PerfPanel
    eventCount={datasetSize}
    {cachedCount}
    {longTasksSupported}
    {longTasks}
    onclose={() => (perfOpen = false)}
  />
{/if}

<Toasts {notifications} />

<style>
  .window {
    display: flex;
    flex-direction: column;
    height: 100vh;
  }
  /* Without the native frame, draw a thin edge so the window stands out. */
  .window.framed {
    border: 1px solid var(--uc-border);
  }
  .app {
    display: grid;
    grid-template-columns: 15rem 1fr;
    flex: 1;
    min-height: 0;
  }
  main {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }
  .calendar {
    position: relative;
    flex: 1;
    min-height: 0;
  }
  /* Results cover the calendar, which stays mounted but is not painted. */
  .calendar.searching > :global(:not(.search-overlay)) {
    visibility: hidden;
  }
  .search-overlay {
    position: absolute;
    inset: 0;
    background: var(--uc-bg);
    z-index: 5;
  }
  .status {
    padding: 0.25rem 0.75rem;
    font-size: 0.75rem;
    color: var(--uc-text-muted);
    border-top: 1px solid var(--uc-border);
  }
  /* Narrow windows keep the sidebar (accounts, calendars, settings have no
     other home), just narrower. */
  @media (max-width: 720px) {
    .app {
      grid-template-columns: 11rem 1fr;
    }
  }
</style>
