import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/svelte";
import Sidebar from "../components/Sidebar.svelte";
import EventEditor from "../components/EventEditor.svelte";
import TimeField from "../components/TimeField.svelte";
import { CalendarsStore } from "../stores/calendars.svelte";
import { simulatedAccounts, simulatedCalendars } from "../services/mock/generator";
import { parseIsoDate } from "../services/time";
import type { CalendarEvent } from "../types/calendar";

function calendarsStore() {
  const store = new CalendarsStore();
  store.load(simulatedAccounts(), simulatedCalendars());
  return store;
}

const noop = () => {};

describe("Sidebar", () => {
  it("hides demo calendars by default and toggles visibility", async () => {
    const calendars = calendarsStore();
    render(Sidebar, {
      calendars,
      datasetSize: 1000,
      latencyMs: 0,
      themePreference: "system",
      ontogglecalendar: (id: string) => calendars.toggle(id),
      oncolor: noop,
      ondataset: noop,
      onlatency: noop,
      ontheme: noop,
      onfailnext: noop,
    });
    const work = screen.getByLabelText("Work") as HTMLInputElement;
    // Simulated (demo) calendars start unchecked; clicking shows them.
    expect(work.checked).toBe(false);
    await fireEvent.click(work);
    expect(calendars.isVisible("cal-work")).toBe(true);
    expect(work.checked).toBe(true);
    expect(calendars.choices()).toEqual({ "cal-work": true });
    expect(screen.getAllByTitle("Read-only")).toHaveLength(2);
  });
});

describe("EventEditor", () => {
  const start = parseIsoDate("2026-06-15").getTime() + 9 * 3_600_000;
  const timing = { kind: "timed" as const, start, end: start + 3_600_000, timeZone: "Europe/Berlin" };

  it("creates a draft from the form", async () => {
    const onsave = vi.fn(() => null);
    render(EventEditor, {
      event: null,
      timing,
      calendars: simulatedCalendars(),
      defaultCalendarId: "cal-work",
      readOnly: false,
      onsave,
      ondelete: noop,
      onclose: noop,
    });
    const title = screen.getByLabelText("Title");
    expect(document.activeElement).toBe(title);
    await fireEvent.input(title, { target: { value: "Lunch" } });
    await fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(onsave).toHaveBeenCalledWith({
      calendarId: "cal-work",
      title: "Lunch",
      description: "",
      location: "",
      timing,
      rrule: null,
    });
    // Read-only calendars are not offered as targets.
    expect(screen.queryByRole("option", { name: "Holidays" })).toBeNull();
  });

  it("shows validation errors instead of saving", async () => {
    const onsave = vi.fn(() => null);
    render(EventEditor, {
      event: null,
      timing,
      calendars: simulatedCalendars(),
      defaultCalendarId: "cal-work",
      readOnly: false,
      onsave,
      ondelete: noop,
      onclose: noop,
    });
    await fireEvent.input(screen.getByLabelText("End time"), { target: { value: "08:00" } });
    await fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(onsave).not.toHaveBeenCalled();
    expect(screen.getByRole("alert").textContent).toContain("end after it starts");
  });

  it("closes on Escape and disables fields for read-only events", async () => {
    const onclose = vi.fn();
    const event: CalendarEvent = {
      id: "h",
      calendarId: "cal-holidays",
      title: "Holiday",
      description: "",
      location: "",
      timing: { kind: "all-day", startDate: "2026-06-15", endDate: "2026-06-16" },
      seriesId: null,
      syncStatus: "synced",
      revision: 1,
      recurrenceId: null,
  rrule: null,
    };
    render(EventEditor, {
      event,
      timing: event.timing,
      calendars: simulatedCalendars(),
      defaultCalendarId: null,
      readOnly: true,
      onsave: () => null,
      ondelete: noop,
      onclose,
    });
    expect(screen.getByLabelText("Title").matches(":disabled")).toBe(true);
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
    await fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(onclose).toHaveBeenCalled();
  });
});

describe("EventEditor with recurring events", () => {
  const start = parseIsoDate("2026-06-15").getTime() + 9 * 3_600_000;
  const occurrence: CalendarEvent = {
    id: "series-1::20260615T070000Z",
    calendarId: "cal-team",
    title: "Stand-up",
    description: "",
    location: "",
    timing: { kind: "timed", start, end: start + 900_000, timeZone: "Europe/Berlin" },
    seriesId: "series-1",
    recurrenceId: "20260615T070000Z",
    rrule: "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR",
    syncStatus: "saved",
    revision: 1,
  };

  it("keeps a custom rule, offers presets and both delete scopes", async () => {
    const onsave = vi.fn((_draft: unknown) => null);
    const ondelete = vi.fn();
    render(EventEditor, {
      event: occurrence,
      timing: occurrence.timing,
      calendars: simulatedCalendars(),
      defaultCalendarId: null,
      readOnly: false,
      onsave,
      ondelete,
      onclose: noop,
    });
    const repeat = screen.getByLabelText("Repeat") as HTMLSelectElement;
    expect(repeat.selectedOptions[0]?.textContent).toBe("Every weekday (Mon–Fri)");
    expect(screen.getByRole("note").textContent).toContain("every event in this series");
    await fireEvent.click(screen.getByRole("button", { name: "Delete this event" }));
    expect(ondelete).toHaveBeenCalledWith("occurrence");
    await fireEvent.click(screen.getByRole("button", { name: "Delete series" }));
    expect(ondelete).toHaveBeenCalledWith("series");
    await fireEvent.change(repeat, { target: { value: "FREQ=MONTHLY" } });
    await fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(onsave.mock.calls[0]?.[0]).toMatchObject({ rrule: "FREQ=MONTHLY", title: "Stand-up" });
  });
});

describe("TimeField", () => {
  it("opens a list of times on click and picks one", async () => {
    const onchange = vi.fn();
    render(TimeField, { value: "09:00", label: "Start time", onchange });
    const input = screen.getByRole("combobox", { name: "Start time" });
    await fireEvent.click(input);
    expect(input.getAttribute("aria-expanded")).toBe("true");
    const options = screen.getAllByRole("option");
    expect(options).toHaveLength(96);
    const selected = options.find((o) => o.getAttribute("aria-selected") === "true");
    expect(selected?.dataset.index).toBe("36");
    await fireEvent.mouseDown(options[42]!);
    expect(onchange).toHaveBeenCalledWith("10:30");
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("supports keyboard selection", async () => {
    const onchange = vi.fn();
    render(TimeField, { value: "09:00", label: "Start time", onchange });
    const input = screen.getByRole("combobox", { name: "Start time" });
    await fireEvent.keyDown(input, { key: "ArrowDown" });
    await fireEvent.keyDown(input, { key: "ArrowDown" });
    await fireEvent.keyDown(input, { key: "Enter" });
    expect(onchange).toHaveBeenCalledWith("09:15");
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("accepts typed times and shows durations after a start time", async () => {
    const onchange = vi.fn();
    render(TimeField, { value: "10:00", label: "End time", relativeTo: "09:00", onchange });
    const input = screen.getByRole("combobox", { name: "End time" });
    await fireEvent.input(input, { target: { value: "2:45pm" } });
    expect(onchange).toHaveBeenLastCalledWith("14:45");
    await fireEvent.click(input);
    expect(screen.getAllByRole("option")[42]?.textContent).toMatch(/\(1 h 30 min\)/);
  });
});

describe("EventEditor time handling", () => {
  it("closes an open time list with Escape before closing the dialog", async () => {
    const start = parseIsoDate("2026-06-15").getTime() + 9 * 3_600_000;
    const onclose = vi.fn();
    render(EventEditor, {
      event: null,
      timing: { kind: "timed", start, end: start + 3_600_000, timeZone: "Europe/Berlin" },
      calendars: simulatedCalendars(),
      defaultCalendarId: "cal-work",
      readOnly: false,
      onsave: () => null,
      ondelete: noop,
      onclose,
    });
    const input = screen.getByRole("combobox", { name: "Start time" });
    await fireEvent.keyDown(input, { key: "ArrowDown" });
    expect(screen.getByRole("listbox")).toBeTruthy();
    await fireEvent.keyDown(input, { key: "Escape" });
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(onclose).not.toHaveBeenCalled();
    await fireEvent.keyDown(input, { key: "Escape" });
    expect(onclose).toHaveBeenCalledOnce();
  });

  it("moves the end with the start so the duration is kept", async () => {
    const start = parseIsoDate("2026-06-15").getTime() + 9 * 3_600_000;
    const timing = { kind: "timed" as const, start, end: start + 90 * 60_000, timeZone: "Europe/Berlin" };
    const onsave = vi.fn((_draft: unknown) => null);
    render(EventEditor, {
      event: null,
      timing,
      calendars: simulatedCalendars(),
      defaultCalendarId: "cal-work",
      readOnly: false,
      onsave,
      ondelete: noop,
      onclose: noop,
    });
    await fireEvent.input(screen.getByRole("combobox", { name: "Start time" }), { target: { value: "23:00" } });
    await fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const saved = onsave.mock.calls[0]?.[0] as { timing: { start: number; end: number } };
    expect(saved.timing.end - saved.timing.start).toBe(90 * 60_000);
    // Crosses midnight: the end date moved to the next day.
    expect(new Date(saved.timing.end).getDate()).toBe(16);
  });
});
