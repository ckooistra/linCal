/**
 * The only module that imports Tauri. Calendar data goes through
 * `TauriEventSource`; this module adds settings, datasets and benchmark
 * hooks. Every call degrades gracefully in a plain browser (vite preview).
 */

import { SourceError } from "./eventSource";

export interface BenchConfig {
  enabled: boolean;
  sizes: number[];
}

export interface MemorySample {
  /** Resident set size of the app process, KiB. */
  appKb: number;
  /** Combined RSS of descendant processes (WebKit web/network processes), KiB. */
  childrenKb: number;
  totalKb: number;
  processes: number;
}

export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

interface CommandError {
  code: string;
  message: string;
}

function isCommandError(value: unknown): value is CommandError {
  return typeof value === "object" && value !== null && "code" in value && "message" in value;
}

/** Invoke a Tauri command; command failures become `SourceError`s with readable messages. */
export async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const core = await import("@tauri-apps/api/core");
  try {
    return await core.invoke<T>(command, args);
  } catch (error) {
    if (isCommandError(error)) throw new SourceError(error.message, { cause: error.code });
    throw new SourceError("The calendar engine did not respond.", { cause: error });
  }
}

export type Settings = Record<string, unknown>;

const LOCAL_SETTINGS_KEY = "uc.settings";

/** Persisted, non-secret preferences (SQLite in the app, localStorage in preview). */
export async function loadSettings(): Promise<Settings> {
  if (isTauri()) return invoke<Settings>("get_settings");
  try {
    return JSON.parse(localStorage.getItem(LOCAL_SETTINGS_KEY) ?? "{}") as Settings;
  } catch {
    return {};
  }
}

export async function saveSetting(key: string, value: unknown): Promise<void> {
  if (isTauri()) return invoke("set_setting", { key, value });
  try {
    const all = JSON.parse(localStorage.getItem(LOCAL_SETTINGS_KEY) ?? "{}") as Settings;
    all[key] = value;
    localStorage.setItem(LOCAL_SETTINGS_KEY, JSON.stringify(all));
  } catch {
    // Preview only; not critical.
  }
}

export async function setCalendarColor(calendarId: string, color: string): Promise<void> {
  if (isTauri()) await invoke("set_calendar_color", { calendarId, color });
}

/** Replace all data with a simulated dataset (desktop app only). */
export async function loadSimulatedDataset(size: number, timeZone: string, anchor?: string): Promise<void> {
  await invoke("load_simulated_dataset", { size, timeZone, anchor: anchor ?? null });
}

export async function getBenchConfig(): Promise<BenchConfig> {
  if (isTauri()) return invoke<BenchConfig>("bench_config");
  const params = new URLSearchParams(location.search);
  const sizes = (params.get("bench") ?? "")
    .split(",")
    .map(Number)
    .filter((n) => Number.isInteger(n) && n > 0);
  return { enabled: params.has("bench"), sizes: sizes.length ? sizes : [100, 1000, 5000] };
}

export async function sampleMemory(): Promise<MemorySample | null> {
  if (!isTauri()) return null;
  return invoke<MemorySample | null>("bench_memory");
}

export async function submitBenchReport(report: unknown): Promise<void> {
  if (isTauri()) {
    await invoke("bench_report", { report });
  } else {
    console.info("[bench] report", JSON.stringify(report, null, 2));
  }
}
