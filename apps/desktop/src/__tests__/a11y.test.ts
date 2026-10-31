/**
 * Automated accessibility checks (axe-core) of the main components in
 * their typical states. Colour contrast is checked separately
 * (`contrast.test.ts`): jsdom does not compute styles.
 */
import { afterEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render } from "@testing-library/svelte";
import axe from "axe-core";
import type { Component } from "svelte";
import AddAccountDialog from "../components/AddAccountDialog.svelte";
import AgendaList from "../components/AgendaList.svelte";
import ConfirmDialog from "../components/ConfirmDialog.svelte";
import EventEditor from "../components/EventEditor.svelte";
import OAuthAccountDialog from "../components/OAuthAccountDialog.svelte";
import RepeatEditor from "../components/RepeatEditor.svelte";
import ShortcutsDialog from "../components/ShortcutsDialog.svelte";
import Sidebar from "../components/Sidebar.svelte";
import SubscriptionDialog from "../components/SubscriptionDialog.svelte";
import SyncProblemsDialog from "../components/SyncProblemsDialog.svelte";
import Toasts from "../components/Toasts.svelte";
import Toolbar from "../components/Toolbar.svelte";
import { agendaDays } from "../services/agenda";
import { simulatedAccounts, simulatedCalendars } from "../services/mock/generator";
import { defaultForm } from "../services/recurrence";
import { parseIsoDate } from "../services/time";
import { CalendarsStore } from "../stores/calendars.svelte";
import { Notifications } from "../stores/notifications.svelte";
import type { CalendarEvent } from "../types/calendar";

const noop = () => {};

async function violations(container: Element): Promise<string[]> {
  const result = await axe.run(container, {
    rules: { "color-contrast": { enabled: false } },
  });
  return result.violations.flatMap((v) =>
    v.nodes.map((n) => `${v.id}: ${n.target.join(" ")} — ${n.failureSummary?.split("\n")[1]?.trim() ?? v.help}`),
  );
}

// Props are checked by each component's own tests; here any props object is fine.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
async function check(component: Component<any>, props: Record<string, unknown>): Promise<string[]> {
  const { container } = render(component, { props } as never);
  return violations(container);
}

function calendars(): CalendarsStore {
  const store = new CalendarsStore();
  store.load(simulatedAccounts(), simulatedCalendars());
  store.setChoices({ "cal-work": true });
  return store;
}

const start = parseIsoDate("2026-06-16").getTime() + 9 * 3_600_000;
const event: CalendarEvent = {
  id: "s::1",
  calendarId: "cal-work",
  title: "Stand-up",
  description: "",
  location: "Room 4",
  timing: { kind: "timed", start, end: start + 900_000, timeZone: "Europe/Berlin" },
  seriesId: "s",
  recurrenceId: "1",
  rrule: "FREQ=WEEKLY",
  reminders: [10],
  syncStatus: "conflict",
  revision: 1,
};

afterEach(cleanup);

describe("accessibility (axe)", () => {
  const cases: [string, () => Promise<string[]>][] = [
    [
      "Sidebar",
      () =>
        check(Sidebar, {
          calendars: calendars(),
          datasetSize: 1000,
          latencyMs: 0,
          themePreference: "system",
          systemTitleBar: false,
          onsystemtitlebar: noop,
          notifications: true,
          onnotifications: noop,
          simulationControls: true,
          onaddaccount: noop,
          ontogglecalendar: noop,
          oncolor: noop,
          ondataset: noop,
          onlatency: noop,
          ontheme: noop,
          onfailnext: noop,
        } as never),
    ],
    [
      "Toolbar",
      () =>
        check(Toolbar, {
          label: "June 2026",
          view: "week",
          loading: true,
          search: "",
          hours: "work",
          onhours: noop,
          onnavigate: noop,
          onview: noop,
          onnew: noop,
          onsearch: noop,
          onsearchsubmit: noop,
          onhelp: noop,
        } as never),
    ],
    [
      "EventEditor (conflict, recurring)",
      () =>
        check(EventEditor, {
          event,
          timing: event.timing,
          calendars: simulatedCalendars(),
          defaultCalendarId: null,
          readOnly: false,
          onsave: () => null,
          ondelete: noop,
          onclose: noop,
          onresolve: noop,
          onloadversions: async () => null,
        } as never),
    ],
    [
      "EventEditor (read-only)",
      () =>
        check(EventEditor, {
          event: { ...event, syncStatus: "synced", seriesId: null },
          timing: event.timing,
          calendars: simulatedCalendars(),
          defaultCalendarId: null,
          readOnly: true,
          onsave: () => null,
          ondelete: noop,
          onclose: noop,
        } as never),
    ],
    [
      "RepeatEditor",
      () => check(RepeatEditor, { form: { ...defaultForm(new Date(start)), end: { kind: "count", count: 3 } }, start: new Date(start) } as never),
    ],
    [
      "AgendaList",
      () =>
        check(AgendaList, {
          days: agendaDays([event], { start: start - 86_400_000, end: start + 86_400_000 }, { spread: true, hour12: false }),
          calendars: calendars(),
          today: "2026-06-16",
          onopen: noop,
        } as never),
    ],
    ["ShortcutsDialog", () => check(ShortcutsDialog, { onclose: noop })],
    ["AddAccountDialog", () => check(AddAccountDialog, { onsubmit: async () => null, onclose: noop } as never)],
    ["SubscriptionDialog", () => check(SubscriptionDialog, { onsubmit: async () => null, onclose: noop } as never)],
    [
      "OAuthAccountDialog",
      () => check(OAuthAccountDialog, { provider: "google", builtInClientId: null, onsubmit: async () => null, onclose: noop } as never),
    ],
    [
      "ConfirmDialog",
      () => check(ConfirmDialog, { title: "Remove?", message: "Really?", confirmLabel: "Remove", onconfirm: noop, oncancel: noop } as never),
    ],
    [
      "SyncProblemsDialog",
      () =>
        check(SyncProblemsDialog, {
          problems: [
            { accountId: "a", calendarId: "c", href: "h", title: "Lunch", deletion: false, conflict: true, message: "changed on the server" },
            { accountId: "a", calendarId: "c", href: "h2", title: "Old", deletion: true, conflict: false, message: "refused" },
          ],
          onresolve: noop,
          onclose: noop,
        } as never),
    ],
  ];

  it.each(cases)("%s", async (_name, run) => {
    expect(await run()).toEqual([]);
  });

  it("Toasts", async () => {
    const notifications = new Notifications();
    notifications.push("Saved.", "info");
    notifications.push("Something failed.");
    expect(await check(Toasts, { notifications } as never)).toEqual([]);
    notifications.dispose();
  });

  it("EventEditor with the custom repeat fields open", async () => {
    const { container, getByLabelText } = render(EventEditor, {
      props: {
        event: null,
        timing: event.timing,
        calendars: simulatedCalendars(),
        defaultCalendarId: "cal-work",
        readOnly: false,
        onsave: () => null,
        ondelete: noop,
        onclose: noop,
      } as never,
    });
    await fireEvent.change(getByLabelText("Repeat"), { target: { value: "custom" } });
    expect(await violations(container)).toEqual([]);
  });
});
