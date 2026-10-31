import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/svelte";
import AddAccountDialog from "../components/AddAccountDialog.svelte";
import Sidebar from "../components/Sidebar.svelte";
import SettingsDialog from "../components/SettingsDialog.svelte";
import { GroupsStore } from "../stores/groups.svelte";
import EventEditor from "../components/EventEditor.svelte";
import OAuthAccountDialog from "../components/OAuthAccountDialog.svelte";
import SubscriptionDialog from "../components/SubscriptionDialog.svelte";
import { CalendarsStore } from "../stores/calendars.svelte";
import { simulatedCalendars } from "../services/mock/generator";
import { parseIsoDate } from "../services/time";
import type { CalendarEvent } from "../types/calendar";

const noop = () => {};

describe("SubscriptionDialog", () => {
  it("validates, submits the address and shows errors", async () => {
    const onsubmit = vi.fn(async () => "Not an iCalendar feed.");
    render(SubscriptionDialog, { onsubmit, onclose: noop });
    await fireEvent.click(screen.getByRole("button", { name: "Subscribe" }));
    expect(onsubmit).not.toHaveBeenCalled();
    await fireEvent.input(screen.getByLabelText("Feed address"), {
      target: { value: " webcal://example.org/holidays.ics " },
    });
    await fireEvent.click(screen.getByRole("button", { name: "Subscribe" }));
    expect(onsubmit).toHaveBeenCalledWith("webcal://example.org/holidays.ics");
    expect((await screen.findByRole("alert")).textContent).toContain("iCalendar");
  });
});

describe("AddAccountDialog", () => {
  it("validates, submits and shows server errors", async () => {
    const onsubmit = vi.fn(async () => "The server rejected the user name or password.");
    render(AddAccountDialog, { onsubmit, onclose: noop });
    await fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    expect(onsubmit).not.toHaveBeenCalled();
    expect(screen.getByRole("alert").textContent).toContain("Fill in");

    await fireEvent.input(screen.getByLabelText("Server address"), { target: { value: " https://cloud.example.org " } });
    await fireEvent.input(screen.getByLabelText("User name"), { target: { value: "alice" } });
    await fireEvent.input(screen.getByLabelText("Password or app password"), { target: { value: "s3cret" } });
    await fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    expect(onsubmit).toHaveBeenCalledWith("https://cloud.example.org", "alice", "s3cret");
    expect((await screen.findByRole("alert")).textContent).toContain("rejected");
  });
});

describe("Account status in Settings and the sidebar", () => {
  function store() {
    const s = new CalendarsStore();
    s.load(
      [
        { id: "acct-sim", name: "Simulated", provider: "simulated" },
        { id: "dav", name: "alice @ cloud.example.org", provider: "caldav" },
      ],
      [{ id: "dav/work", accountId: "dav", name: "Work", color: "#123456", readOnly: false }],
    );
    return s;
  }
  const base = { accountId: "dav", remote: true, lastSyncedAt: null, error: null, pending: 0, failed: 0, conflicts: 0, importFailures: 0, approximated: 0, needsSignIn: false };
  const settings = (calendars: CalendarsStore, extra = {}) => ({
    calendars,
    groups: new GroupsStore(),
    initialTab: "accounts" as const,
    themePreference: "system" as const,
    ontheme: noop,
    ontogglecalendar: noop,
    onsidebar: noop,
    oncolor: noop,
    ongroupschange: noop,
    onshortcuts: noop,
    onclose: noop,
    ...extra,
  });

  it("summarises account states and offers account actions", async () => {
    const calendars = store();
    const onremoveaccount = vi.fn();
    const onaddaccount = vi.fn();
    calendars.setStatuses([
      { ...base, accountId: "acct-sim", remote: false },
      { ...base, lastSyncedAt: Date.now(), pending: 2 },
    ]);
    render(SettingsDialog, settings(calendars, { onremoveaccount, onaddaccount, onsyncnow: noop }));
    expect(screen.getByText("2 changes waiting to sync")).toBeTruthy();
    expect(screen.getAllByRole("button", { name: /^Remove/ })).toHaveLength(1);
    await fireEvent.click(screen.getByRole("button", { name: "Remove alice @ cloud.example.org" }));
    expect(onremoveaccount).toHaveBeenCalledWith("dav");
    for (const [name, kind] of [
      ["Add CalDAV account…", "caldav"],
      ["Add Google account…", "google"],
      ["Add Microsoft account…", "microsoft"],
      ["Subscribe to a calendar…", "ics"],
    ] as const) {
      await fireEvent.click(screen.getByRole("button", { name }));
      expect(onaddaccount).toHaveBeenLastCalledWith(kind);
    }
    expect(screen.getByRole("button", { name: "Sync all accounts now" })).toBeTruthy();
  });

  it("shows syncing, offline and conflict states with their actions", async () => {
    const calendars = store();
    calendars.setStatuses([{ ...base, error: "Offline: the server could not be reached. Changes are kept and will sync later." }]);
    const onreviewproblems = vi.fn();
    const onsignin = vi.fn();
    const { rerender } = render(SettingsDialog, settings(calendars, { onreviewproblems, onsignin }));
    expect(screen.getByText(/^Offline/).className).toContain("warn");
    calendars.setStatuses([{ ...base, conflicts: 1 }]);
    await rerender(settings(calendars, { onreviewproblems, onsignin }));
    expect(screen.getByText("1 conflict to resolve").className).toContain("error");
    await fireEvent.click(screen.getByRole("button", { name: "Review…" }));
    expect(onreviewproblems).toHaveBeenCalled();
    calendars.setStatuses([{ ...base, error: "The sign-in has expired.", needsSignIn: true }]);
    await rerender(settings(calendars, { onreviewproblems, onsignin }));
    await fireEvent.click(screen.getByRole("button", { name: "Sign in again…" }));
    expect(onsignin).toHaveBeenCalledWith("dav");
    calendars.setStatuses([{ ...base, lastSyncedAt: Date.now(), importFailures: 2 }]);
    await rerender(settings(calendars, { onreviewproblems, onsignin }));
    expect(screen.getByText(/2 server events could not be read/).className).toContain("warn");
    calendars.setSyncing("dav", true);
    await rerender(settings(calendars, { onreviewproblems, onsignin }));
    expect(screen.getByText("Syncing…")).toBeTruthy();
  });

  it("the sidebar shows one line when an account needs attention", async () => {
    const calendars = store();
    const onsettings = vi.fn();
    const props = () => ({
      calendars,
      groups: new GroupsStore(),
      selection: "all",
      onselectgroup: noop,
      ontogglecalendar: noop,
      onsettings,
    });
    calendars.setStatuses([{ ...base, lastSyncedAt: Date.now() }]);
    const { rerender } = render(Sidebar, props());
    expect(screen.queryByText(/needs attention/)).toBeNull();
    calendars.setStatuses([{ ...base, conflicts: 2 }]);
    await rerender(props());
    await fireEvent.click(screen.getByRole("button", { name: /1 account needs attention/ }));
    expect(onsettings).toHaveBeenCalledWith("accounts");
  });
});

describe("EventEditor sync conflicts", () => {
  it("offers to keep the local or the server version", async () => {
    const start = parseIsoDate("2026-06-15").getTime() + 9 * 3_600_000;
    const event: CalendarEvent = {
      id: "dav/work/abc",
      calendarId: "cal-work",
      title: "Review",
      description: "",
      location: "",
      timing: { kind: "timed", start, end: start + 3_600_000, timeZone: "Europe/Berlin" },
      seriesId: null,
      recurrenceId: null,
      rrule: null,
      reminders: [],
      syncStatus: "conflict",
      revision: 2,
    };
    const onresolve = vi.fn();
    render(EventEditor, {
      event,
      timing: event.timing,
      calendars: simulatedCalendars(),
      defaultCalendarId: null,
      readOnly: false,
      onsave: () => null,
      ondelete: noop,
      onclose: noop,
      onresolve,
    });
    expect(screen.getByRole("alert").textContent).toContain("also changed on the server");
    await fireEvent.click(screen.getByRole("button", { name: "Use server version" }));
    expect(onresolve).toHaveBeenCalledWith("use-server");
    await fireEvent.click(screen.getByRole("button", { name: "Keep mine" }));
    expect(onresolve).toHaveBeenCalledWith("keep-local");
  });
});

describe("Calendar visibility defaults", () => {
  it("shows real calendars, hides demo ones, and honours saved choices", () => {
    const s = new CalendarsStore();
    s.setChoices({ "cal-team": true, "dav/home": false });
    s.load(
      [
        { id: "acct-work", name: "Work (simulated Google)", provider: "simulated" },
        { id: "dav", name: "alice", provider: "caldav" },
      ],
      [
        { id: "cal-work", accountId: "acct-work", name: "Work", color: "#111111", readOnly: false },
        { id: "cal-team", accountId: "acct-work", name: "Team", color: "#222222", readOnly: false },
        { id: "dav/work", accountId: "dav", name: "Work", color: "#333333", readOnly: false },
        { id: "dav/home", accountId: "dav", name: "Home", color: "#444444", readOnly: false },
      ],
    );
    expect([...s.hidden].sort()).toEqual(["cal-work", "dav/home"]);
    s.toggle("dav/home");
    expect(s.isVisible("dav/home")).toBe(true);
    expect(s.choices()).toEqual({ "cal-team": true, "dav/home": true });
  });
});

describe("TitleBar", async () => {
  const { default: TitleBar } = await import("../components/TitleBar.svelte");
  it("renders a draggable title with window controls", () => {
    const { container } = render(TitleBar, { title: "Universal Calendar" });
    expect(container.querySelector("[data-tauri-drag-region]")).toBeTruthy();
    for (const name of ["Minimize", "Maximize", "Close"]) {
      expect(screen.getByRole("button", { name })).toBeTruthy();
    }
  });
});

describe("EventEditor compares conflicting versions", () => {
  it("shows both versions with the changed fields marked", async () => {
    const start = parseIsoDate("2026-06-15").getTime() + 9 * 3_600_000;
    const event: CalendarEvent = {
      id: "e1",
      calendarId: "cal-work",
      title: "Mine",
      description: "",
      location: "Room 4",
      timing: { kind: "timed", start, end: start + 3_600_000, timeZone: "Europe/Berlin" },
      seriesId: null,
      recurrenceId: null,
      rrule: null,
      reminders: [],
      syncStatus: "conflict",
      revision: 1,
    };
    const version = { title: "Mine", description: "", location: "Room 4", timing: event.timing, rrule: null, reminders: [] };
    const onloadversions = vi.fn(async () => ({ local: version, server: { ...version, title: "Theirs" } }));
    render(EventEditor, {
      event,
      timing: event.timing,
      calendars: simulatedCalendars(),
      defaultCalendarId: null,
      readOnly: false,
      onsave: () => null,
      ondelete: noop,
      onclose: noop,
      onresolve: noop,
      onloadversions,
    });
    await fireEvent.click(screen.getByRole("button", { name: "Compare versions" }));
    const table = await screen.findByRole("table");
    const changed = [...table.querySelectorAll("tr.differs")].map((r) => r.textContent);
    expect(changed).toEqual(["TitleMineTheirs"]);
    expect(screen.getByRole("button", { name: "Keep mine" })).toBeTruthy();
  });
});

describe("EventEditor keeps untouched times exactly (audit F09)", () => {
  it("saves a title-only change without moving the event in a DST fold", async () => {
    // 02:30:45 CET on 2026-10-25: the second 02:30 of the night clocks go back.
    const start = Date.UTC(2026, 9, 25, 1, 30, 45);
    const event: CalendarEvent = {
      id: "e",
      calendarId: "cal-work",
      title: "Night shift",
      description: "",
      location: "",
      timing: { kind: "timed", start, end: start + 3_600_000, timeZone: "Europe/Berlin" },
      seriesId: null,
      recurrenceId: null,
      rrule: null,
      reminders: [],
      syncStatus: "saved",
      revision: 1,
    };
    const onsave = vi.fn(() => null);
    render(EventEditor, {
      event,
      timing: event.timing,
      calendars: simulatedCalendars(),
      defaultCalendarId: null,
      readOnly: false,
      onsave,
      ondelete: noop,
      onclose: noop,
    });
    await fireEvent.input(screen.getByLabelText("Title"), { target: { value: "Night shift (renamed)" } });
    await fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(onsave).toHaveBeenCalledWith(
      expect.objectContaining({ title: "Night shift (renamed)", timing: event.timing }),
      "series",
    );
  });
});

describe("EventEditor notices newer versions", () => {
  it("offers to show a version changed elsewhere", async () => {
    const start = parseIsoDate("2026-06-15").getTime() + 9 * 3_600_000;
    const event: CalendarEvent = {
      id: "e",
      calendarId: "cal-work",
      title: "Review",
      description: "",
      location: "",
      timing: { kind: "timed", start, end: start + 3_600_000, timeZone: "Europe/Berlin" },
      seriesId: null,
      recurrenceId: null,
      rrule: null,
      reminders: [],
      syncStatus: "synced",
      revision: 1,
    };
    const onshownewer = vi.fn();
    render(EventEditor, {
      event,
      timing: event.timing,
      calendars: simulatedCalendars(),
      defaultCalendarId: null,
      readOnly: false,
      onsave: () => null,
      ondelete: noop,
      onclose: noop,
      newer: true,
      onshownewer,
    });
    await fireEvent.click(screen.getByRole("button", { name: "Show the new version" }));
    expect(onshownewer).toHaveBeenCalled();
  });
});

describe("OAuthAccountDialog", () => {
  it("requires a client ID, submits, and cancels a sign-in in progress", async () => {
    let finish!: (v: string | null) => void;
    const onsubmit = vi.fn(() => new Promise<string | null>((r) => (finish = r)));
    const oncancel = vi.fn();
    const onclose = vi.fn();
    render(OAuthAccountDialog, { provider: "google", builtInClientId: null, onsubmit, onclose, oncancel });
    await fireEvent.click(screen.getByRole("button", { name: "Sign in with Google" }));
    expect(onsubmit).not.toHaveBeenCalled();
    await fireEvent.input(screen.getByLabelText("OAuth client ID"), { target: { value: " id.apps " } });
    await fireEvent.click(screen.getByRole("button", { name: "Sign in with Google" }));
    expect(onsubmit).toHaveBeenCalledWith("id.apps", null);
    expect(screen.getByRole("status").textContent).toContain("browser");
    await fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(oncancel).toHaveBeenCalled();
    expect(onclose).toHaveBeenCalled();
    finish(null);
  });
});

describe("OAuthAccountDialog for Microsoft", () => {
  it("uses the built-in client ID and asks for no secret", async () => {
    const onsubmit = vi.fn(async () => null);
    render(OAuthAccountDialog, { provider: "microsoft", builtInClientId: "built-in-id", onsubmit, onclose: noop });
    expect(screen.queryByLabelText(/Client secret/)).toBeNull();
    await fireEvent.click(screen.getByRole("button", { name: "Sign in with Microsoft" }));
    expect(onsubmit).toHaveBeenCalledWith("built-in-id", null);
  });
});
