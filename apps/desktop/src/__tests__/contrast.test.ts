/**
 * WCAG 2.2 contrast of the theme tokens (both themes) and of event text on
 * calendar colours: 4.5:1 for text (1.4.3), 3:1 for control borders and
 * the focus ring (1.4.11).
 */
// Vitest stubs CSS imports, so the stylesheet is read from disk. Node's
// types are not part of this browser project; declare what is used.
declare const process: { cwd(): string };
// @ts-expect-error -- node:fs has no type declarations here
import { readFileSync } from "node:fs";

const css: string = (readFileSync as (path: string, encoding: string) => string)(
  `${process.cwd()}/src/app.css`,
  "utf8",
);
import { describe, expect, it } from "vitest";
import { textColorFor } from "../components/calendar/svarMapping";

function tokens(block: string, prefix = "uc"): Record<string, string> {
  const start = css.indexOf(block);
  expect(start, block).toBeGreaterThanOrEqual(0);
  const body = css.slice(start, css.indexOf("}", start));
  const pattern = new RegExp(`--${prefix}-([\\w-]+):\\s*(#[0-9a-fA-F]{6})`, "g");
  return Object.fromEntries([...body.matchAll(pattern)].map((m) => [m[1]!, m[2]!]));
}

function luminance(hex: string): number {
  const n = Number.parseInt(hex.slice(1, 7), 16);
  const channel = (v: number) => {
    const c = v / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel((n >> 16) & 255) + 0.7152 * channel((n >> 8) & 255) + 0.0722 * channel(n & 255);
}

function contrast(a: string, b: string): number {
  const [x, y] = [luminance(a), luminance(b)].sort((p, q) => q - p);
  return (x! + 0.05) / (y! + 0.05);
}

const themes = {
  light: tokens(":root {"),
  dark: tokens(':root[data-theme="dark"] {'),
};

describe.each(Object.entries(themes))("%s theme", (_name, t) => {
  const backgrounds = ["bg", "surface", "surface-alt"];
  it.each([
    ["text", 4.5],
    ["text-muted", 4.5],
    ["danger", 4.5],
    ["accent", 4.5],
  ] as const)("%s text on every background", (fg, min) => {
    for (const bg of backgrounds) {
      expect(contrast(t[fg]!, t[bg]!), `${fg} on ${bg}`).toBeGreaterThanOrEqual(min);
    }
  });

  it("text on accent buttons", () => {
    expect(contrast(t["accent-text"]!, t.accent!)).toBeGreaterThanOrEqual(4.5);
  });

  it("control borders and the focus ring stand out", () => {
    for (const bg of backgrounds) {
      expect(contrast(t["control-border"]!, t[bg]!), `control border on ${bg}`).toBeGreaterThanOrEqual(3);
      expect(contrast(t.focus!, t[bg]!), `focus on ${bg}`).toBeGreaterThanOrEqual(3);
    }
  });
});

describe("calendar grid (SVAR) text", () => {
  // SVAR's own backgrounds (Willow / Willow Dark): plain and alternate.
  it.each([
    [".wx-theme.wx-willow-theme {", ["#ffffff", "#f2f3f7"]],
    [".wx-theme.wx-willow-dark-theme {", ["#2a2b2d", "#384047"]],
  ] as const)("%s", (block, backgrounds) => {
    const t = tokens(block, "wx");
    for (const fg of ["color-font-alt", "color-secondary-font"]) {
      for (const bg of backgrounds) {
        expect(contrast(t[fg]!, bg), `${fg} on ${bg}`).toBeGreaterThanOrEqual(4.5);
      }
    }
  });
});

describe("event text on calendar colours", () => {
  it("reaches 4.5:1 on any colour", () => {
    let worst = Infinity;
    for (let r = 0; r < 256; r += 15) {
      for (let g = 0; g < 256; g += 15) {
        for (let b = 0; b < 256; b += 15) {
          const hex = `#${[r, g, b].map((v) => v.toString(16).padStart(2, "0")).join("")}`;
          const fg = textColorFor(hex) === "#fff" ? "#ffffff" : "#000000";
          worst = Math.min(worst, contrast(fg, hex));
        }
      }
    }
    expect(worst).toBeGreaterThanOrEqual(4.5);
  });
});
