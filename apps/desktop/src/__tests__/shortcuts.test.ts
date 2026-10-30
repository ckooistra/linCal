import { describe, expect, it } from "vitest";
import { resolveShortcut } from "../services/shortcuts";

function key(k: string, extra: Partial<KeyboardEvent> = {}, target: EventTarget | null = document.body) {
  return { key: k, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, isComposing: false, target, ...extra };
}

describe("resolveShortcut", () => {
  it.each([
    ["t", "today"],
    ["n", "new-event"],
    ["m", "view-month"],
    ["w", "view-week"],
    ["d", "view-day"],
    ["T", "today"],
    ["h", "toggle-hours"],
  ])("maps %s to %s", (k, action) => {
    expect(resolveShortcut(key(k))).toBe(action);
  });

  it("maps Ctrl+F to search even inside inputs", () => {
    const input = document.createElement("input");
    expect(resolveShortcut(key("f", { ctrlKey: true }, input))).toBe("search");
  });

  it("ignores letters typed into form fields", () => {
    for (const tag of ["input", "textarea", "select"]) {
      const el = document.createElement(tag);
      expect(resolveShortcut(key("t", {}, el))).toBeNull();
    }
    const editable = document.createElement("div");
    editable.setAttribute("contenteditable", "true");
    const child = document.createElement("span");
    editable.append(child);
    expect(resolveShortcut(key("n", {}, child))).toBeNull();
  });

  it("ignores modified letters and IME composition", () => {
    expect(resolveShortcut(key("t", { ctrlKey: true }))).toBeNull();
    expect(resolveShortcut(key("t", { altKey: true }))).toBeNull();
    expect(resolveShortcut(key("t", { isComposing: true }))).toBeNull();
  });
});
