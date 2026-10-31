import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/svelte";
import Sidebar from "../components/Sidebar.svelte";
import EventEditor from "../components/EventEditor.svelte";
import TimeField from "../components/TimeField.svelte";
import SearchResults from "../components/SearchResults.svelte";
import AgendaView from "../components/AgendaView.svelte";
import { MockEventSource } from "../services/mock/mockEventSource";
import { EventStore } from "../stores/events.svelte";
import { CalendarsStore } from "../stores/calendars.svelte";
import { GroupsStore, selectionFor } from "../stores/groups.svelte";
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
  function setup() {
    const calendars = calendarsStore();
    const groups = new GroupsStore();
    const work = groups.add("Work", "#3b5bdb", ["cal-work", "cal-team"]);
    return { calendars, groups, work };
  }
  const props = (calendars: CalendarsStore, groups: GroupsStore, extra = {}) => ({
    calendars,
    groups,
    selection: selectionFor(groups.groups, calendars.calendars.map((c) => c.id), calendars.hidden),
    onselectgroup: noop,
    ontogglecalendar: (id: string) => calendars.toggle(id),
    onsettings: noop,
    ...extra,
  });

  it("lists only sidebar calendars and toggles their events", async () => {
    const { calendars, groups } = setup();
    calendars.setSidebarChoices({ "cal-holidays": false, "cal-birthdays": false });
    render(Sidebar, props(calendars, groups));
    expect(screen.queryByLabelText("Show events of Holidays")).toBeNull();
    const work = screen.getByLabelText("Show events of Work") as HTMLInputElement;
    // Simulated (demo) calendars start hidden; checking shows their events.
    expect(work.checked).toBe(false);
    await fireEvent.click(work);
    expect(calendars.isVisible("cal-work")).toBe(true);
    // Hidden from the sidebar is not hidden events: the calendar still exists.
    expect(calendars.get("cal-holidays")).toBeDefined();
  });

  it("selects groups, and shows a custom selection after manual changes", async () => {
    const { calendars, groups, work } = setup();
    const onselectgroup = vi.fn((id: string) =>
      calendars.setVisibleExactly(id === "all" ? calendars.calendars.map((c) => c.id) : groups.get(id)!.calendarIds),
    );
    const { rerender } = render(Sidebar, props(calendars, groups, { onselectgroup }));
    const select = screen.getByLabelText("Calendar group") as HTMLSelectElement;
    expect(select.value).toBe("custom");
    await fireEvent.change(select, { target: { value: work.id } });
    expect(onselectgroup).toHaveBeenCalledWith(work.id);
    await rerender(props(calendars, groups, { onselectgroup }));
    expect(select.value).toBe(work.id);
    expect([...calendars.calendars].filter((c) => calendars.isVisible(c.id)).map((c) => c.id)).toEqual(["cal-work", "cal-team"]);
    // A group never changes calendar colours.
    expect(calendars.get("cal-work")?.color).toBe(simulatedCalendars().find((c) => c.id === "cal-work")?.color);
    calendars.toggle("cal-team");
    await rerender(props(calendars, groups, { onselectgroup }));
    expect(select.value).toBe("custom");
    await fireEvent.change(select, { target: { value: "all" } });
    await rerender(props(calendars, groups, { onselectgroup }));
    expect(select.value).toBe("all");
  });

  it("shows an empty state that leads to Settings", async () => {
    const { calendars, groups } = setup();
    calendars.setSidebarChoices(Object.fromEntries(calendars.calendars.map((c) => [c.id, false])));
    const onsettings = vi.fn();
    render(Sidebar, props(calendars, groups, { onsettings }));
    expect(screen.getByText(/No calendars in the sidebar/)).toBeTruthy();
    await fireEvent.click(screen.getByRole("button", { name: "Choose some in Settings" }));
    expect(onsettings).toHaveBeenCalledWith("calendars");
    await fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    expect(onsettings).toHaveBeenLastCalledWith();
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
      reminders: [],
    }, "series");
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
  reminders: [],
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
    reminders: [],
    syncStatus: "saved",
    revision: 1,
  };

  it("keeps the rule and asks which events a save or delete applies to", async () => {
    const onsave = vi.fn((_draft: unknown, _scope: unknown) => null);
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
    // Delete: three choices, Back returns to the form.
    await fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(screen.getByRole("group", { name: /repeating event\. Delete:/ })).toBeTruthy();
    await fireEvent.click(screen.getByRole("button", { name: "Back" }));
    await fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    await fireEvent.click(screen.getByRole("button", { name: "This and following events" }));
    expect(ondelete).toHaveBeenCalledWith("following");

    // Saving without changes just closes; a title change can be for this event only.
    await fireEvent.click(screen.getByRole("button", { name: "Back" }));
    await fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(screen.queryByRole("group", { name: /repeating event/ })).toBeNull();
    await fireEvent.input(screen.getByLabelText("Title"), { target: { value: "Sync" } });
    await fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "This event" }));
    await fireEvent.click(screen.getByRole("button", { name: "This event" }));
    expect(onsave).toHaveBeenLastCalledWith(expect.objectContaining({ title: "Sync" }), "occurrence");

    // A new rule can't apply to one event.
    await fireEvent.change(repeat, { target: { value: "FREQ=MONTHLY" } });
    await fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect((screen.getByRole("button", { name: "This event" }) as HTMLButtonElement).disabled).toBe(true);
    await fireEvent.click(screen.getByRole("button", { name: "All events" }));
    expect(onsave).toHaveBeenLastCalledWith(expect.objectContaining({ rrule: "FREQ=MONTHLY" }), "series");
  });
});

describe("EventEditor custom repeat", () => {
  it("builds a rule from the custom fields", async () => {
    // Tuesday 16 June 2026, 09:00.
    const start = parseIsoDate("2026-06-16").getTime() + 9 * 3_600_000;
    const onsave = vi.fn((_draft: unknown, _scope: unknown) => null);
    render(EventEditor, {
      event: null,
      timing: { kind: "timed", start, end: start + 3_600_000, timeZone: "Europe/Berlin" },
      calendars: simulatedCalendars(),
      defaultCalendarId: "cal-work",
      readOnly: false,
      onsave,
      ondelete: noop,
      onclose: noop,
    });
    await fireEvent.change(screen.getByLabelText("Repeat"), { target: { value: "custom" } });
    expect(screen.getByText("Every week on Tuesday")).toBeTruthy();
    await fireEvent.input(screen.getByLabelText("Repeat every"), { target: { value: "2" } });
    await fireEvent.click(screen.getByRole("button", { name: "Thursday" }));
    // The last selected day cannot be switched off.
    await fireEvent.click(screen.getByRole("button", { name: "Tuesday" }));
    await fireEvent.click(screen.getByRole("button", { name: "Thursday" }));
    expect(screen.getByRole("button", { name: "Thursday" }).getAttribute("aria-pressed")).toBe("true");
    await fireEvent.change(screen.getByLabelText("Repeat ends"), { target: { value: "count" } });
    await fireEvent.input(screen.getByLabelText("Number of times"), { target: { value: "6" } });
    expect(screen.getByText("Every 2 weeks on Thursday, 6 times")).toBeTruthy();
    await fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(onsave).toHaveBeenCalledWith(
      expect.objectContaining({ rrule: "FREQ=WEEKLY;INTERVAL=2;BYDAY=TH;COUNT=6" }),
      "series",
    );
  });

  it("opens an editable custom rule in the custom fields and keeps others as they are", async () => {
    const start = parseIsoDate("2026-06-16").getTime() + 9 * 3_600_000;
    const series = (rrule: string): CalendarEvent => ({
      id: "s::x",
      calendarId: "cal-work",
      title: "Review",
      description: "",
      location: "",
      timing: { kind: "timed", start, end: start + 3_600_000, timeZone: "Europe/Berlin" },
      seriesId: "s",
      recurrenceId: "x",
      rrule,
      reminders: [],
      syncStatus: "saved",
      revision: 1,
    });
    const props = { calendars: simulatedCalendars(), defaultCalendarId: null, readOnly: false, ondelete: noop, onclose: noop };
    const first = render(EventEditor, { ...props, event: series("FREQ=MONTHLY;BYDAY=3TU"), timing: series("").timing, onsave: () => null });
    expect(screen.getByText("Every month on the third Tuesday")).toBeTruthy();
    first.unmount();
    render(EventEditor, { ...props, event: series("FREQ=MONTHLY;BYMONTHDAY=1,15"), timing: series("").timing, onsave: () => null });
    const repeat = screen.getByLabelText("Repeat") as HTMLSelectElement;
    expect(repeat.selectedOptions[0]?.textContent).toBe("Custom (FREQ=MONTHLY;BYMONTHDAY=1,15)");
    expect(screen.queryByRole("group", { name: "Custom repeat" })).toBeNull();
  });
});

describe("EventEditor reminders", () => {
  const start = parseIsoDate("2026-06-16").getTime() + 9 * 3_600_000;
  const timing = { kind: "timed" as const, start, end: start + 3_600_000, timeZone: "Europe/Berlin" };
  const props = {
    event: null,
    timing,
    calendars: simulatedCalendars(),
    defaultCalendarId: "cal-work",
    readOnly: false,
    ondelete: noop,
    onclose: noop,
  };

  it("adds and removes reminders", async () => {
    const onsave = vi.fn((_draft: unknown, _scope: unknown) => null);
    render(EventEditor, { ...props, onsave });
    const add = () => screen.getByLabelText("Add reminder");
    await fireEvent.change(add(), { target: { value: "1440" } });
    await fireEvent.change(add(), { target: { value: "15" } });
    await fireEvent.change(add(), { target: { value: "0" } });
    expect(screen.getByText("1 day before")).toBeTruthy();
    // Chosen ones are not offered again.
    expect(screen.queryByRole("option", { name: "15 minutes before" })).toBeNull();
    await fireEvent.click(screen.getByRole("button", { name: "Remove reminder At start" }));
    await fireEvent.input(screen.getByLabelText("Title"), { target: { value: "Dentist" } });
    await fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(onsave).toHaveBeenCalledWith(expect.objectContaining({ reminders: [15, 1440] }), "series");
  });

  it("offers one reminder where the calendar holds one", async () => {
    render(EventEditor, { ...props, onsave: () => null, reminderLimit: () => 1 });
    await fireEvent.change(screen.getByLabelText("Add reminder"), { target: { value: "30" } });
    expect(screen.queryByLabelText("Add reminder")).toBeNull();
    expect(screen.getByText("This calendar holds one reminder per event.")).toBeTruthy();
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

describe("SearchResults", () => {
  it("lists upcoming then earlier matches and opens one", async () => {
    const source = new MockEventSource({ accounts: simulatedAccounts(), calendars: simulatedCalendars(), events: [] });
    const now = Date.now();
    const day = 86_400_000;
    const add = (id: string, title: string, start: number) =>
      source.createEvent(id, {
        calendarId: "cal-work",
        title,
        description: "",
        location: "",
        timing: { kind: "timed", start, end: start + 3_600_000, timeZone: null },
      });
    await add("past", "Dentist check", now - 7 * day);
    await add("next", "Dentist appointment", now + 2 * day);
    await add("other", "Lunch", now + day);
    const calendars = calendarsStore();
    calendars.setChoices({ "cal-work": true });
    const store = new EventStore(source, { isReadOnly: () => false, onError: noop });
    const onopen = vi.fn();
    render(SearchResults, { query: "dentist", store, calendars, onopen });
    expect(await screen.findByText(/2 events match/)).toBeTruthy();
    const headings = screen.getAllByRole("heading", { level: 2 }).map((h) => h.textContent);
    expect(headings.indexOf("Upcoming")).toBeLessThan(headings.indexOf("Earlier"));
    expect(screen.queryByText("Lunch")).toBeNull();
    await fireEvent.click(screen.getByText("Dentist appointment"));
    expect(onopen).toHaveBeenCalledWith(expect.objectContaining({ id: "next" }));
  });
});

describe("AgendaView", () => {
  it("lists the month by day, navigates by month and opens events", async () => {
    const source = new MockEventSource({ accounts: simulatedAccounts(), calendars: simulatedCalendars(), events: [] });
    const at = (date: string, hour: number) => parseIsoDate(date).getTime() + hour * 3_600_000;
    const add = (id: string, title: string, start: number, calendarId = "cal-work") =>
      source.createEvent(id, {
        calendarId,
        title,
        description: "",
        location: "",
        timing: { kind: "timed", start, end: start + 3_600_000, timeZone: null },
      });
    await add("june", "Planning", at("2026-06-03", 9));
    await add("july", "Retro", at("2026-07-08", 14));
    await add("hidden", "Secret", at("2026-06-04", 9), "cal-personal");
    const calendars = calendarsStore();
    calendars.setChoices({ "cal-work": true, "cal-personal": false });
    const store = new EventStore(source, { isReadOnly: () => false, onError: noop });
    const onopen = vi.fn();
    const states: string[] = [];
    let controller: { navigate(d: "previous" | "next" | "today"): void } | null = null;
    render(AgendaView, {
      store,
      calendars,
      initialDate: parseIsoDate("2026-06-15"),
      onready: (c: typeof controller) => (controller = c),
      onstatechange: (s: { view: string; label: string }) => states.push(`${s.view} ${s.label}`),
      onopen,
    });
    expect(await screen.findByText("Planning")).toBeTruthy();
    expect(screen.queryByText("Secret")).toBeNull();
    expect(screen.queryByText("Retro")).toBeNull();
    expect(states.at(-1)).toMatch(/^agenda June 2026/);
    controller!.navigate("next");
    expect(await screen.findByText("Retro")).toBeTruthy();
    expect(states.at(-1)).toMatch(/^agenda July 2026/);
    await fireEvent.click(screen.getByText("Retro"));
    expect(onopen).toHaveBeenCalledWith(expect.objectContaining({ id: "july" }));
  });
});

describe("EventContent (grid events)", () => {
  it("is a labelled button that opens with Enter or Space", async () => {
    const { default: Harness } = await import("./EventContentHarness.svelte");
    const open = vi.fn();
    const start = new Date(2026, 5, 16, 10, 30);
    render(Harness, {
      open,
      event: {
        id: "e1",
        start,
        end: new Date(2026, 5, 16, 11, 0),
        allDay: false,
        text: "Dentist",
        calendarId: "cal-work",
        status: "synced",
        readOnly: false,
      },
      mode: "boxes",
    });
    const button = screen.getByRole("button", { name: /^Dentist, .*, Work$/ });
    expect(button.getAttribute("tabindex")).toBe("0");
    await fireEvent.keyDown(button, { key: "Enter" });
    await fireEvent.keyDown(button, { key: " " });
    await fireEvent.keyDown(button, { key: "a" });
    expect(open).toHaveBeenCalledTimes(2);
    expect(open).toHaveBeenCalledWith("e1");
  });
});

describe("SettingsDialog", () => {
  async function open(extra = {}) {
    const { default: SettingsDialog } = await import("../components/SettingsDialog.svelte");
    const calendars = calendarsStore();
    const groups = new GroupsStore();
    const handlers = {
      onsidebar: vi.fn((id: string, on: boolean) => calendars.setInSidebar(id, on)),
      ontogglecalendar: vi.fn((id: string) => calendars.toggle(id)),
      ongroupschange: vi.fn(),
      onclose: vi.fn(),
    };
    render(SettingsDialog, {
      calendars,
      groups,
      themePreference: "system",
      ontheme: noop,
      oncolor: noop,
      onshortcuts: noop,
      ...handlers,
      ...extra,
    });
    return { calendars, groups, ...handlers };
  }

  it("switches tabs with the arrow keys and closes with Escape", async () => {
    const { onclose } = await open();
    const calendarsTab = screen.getByRole("tab", { name: "Calendars" });
    expect(document.activeElement).toBe(calendarsTab);
    expect(calendarsTab.getAttribute("aria-selected")).toBe("true");
    await fireEvent.keyDown(calendarsTab, { key: "ArrowDown" });
    const groupsTab = screen.getByRole("tab", { name: "Calendar groups" });
    expect(groupsTab.getAttribute("aria-selected")).toBe("true");
    expect(screen.getByRole("tabpanel").getAttribute("aria-labelledby")).toBe(groupsTab.id);
    await fireEvent.keyDown(groupsTab, { key: "End" });
    expect(screen.getByRole("tab", { name: "General" }).getAttribute("aria-selected")).toBe("true");
    await fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(onclose).toHaveBeenCalled();
  });

  it("keeps 'in sidebar' and 'show events' separate", async () => {
    const { calendars, onsidebar, ontogglecalendar } = await open();
    await fireEvent.click(screen.getByLabelText("Show Work in the sidebar"));
    expect(onsidebar).toHaveBeenCalledWith("cal-work", false);
    expect(calendars.inSidebar("cal-work")).toBe(false);
    expect(ontogglecalendar).not.toHaveBeenCalled();
    await fireEvent.click(screen.getByLabelText("Show events of Work"));
    expect(ontogglecalendar).toHaveBeenCalledWith("cal-work");
    // Still listed (connected), just not in the sidebar.
    expect(screen.getByLabelText("Show Work in the sidebar")).toBeTruthy();
  });

  it("creates, edits and deletes calendar groups", async () => {
    const { groups, ongroupschange } = await open({ initialTab: "groups" });
    await fireEvent.click(screen.getByRole("button", { name: "New group…" }));
    const name = screen.getByLabelText("Name");
    expect(document.activeElement).toBe(name);
    await fireEvent.click(screen.getByRole("button", { name: "Create group" }));
    expect(screen.getByRole("alert").textContent).toContain("name");
    await fireEvent.input(name, { target: { value: "Work" } });
    await fireEvent.click(screen.getByRole("checkbox", { name: "Team" }));
    await fireEvent.click(screen.getByRole("button", { name: "Create group" }));
    expect(groups.groups).toHaveLength(1);
    expect(groups.groups[0]!.name).toBe("Work");
    expect(groups.groups[0]!.calendarIds).toContain("cal-team");
    expect(ongroupschange).toHaveBeenCalledTimes(1);
    expect(screen.getByText("1 calendar")).toBeTruthy();
    // Focus stays in the dialog, on the new group.
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Edit Work" }));

    await fireEvent.click(screen.getByRole("button", { name: "Edit Work" }));
    await fireEvent.input(screen.getByLabelText("Name"), { target: { value: "Office" } });
    await fireEvent.click(screen.getByRole("button", { name: "Save group" }));
    expect(groups.groups[0]!.name).toBe("Office");

    await fireEvent.click(screen.getByRole("button", { name: "Delete Office" }));
    expect(screen.getByRole("alert").textContent).toContain("Delete “Office”?");
    await fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(groups.groups).toHaveLength(0);
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "New group…" }));
    expect(ongroupschange).toHaveBeenCalledTimes(3);
  });
});

describe("CommandPalette", () => {
  it("filters as you type and runs the chosen command", async () => {
    const { default: CommandPalette } = await import("../components/CommandPalette.svelte");
    const ran: string[] = [];
    const onclose = vi.fn();
    const command = (id: string, title: string) => ({ id, title, section: "Test", run: () => ran.push(id) });
    render(CommandPalette, {
      commands: [command("today", "Go to today"), command("week", "Week view"), command("month", "Month view")],
      onclose,
    });
    const input = screen.getByRole("combobox");
    expect(document.activeElement).toBe(input);
    expect(screen.getAllByRole("option")).toHaveLength(3);
    await fireEvent.input(input, { target: { value: "view" } });
    expect(screen.getAllByRole("option").map((o) => o.querySelector(".title")?.textContent)).toEqual([
      "Week view",
      "Month view",
    ]);
    await fireEvent.keyDown(input, { key: "ArrowDown" });
    expect(input.getAttribute("aria-activedescendant")).toBe("palette-month");
    await fireEvent.keyDown(input, { key: "Enter" });
    expect(ran).toEqual(["month"]);
    expect(onclose).toHaveBeenCalled();
  });

  it("closes with Escape and shows when nothing matches", async () => {
    const { default: CommandPalette } = await import("../components/CommandPalette.svelte");
    const onclose = vi.fn();
    render(CommandPalette, { commands: [{ id: "a", title: "Go to today", section: "Test", run: noop }], onclose });
    const input = screen.getByRole("combobox");
    await fireEvent.input(input, { target: { value: "qqq" } });
    expect(screen.getByText("No matching command")).toBeTruthy();
    await fireEvent.keyDown(input, { key: "Enter" });
    await fireEvent.keyDown(input, { key: "Escape" });
    expect(onclose).toHaveBeenCalledTimes(1);
  });
});

describe("Choosing a calendar for new events", () => {
  const accounts = [
    { id: "local", name: "On this computer", provider: "local" as const },
    { id: "nc", name: "chris @ cloud", provider: "caldav" as const },
  ];
  const cals = [
    { id: "local-personal", accountId: "local", name: "Personal", color: "#2f9e44", readOnly: false },
    { id: "nc/personal", accountId: "nc", name: "Personal", color: "#0082c9", readOnly: false },
  ];

  it("groups the editor's calendars by account and starts at the default", () => {
    const start = parseIsoDate("2026-06-16").getTime() + 9 * 3_600_000;
    render(EventEditor, {
      event: null,
      timing: { kind: "timed", start, end: start + 3_600_000, timeZone: "Europe/Berlin" },
      calendars: cals,
      accounts,
      defaultCalendarId: "nc/personal",
      readOnly: false,
      onsave: () => null,
      ondelete: noop,
      onclose: noop,
    });
    const select = screen.getByLabelText("Calendar") as HTMLSelectElement;
    expect(select.value).toBe("nc/personal");
    expect([...select.querySelectorAll("optgroup")].map((g) => g.label)).toEqual(["On this computer", "chris @ cloud"]);
  });

  it("is set in Settings", async () => {
    const { default: SettingsDialog } = await import("../components/SettingsDialog.svelte");
    const calendars = new CalendarsStore();
    calendars.load(accounts, cals);
    const ondefaultcalendar = vi.fn();
    render(SettingsDialog, {
      calendars,
      groups: new GroupsStore(),
      themePreference: "system",
      ontheme: noop,
      ontogglecalendar: noop,
      onsidebar: noop,
      oncolor: noop,
      ongroupschange: noop,
      onshortcuts: noop,
      onclose: noop,
      defaultCalendarId: "local-personal",
      ondefaultcalendar,
    });
    const select = screen.getByLabelText("New events go to") as HTMLSelectElement;
    expect(select.value).toBe("local-personal");
    await fireEvent.change(select, { target: { value: "nc/personal" } });
    expect(ondefaultcalendar).toHaveBeenCalledWith("nc/personal");
  });
});
