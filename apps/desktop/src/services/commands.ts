/**
 * Commands for the command palette (Ctrl+K). The application builds the
 * list from what it can do right now; this module only matches and ranks.
 */

export interface Command {
  /** Stable id (used as the list key and for tests). */
  id: string;
  title: string;
  /** Extra words that should find the command (not shown). */
  keywords?: string;
  /** Shown beside the title, e.g. `Navigation`, `Calendar groups`. */
  section: string;
  /** Keyboard shortcut, for display (e.g. `T`, `Ctrl+F`). */
  shortcut?: string;
  run: () => void;
}

/**
 * How well `query` matches `text`: its letters in order (case-insensitive),
 * preferring contiguous runs and matches at word starts. Null when the
 * letters do not all appear in order.
 */
export function matchScore(text: string, query: string): number | null {
  const t = text.toLowerCase();
  const typed = query.toLowerCase().trim();
  if (!typed) return 0;
  // The words as typed: best at a word start, then earlier.
  let best: number | null = null;
  for (let at = t.indexOf(typed); at >= 0; at = t.indexOf(typed, at + 1)) {
    const wordStart = at === 0 || WORD_BREAK.test(t[at - 1]!);
    const score = 100 + (wordStart ? 20 : 0) - at * 0.1;
    best = Math.max(best ?? score, score);
  }
  if (best !== null) return best;
  const q = typed.replace(/\s+/g, "");
  let score = 0;
  let from = 0;
  let previous = -2;
  for (const ch of q) {
    const at = t.indexOf(ch, from);
    if (at < 0) return null;
    // Contiguous letters and word starts count most; gaps cost a little.
    if (at === previous + 1) score += 3;
    if (at === 0 || WORD_BREAK.test(t[at - 1]!)) score += 4;
    score -= Math.min(at - from, 5) * 0.2;
    previous = at;
    from = at + 1;
  }
  return score;
}

const WORD_BREAK = /[\s\-/:(]/;

/**
 * Commands matching `query`, best first; every word of the query must
 * match the title (or keywords). An empty query keeps the given order.
 */
export function filterCommands(commands: readonly Command[], query: string): Command[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return [...commands];
  const scored: { command: Command; score: number; index: number }[] = [];
  commands.forEach((command, index) => {
    const haystack = `${command.title} ${command.keywords ?? ""}`;
    let total = 0;
    for (const word of words) {
      const inTitle = matchScore(command.title, word);
      const anywhere = inTitle ?? matchScore(haystack, word);
      if (anywhere === null) return;
      // Matches in the title rank above matches in keywords only.
      total += inTitle !== null ? inTitle + 2 : anywhere;
    }
    scored.push({ command, score: total, index });
  });
  scored.sort((a, b) => b.score - a.score || a.index - b.index);
  return scored.map((s) => s.command);
}
