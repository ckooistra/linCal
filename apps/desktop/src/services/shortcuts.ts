export type ShortcutAction =
  | "today"
  | "new-event"
  | "view-month"
  | "view-week"
  | "view-day"
  | "view-agenda"
  | "search"
  | "previous"
  | "next"
  | "toggle-perf"
  | "toggle-hours";

/** True when keyboard input is going into a text field and shortcuts must not fire. */
export function isEditableTarget(target: EventTarget | null): boolean {
  if (!target || typeof (target as Element).closest !== "function") return false;
  const el = target as HTMLElement;
  if (el.isContentEditable) return true;
  return el.closest("input, textarea, select, [contenteditable=''], [contenteditable='true']") !== null;
}

type KeyInput = Pick<
  KeyboardEvent,
  "key" | "ctrlKey" | "metaKey" | "altKey" | "shiftKey" | "target" | "isComposing"
>;

/**
 * Map a keydown to an application action. Single-letter shortcuts are
 * ignored while typing, during IME composition and with modifiers held.
 * Escape is handled by dialogs themselves.
 */
export function resolveShortcut(e: KeyInput): ShortcutAction | null {
  if (e.isComposing) return null;
  const key = e.key.toLowerCase();
  const primary = e.ctrlKey || e.metaKey;
  if (primary && !e.altKey && !e.shiftKey && key === "f") return "search";
  if (primary && e.shiftKey && !e.altKey && key === "p") return "toggle-perf";
  if (primary || e.altKey || isEditableTarget(e.target)) return null;
  switch (key) {
    case "t":
      return "today";
    case "n":
      return "new-event";
    case "m":
      return "view-month";
    case "w":
      return "view-week";
    case "d":
      return "view-day";
    case "a":
      return "view-agenda";
    case "j":
      return "next";
    case "k":
      return "previous";
    case "h":
      return "toggle-hours";
    default:
      return null;
  }
}
