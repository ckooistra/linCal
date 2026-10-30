import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/svelte";
import AddAccountDialog from "../components/AddAccountDialog.svelte";
import Sidebar from "../components/Sidebar.svelte";
import EventEditor from "../components/EventEditor.svelte";
import OAuthAccountDialog from "../components/OAuthAccountDialog.svelte";
import { CalendarsStore } from "../stores/calendars.svelte";
import { simulatedCalendars } from "../services/mock/generator";
import { parseIsoDate } from "../services/time";
import type { CalendarEvent } from "../types/calendar";

const noop = () => {};

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

describe("Sidebar sync status", () => {
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
  const props = (calendars: CalendarsStore, extra = {}) => ({
    calendars,
    datasetSize: 1000,
    latencyMs: 0,
    themePreference: "system" as const,
    simulationControls: false,
    ontogglecalendar: noop,
    oncolor: noop,
    ondataset: noop,
    onlatency: noop,
    ontheme: noop,
    onfailnext: noop,
    ...extra,
  });

  it("summarises account states and offers account actions", async () => {
    const calendars = store();
    const onremoveaccount = vi.fn();
    const onaddaccount = vi.fn();
    calendars.setStatuses([
      { accountId: "acct-sim", remote: false, lastSyncedAt: null, error: null, pending: 0, failed: 0, conflicts: 0, importFailures: 0, approximated: 0, needsSignIn: false },
      { accountId: "dav", remote: true, lastSyncedAt: Date.now(), error: null, pending: 2, failed: 0, conflicts: 0, importFailures: 0, approximated: 0, needsSignIn: false },
    ]);
    render(Sidebar, props(calendars, { onremoveaccount, onaddaccount, onsyncnow: noop }));
    expect(screen.getByText("2 changes waiting to sync")).toBeTruthy();
    expect(screen.getAllByRole("button", { name: "Remove" })).toHaveLength(1);
    await fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    expect(onremoveaccount).toHaveBeenCalledWith("dav");
    await fireEvent.click(screen.getByRole("button", { name: "Add CalDAV account…" }));
    expect(onaddaccount).toHaveBeenCalledWith("caldav");
    await fireEvent.click(screen.getByRole("button", { name: "Add Google account…" }));
    expect(onaddaccount).toHaveBeenCalledWith("google");
    await fireEvent.click(screen.getByRole("button", { name: "Add Microsoft account…" }));
    expect(onaddaccount).toHaveBeenCalledWith("microsoft");
    expect(screen.getByRole("button", { name: "Sync now" })).toBeTruthy();
  });

  it("shows syncing, offline and conflict states", async () => {
    const calendars = store();
    const base = { accountId: "dav", remote: true, lastSyncedAt: null, error: null, pending: 0, failed: 0, conflicts: 0, importFailures: 0, approximated: 0, needsSignIn: false };
    calendars.setStatuses([{ ...base, error: "Offline: the server could not be reached. Changes are kept and will sync later." }]);
    const { rerender } = render(Sidebar, props(calendars));
    expect(screen.getByText(/^Offline/).className).toContain("warn");
    calendars.setStatuses([{ ...base, conflicts: 1 }]);
    await rerender(props(calendars));
    expect(screen.getByText("1 conflict to resolve").className).toContain("error");
    const onreviewproblems = vi.fn();
    await rerender(props(calendars, { onreviewproblems }));
    await fireEvent.click(screen.getByRole("button", { name: "Review…" }));
    expect(onreviewproblems).toHaveBeenCalled();
    const onsignin = vi.fn();
    calendars.setStatuses([{ ...base, error: "The sign-in has expired.", needsSignIn: true }]);
    await rerender(props(calendars, { onsignin }));
    await fireEvent.click(screen.getByRole("button", { name: "Sign in again…" }));
    expect(onsignin).toHaveBeenCalledWith("dav");
    calendars.setStatuses([{ ...base, lastSyncedAt: Date.now(), importFailures: 2 }]);
    await rerender(props(calendars));
    expect(screen.getByText(/2 server events could not be read/).className).toContain("warn");
    calendars.setSyncing("dav", true);
    await rerender(props(calendars));
    expect(screen.getByText("Syncing…")).toBeTruthy();
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
