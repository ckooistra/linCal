// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import ImportDialog from "../components/ImportDialog.svelte";
import SettingsDialog from "../components/SettingsDialog.svelte";
import { GroupsStore } from "../stores/groups.svelte";
import { CalendarsStore } from "../stores/calendars.svelte";
import type { ImportReport } from "../types/calendar";

afterEach(cleanup);

const ICS = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n";

function store() {
  const s = new CalendarsStore();
  s.load(
    [
      { id: "local", name: "On this computer", provider: "local" },
      { id: "dav", name: "alice @ cloud.example.org", provider: "caldav" },
    ],
    [
      { id: "local/personal", accountId: "local", name: "Personal", color: "#123456", readOnly: false },
      { id: "dav/personal", accountId: "dav", name: "Personal", color: "#654321", readOnly: false },
      { id: "dav/holidays", accountId: "dav", name: "Holidays", color: "#abcdef", readOnly: true },
    ],
  );
  return s;
}

async function chooseFile(name = "events.ics", text = ICS) {
  const input = screen.getByLabelText("File") as HTMLInputElement;
  const file = new File([text], name, { type: "text/calendar" });
  Object.defineProperty(input, "files", { value: [file], configurable: true });
  await fireEvent.change(input);
}

const report = (r: Partial<ImportReport>): ImportReport => ({
  imported: 0,
  alreadyPresent: 0,
  failed: [],
  adjusted: [],
  unlisted: 0,
  ...r,
});

describe("ImportDialog", () => {
  it("offers writable calendars only, preselecting the default", () => {
    render(ImportDialog, { calendars: store(), defaultCalendarId: "dav/personal", onimport: vi.fn(), onclose: vi.fn() });
    const select = screen.getByLabelText("Import into") as HTMLSelectElement;
    expect(select.value).toBe("dav/personal");
    const options = [...select.options].map((o) => o.value);
    expect(options).toEqual(["local/personal", "dav/personal"]);
    expect(document.activeElement).toBe(screen.getByLabelText("File"));
  });

  it("asks for a file, imports it and shows the summary", async () => {
    const onimport = vi.fn(async () =>
      report({
        imported: 3,
        alreadyPresent: 1,
        failed: [{ title: "Anniversary", reason: "Outlook calendars cannot repeat on extra dates outside a rule" }],
        adjusted: [{ title: "Dentist", reason: "kept only the first reminder (this calendar holds one)" }],
      }),
    );
    render(ImportDialog, { calendars: store(), defaultCalendarId: null, onimport, onclose: vi.fn() });
    await fireEvent.click(screen.getByRole("button", { name: "Import" }));
    expect(onimport).not.toHaveBeenCalled();
    expect(screen.getByRole("alert").textContent).toContain("Choose an .ics file");

    await chooseFile();
    await fireEvent.change(screen.getByLabelText("Import into"), { target: { value: "dav/personal" } });
    await fireEvent.click(screen.getByRole("button", { name: "Import" }));
    expect(onimport).toHaveBeenCalledWith("dav/personal", ICS);
    const status = await screen.findByText(/Imported 3 events/);
    // Same-named calendars are told apart by their account.
    expect(status.textContent).toContain("Personal (alice @ cloud.example.org)");
    expect(status.textContent).toContain("1 event was already in this calendar");
    expect(status.textContent).toContain("1 event could not be imported");
    const list = screen.getByRole("list", { name: "Events skipped or changed" });
    expect(list.textContent).toContain("Anniversary");
    expect(list.textContent).toContain("Dentist");
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Done" }));
  });

  it("shows why a file could not be imported, and closes with Escape", async () => {
    const onclose = vi.fn();
    const onimport = vi.fn(async () => {
      throw new Error("This file is not an iCalendar (.ics) file.");
    });
    render(ImportDialog, { calendars: store(), defaultCalendarId: null, onimport, onclose });
    await chooseFile("notes.txt", "hello");
    await fireEvent.click(screen.getByRole("button", { name: "Import" }));
    expect((await screen.findByRole("alert")).textContent).toContain("not an iCalendar");
    await fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(onclose).toHaveBeenCalled();
  });

  it("says when a file has no events", async () => {
    render(ImportDialog, {
      calendars: store(),
      defaultCalendarId: null,
      onimport: async () => report({}),
      onclose: vi.fn(),
    });
    await chooseFile();
    await fireEvent.click(screen.getByRole("button", { name: "Import" }));
    expect(await screen.findByText("The file contains no events.")).toBeTruthy();
  });
});

describe("Import in Settings", () => {
  const props = (extra = {}) => ({
    calendars: store(),
    groups: new GroupsStore(),
    initialTab: "calendars" as const,
    themePreference: "system" as const,
    ontheme: vi.fn(),
    ontogglecalendar: vi.fn(),
    onsidebar: vi.fn(),
    oncolor: vi.fn(),
    ongroupschange: vi.fn(),
    onshortcuts: vi.fn(),
    onclose: vi.fn(),
    ...extra,
  });

  it("offers importing on the Calendars tab when available", async () => {
    const onimport = vi.fn();
    render(SettingsDialog, props({ onimport }));
    await fireEvent.click(screen.getByRole("button", { name: "Import events from a file…" }));
    expect(onimport).toHaveBeenCalled();
    cleanup();
    // The browser preview cannot import.
    render(SettingsDialog, props());
    expect(screen.queryByRole("button", { name: "Import events from a file…" })).toBeNull();
  });
});
