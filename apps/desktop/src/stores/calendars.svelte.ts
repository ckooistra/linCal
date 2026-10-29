import type { Account, AccountId, AccountStatus, Calendar, CalendarId } from "../types/calendar";

/** Accounts, calendars and per-calendar display settings (visibility, colour). */
export class CalendarsStore {
  accounts = $state.raw<Account[]>([]);
  calendars = $state.raw<Calendar[]>([]);
  /** Hidden calendar ids. Replaced (not mutated) so dependents update once. */
  hidden = $state.raw<ReadonlySet<CalendarId>>(new Set());

  #byId = $derived(new Map(this.calendars.map((c) => [c.id, c])));

  writable = $derived(this.calendars.filter((c) => !c.readOnly));

  /** Sync status per account (remote accounts only matter). */
  statuses = $state.raw<Record<AccountId, AccountStatus>>({});
  /** Accounts with a sync pass in progress. */
  syncing = $state.raw<ReadonlySet<AccountId>>(new Set());

  setStatuses(list: AccountStatus[]): void {
    this.statuses = Object.fromEntries(list.map((s) => [s.accountId, s]));
  }

  setSyncing(accountId: AccountId, on: boolean): void {
    if (on === this.syncing.has(accountId)) return;
    const next = new Set(this.syncing);
    if (on) next.add(accountId);
    else next.delete(accountId);
    this.syncing = next;
  }

  /** Explicit user choices; calendars without one use `defaultVisible`. */
  #choices: Record<CalendarId, boolean> = {};

  /** Simulated demo calendars start hidden; real calendars start visible. */
  defaultVisible(calendar: Calendar): boolean {
    return this.accounts.find((a) => a.id === calendar.accountId)?.provider !== "simulated";
  }

  /** Restores saved visibility choices. */
  setChoices(choices: Record<CalendarId, boolean>): void {
    this.#choices = { ...choices };
    this.#recompute();
  }

  /** Visibility choices to persist. */
  choices(): Record<CalendarId, boolean> {
    return { ...this.#choices };
  }

  #recompute(): void {
    this.hidden = new Set(
      this.calendars.filter((c) => !(this.#choices[c.id] ?? this.defaultVisible(c))).map((c) => c.id),
    );
  }

  load(accounts: Account[], calendars: Calendar[]): void {
    this.accounts = accounts;
    this.calendars = calendars;
    this.#recompute();
  }

  get(id: CalendarId): Calendar | undefined {
    return this.#byId.get(id);
  }

  isVisible(id: CalendarId): boolean {
    return !this.hidden.has(id);
  }

  /** Unknown calendars are treated as read-only. */
  isReadOnly(id: CalendarId): boolean {
    return this.#byId.get(id)?.readOnly ?? true;
  }

  setVisible(id: CalendarId, visible: boolean): void {
    this.#choices[id] = visible;
    if (visible === this.isVisible(id)) return;
    const next = new Set(this.hidden);
    if (visible) next.delete(id);
    else next.add(id);
    this.hidden = next;
  }

  toggle(id: CalendarId): void {
    this.setVisible(id, !this.isVisible(id));
  }

  setColor(id: CalendarId, color: string): void {
    if (!/^#[0-9a-fA-F]{6}$/.test(color)) return;
    this.calendars = this.calendars.map((c) => (c.id === id ? { ...c, color } : c));
  }
}
