/**
 * Svelte action for modal dialogs: keeps Tab/Shift+Tab inside the dialog
 * and gives focus back to whatever had it when the dialog closes.
 */
const FOCUSABLE =
  "input:not([disabled]), select:not([disabled]), textarea:not([disabled]), button:not([disabled]), [href], [tabindex]:not([tabindex='-1'])";

export function modal(node: HTMLElement): { destroy(): void } {
  const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  function keydown(e: KeyboardEvent): void {
    if (e.key !== "Tab") return;
    const focusable = [...node.querySelectorAll<HTMLElement>(FOCUSABLE)];
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (!first || !last) {
      e.preventDefault();
      return;
    }
    const inside = node.contains(document.activeElement);
    if (e.shiftKey && (document.activeElement === first || !inside)) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && (document.activeElement === last || !inside)) {
      e.preventDefault();
      first.focus();
    }
  }
  node.addEventListener("keydown", keydown);
  return {
    destroy() {
      node.removeEventListener("keydown", keydown);
      if (previous?.isConnected) previous.focus();
    },
  };
}
