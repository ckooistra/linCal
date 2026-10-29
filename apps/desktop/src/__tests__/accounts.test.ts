import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/svelte";
import AddAccountDialog from "../components/AddAccountDialog.svelte";
import Sidebar from "../components/Sidebar.svelte";
import EventEditor from "../components/EventEditor.svelte";
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
      { accountId: "acct-sim", remote: false, lastSyncedAt: null, error: null, pending: 0, failed: 0, conflicts: 0 },
      { accountId: "dav", remote: true, lastSyncedAt: Date.now(), error: null, pending: 2, failed: 0, conflicts: 0 },
    ]);
    render(Sidebar, props(calendars, { onremoveaccount, onaddaccount, onsyncnow: noop }));
    expect(screen.getByText("2 changes waiting to sync")).toBeTruthy();
    expect(screen.getAllByRole("button", { name: "Remove" })).toHaveLength(1);
    await fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    expect(onremoveaccount).toHaveBeenCalledWith("dav");
    await fireEvent.click(screen.getByRole("button", { name: "Add CalDAV account…" }));
    expect(onaddaccount).toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Sync now" })).toBeTruthy();
  });

  it("shows syncing, offline and conflict states", async () => {
    const calendars = store();
    const base = { accountId: "dav", remote: true, lastSyncedAt: null, error: null, pending: 0, failed: 0, conflicts: 0 };
    calendars.setStatuses([{ ...base, error: "Offline: the server could not be reached. Changes are kept and will sync later." }]);
    const { rerender } = render(Sidebar, props(calendars));
    expect(screen.getByText(/^Offline/).className).toContain("warn");
    calendars.setStatuses([{ ...base, conflicts: 1 }]);
    await rerender(props(calendars));
    expect(screen.getByText("1 conflict to resolve").className).toContain("error");
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
    expect(onresolve).toHaveBeenCalledWith(false);
    await fireEvent.click(screen.getByRole("button", { name: "Keep mine" }));
    expect(onresolve).toHaveBeenCalledWith(true);
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
