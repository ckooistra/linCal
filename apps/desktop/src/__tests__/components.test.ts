import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/svelte";
import Sidebar from "../components/Sidebar.svelte";
import EventEditor from "../components/EventEditor.svelte";
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
  it("lists calendars by account and toggles visibility", async () => {
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
    expect(work.checked).toBe(true);
    await fireEvent.click(work);
    expect(calendars.isVisible("cal-work")).toBe(false);
    expect(work.checked).toBe(false);
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
      updatedAt: 0,
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
