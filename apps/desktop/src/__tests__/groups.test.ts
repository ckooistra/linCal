import { describe, expect, it } from "vitest";
import { CalendarsStore } from "../stores/calendars.svelte";
import { GroupsStore, parseGroups, selectionFor } from "../stores/groups.svelte";
import { simulatedAccounts, simulatedCalendars } from "../services/mock/generator";

describe("parseGroups", () => {
  it("keeps valid groups and drops or repairs the rest", () => {
    expect(parseGroups(undefined)).toEqual([]);
    expect(parseGroups({ not: "a list" })).toEqual([]);
    expect(
      parseGroups([
        { id: "g1", name: "Work", color: "#112233", calendarIds: ["a", "a", 3, "b"] },
        { id: "g1", name: "Duplicate id" },
        { name: "No id" },
        { id: "g2", name: "Bad colour", color: "red" },
        null,
      ]),
    ).toEqual([
      { id: "g1", name: "Work", color: "#112233", calendarIds: ["a", "b"] },
      { id: "g2", name: "Bad colour", color: "#3b5bdb", calendarIds: [] },
    ]);
  });
});

describe("selectionFor", () => {
  const ids = ["a", "b", "c"];
  const groups = [
    { id: "work", name: "Work", color: "#000000", calendarIds: ["a", "b"] },
    { id: "gone", name: "Gone", color: "#000000", calendarIds: ["a", "removed"] },
  ];
  it("names all, a matching group, or custom", () => {
    expect(selectionFor(groups, ids, new Set())).toBe("all");
    expect(selectionFor(groups, ids, new Set(["c"]))).toBe("work");
    // Calendars that no longer exist are ignored.
    expect(selectionFor(groups, ids, new Set(["b", "c"]))).toBe("gone");
    expect(selectionFor(groups, ids, new Set(["a"]))).toBe("custom");
  });
});

describe("GroupsStore", () => {
  it("adds, updates and removes groups with unique ids", () => {
    const store = new GroupsStore();
    const a = store.add(" Work ", "#123456", ["x", "x", "y"]);
    const b = store.add("Family", "#654321", []);
    expect(a.id).not.toBe(b.id);
    expect(store.get(a.id)).toEqual({ id: a.id, name: "Work", color: "#123456", calendarIds: ["x", "y"] });
    store.update(a.id, { name: "Office", calendarIds: ["y"] });
    expect(store.get(a.id)?.name).toBe("Office");
    expect(store.get(a.id)?.calendarIds).toEqual(["y"]);
    store.remove(b.id);
    expect(store.groups.map((g) => g.name)).toEqual(["Office"]);
  });
});

describe("CalendarsStore display choices", () => {
  function store() {
    const s = new CalendarsStore();
    s.load(simulatedAccounts(), simulatedCalendars());
    return s;
  }

  it("keeps sidebar and event visibility independent", () => {
    const s = store();
    expect(s.sidebarCalendars).toHaveLength(simulatedCalendars().length);
    s.setInSidebar("cal-work", false);
    expect(s.inSidebar("cal-work")).toBe(false);
    expect(s.sidebarChoices()).toEqual({ "cal-work": false });
    s.setVisible("cal-work", true);
    expect(s.isVisible("cal-work")).toBe(true);
    s.setSidebarChoices({ "cal-team": false, "cal-work": true });
    expect(s.inSidebar("cal-work")).toBe(true);
    expect(s.inSidebar("cal-team")).toBe(false);
  });

  it("shows exactly a set of calendars and remembers it as choices", () => {
    const s = store();
    s.setVisibleExactly(["cal-work", "cal-team"]);
    const visible = s.calendars.filter((c) => s.isVisible(c.id)).map((c) => c.id);
    expect(visible).toEqual(["cal-work", "cal-team"]);
    expect(s.choices()["cal-family"]).toBe(false);
    expect(s.choices()["cal-work"]).toBe(true);
  });
});

describe("same-named calendars", () => {
  it("are told apart by account only when names clash", () => {
    const s = new CalendarsStore();
    s.load(
      [
        { id: "local", name: "On this computer", provider: "local" },
        { id: "nc", name: "chris @ cloud.example.org", provider: "caldav" },
        { id: "g", name: "chris@gmail.com (Google)", provider: "google" },
      ],
      [
        { id: "local-personal", accountId: "local", name: "Personal", color: "#2f9e44", readOnly: false },
        { id: "nc/personal", accountId: "nc", name: "Personal", color: "#0082c9", readOnly: false },
        { id: "g/primary", accountId: "g", name: "personal", color: "#9fc6e7", readOnly: false },
        { id: "nc/work", accountId: "nc", name: "Work", color: "#ff0000", readOnly: false },
      ],
    );
    expect(s.label("nc/personal")).toBe("Personal (chris @ cloud.example.org)");
    expect(s.label("g/primary")).toBe("personal (chris@gmail.com (Google))");
    expect(s.accountHint("local-personal")).toBe("On this computer");
    expect(s.label("nc/work")).toBe("Work");
    expect(s.accountHint("nc/work")).toBeNull();
  });
});
