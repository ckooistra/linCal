import type { AccountId, AccountStatus } from "../types/calendar";

/** One-line sync summary of a remote account. */
export interface AccountSummary {
  text: string;
  tone: "ok" | "busy" | "warn" | "error";
  /** Failed or conflicting changes wait for the user (Review…). */
  review: boolean;
  /** The sign-in expired or was revoked (Sign in again…). */
  signIn: boolean;
}

function ago(ms: number): string {
  const s = Math.max(0, Math.round((Date.now() - ms) / 1000));
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 86_400) return `${Math.round(s / 3600)} h ago`;
  return new Date(ms).toLocaleDateString();
}

const plural = (n: number, what: string) => `${n} ${what}${n > 1 ? "s" : ""}`;

/** The summary for an account, or null for accounts that do not sync. */
export function accountSummary(status: AccountStatus | undefined, syncing: boolean): AccountSummary | null {
  if (!status?.remote) return null;
  const extra = { review: status.conflicts + status.failed > 0, signIn: status.needsSignIn };
  const line = (text: string, tone: AccountSummary["tone"]) => ({ text, tone, ...extra });
  if (syncing) return line("Syncing…", "busy");
  if (status.conflicts) return line(`${plural(status.conflicts, "conflict")} to resolve`, "error");
  if (status.error) return line(status.error, status.error.startsWith("Offline") ? "warn" : "error");
  if (status.failed) return line(`${plural(status.failed, "change")} could not be saved`, "error");
  if (status.pending) return line(`${plural(status.pending, "change")} waiting to sync`, "warn");
  const synced = status.lastSyncedAt ? `Synced ${ago(status.lastSyncedAt)}` : "Not synced yet";
  if (status.importFailures) {
    return line(`${synced}; ${plural(status.importFailures, "server event")} could not be read`, "warn");
  }
  if (status.approximated) return line(`${synced}; ${plural(status.approximated, "event")} shown approximately`, "ok");
  return line(synced, "ok");
}

/** Accounts whose state needs the user: errors, conflicts, failed changes, sign-in. */
export function accountsNeedingAttention(
  statuses: Record<AccountId, AccountStatus>,
): AccountId[] {
  return Object.values(statuses)
    .filter((s) => s.remote && (s.error !== null || s.conflicts + s.failed > 0 || s.needsSignIn))
    .map((s) => s.accountId);
}
