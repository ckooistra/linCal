import { describe, expect, it } from "vitest";
import { filterCommands, matchScore, type Command } from "../services/commands";

const command = (title: string, keywords?: string): Command => ({
  id: title,
  title,
  keywords,
  section: "Test",
  run: () => {},
});

const commands = [
  command("Go to today"),
  command("Next day"),
  command("Next week"),
  command("Previous week"),
  command("Week view", "switch"),
  command("Month view", "switch"),
  command("Open settings", "preferences"),
  command("Manage calendar groups"),
  command("Show group: Work", "activate group"),
  command("Hide Personal", "toggle calendar"),
];
const titles = (query: string) => filterCommands(commands, query).map((c) => c.title);

describe("matchScore", () => {
  it("needs the letters in order, ignoring case", () => {
    expect(matchScore("Week view", "WV")).not.toBeNull();
    expect(matchScore("Week view", "xq")).toBeNull();
    // A word start beats letters inside a word.
    expect(matchScore("Calendar groups", "group")!).toBeGreaterThan(matchScore("Ungrouped", "group")!);
    expect(matchScore("Week view", "")).toBe(0);
  });
});

describe("filterCommands", () => {
  it("keeps the order for an empty query", () => {
    expect(titles("")).toEqual(commands.map((c) => c.title));
  });

  it("matches partial and scattered letters, best first", () => {
    expect(titles("week")[0]).toBe("Week view");
    expect(titles("next wk")).toEqual(["Next week"]);
    expect(titles("nxt wk")).toEqual(["Next week"]);
    expect(titles("wv")[0]).toBe("Week view");
    expect(titles("today")).toEqual(["Go to today"]);
    expect(titles("zzz")).toEqual([]);
  });

  it("finds commands by their keywords, after title matches", () => {
    expect(titles("preferences")).toEqual(["Open settings"]);
    expect(titles("group").slice(0, 2).sort()).toEqual(["Manage calendar groups", "Show group: Work"]);
    expect(titles("toggle")).toEqual(["Hide Personal"]);
  });
});
