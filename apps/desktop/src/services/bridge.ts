/**
 * The only module that talks to Tauri. In Milestone 1 it carries nothing but
 * benchmark plumbing; calendar commands arrive with the Rust core in M2.
 * Every call degrades gracefully when running in a plain browser (vite dev).
 */

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

async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const core = await import("@tauri-apps/api/core");
  return core.invoke<T>(command, args);
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
