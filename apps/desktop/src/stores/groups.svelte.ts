import type { CalendarId } from "../types/calendar";

/** A named set of calendars whose events are shown together. */
export interface CalendarGroup {
  id: string;
  name: string;
  /** `#rrggbb`; identifies the group only (calendar colours are separate). */
  color: string;
  calendarIds: CalendarId[];
}

/** What the group selector shows: a group, all calendars, or a hand-made selection. */
export type GroupSelection = "all" | "custom" | string;

let fallbackCounter = 0;

function newGroupId(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return `group-${crypto.randomUUID()}`;
  }
  return `group-${Date.now().toString(36)}-${(fallbackCounter++).toString(36)}`;
}

/** Reads stored groups, skipping anything malformed. */
export function parseGroups(value: unknown): CalendarGroup[] {
  if (!Array.isArray(value)) return [];
  const seen = new Set<string>();
  const groups: CalendarGroup[] = [];
  for (const item of value) {
    if (!item || typeof item !== "object") continue;
    const g = item as Record<string, unknown>;
    if (typeof g.id !== "string" || !g.id || seen.has(g.id) || typeof g.name !== "string") continue;
    seen.add(g.id);
    groups.push({
      id: g.id,
      name: g.name,
      color: typeof g.color === "string" && /^#[0-9a-fA-F]{6}$/.test(g.color) ? g.color : "#3b5bdb",
      calendarIds: Array.isArray(g.calendarIds)
        ? [...new Set(g.calendarIds.filter((c): c is string => typeof c === "string"))]
        : [],
    });
  }
  return groups;
}

/**
 * The selection a set of visible calendars corresponds to: every calendar
 * shown is "all"; exactly a group's (existing) calendars is that group;
 * anything else is "custom".
 */
export function selectionFor(
  groups: readonly CalendarGroup[],
  calendarIds: readonly CalendarId[],
  hidden: ReadonlySet<CalendarId>,
): GroupSelection {
  if (calendarIds.every((id) => !hidden.has(id))) return "all";
  const visible = calendarIds.filter((id) => !hidden.has(id));
  for (const g of groups) {
    const members = g.calendarIds.filter((id) => calendarIds.includes(id));
    if (members.length === visible.length && members.every((id) => !hidden.has(id))) return g.id;
  }
  return "custom";
}

/** Calendar groups (user-defined). Persisted by the caller. */
export class GroupsStore {
  groups = $state.raw<CalendarGroup[]>([]);

  load(groups: CalendarGroup[]): void {
    this.groups = groups;
  }

  get(id: string): CalendarGroup | undefined {
    return this.groups.find((g) => g.id === id);
  }

  add(name: string, color: string, calendarIds: CalendarId[]): CalendarGroup {
    const group: CalendarGroup = { id: newGroupId(), name: name.trim(), color, calendarIds: [...new Set(calendarIds)] };
    this.groups = [...this.groups, group];
    return group;
  }

  update(id: string, changes: Partial<Omit<CalendarGroup, "id">>): void {
    this.groups = this.groups.map((g) =>
      g.id === id
        ? {
            ...g,
            ...changes,
            name: (changes.name ?? g.name).trim(),
            calendarIds: [...new Set(changes.calendarIds ?? g.calendarIds)],
          }
        : g,
    );
  }

  remove(id: string): void {
    this.groups = this.groups.filter((g) => g.id !== id);
  }
}
