export type NotificationKind = "error" | "info";

export interface Notification {
  id: number;
  kind: NotificationKind;
  message: string;
}

const AUTO_DISMISS_MS = 6000;

/** Transient, user-facing messages (failed saves, read-only edits, …). */
export class Notifications {
  items = $state.raw<Notification[]>([]);
  #nextId = 1;
  readonly #timers = new Map<number, ReturnType<typeof setTimeout>>();

  push(message: string, kind: NotificationKind = "error"): void {
    const id = this.#nextId++;
    this.items = [...this.items, { id, kind, message }].slice(-4);
    this.#timers.set(
      id,
      setTimeout(() => this.dismiss(id), AUTO_DISMISS_MS),
    );
  }

  dismiss(id: number): void {
    clearTimeout(this.#timers.get(id));
    this.#timers.delete(id);
    this.items = this.items.filter((n) => n.id !== id);
  }

  dispose(): void {
    for (const timer of this.#timers.values()) clearTimeout(timer);
    this.#timers.clear();
  }
}
