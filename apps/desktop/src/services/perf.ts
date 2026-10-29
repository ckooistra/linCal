import { flushSync } from "svelte";

/**
 * Lightweight interaction timing.
 *
 * `measureInteraction` records two numbers per interaction:
 *  - `script`: JS time of the action including Svelte's DOM updates
 *    (flushed synchronously), i.e. everything except browser style/layout/paint;
 *  - `paint`: time from the start of the action until the frame showing its
 *    result has been produced: a requestAnimationFrame callback followed by a
 *    MessageChannel task, which runs once that frame's style, layout and
 *    paint work is done (a double rAF would add up to a whole extra frame).
 * `paint` is the user-perceived latency and the number compared with targets.
 * WebKit clamps `performance.now()` to 1 ms, so single samples are coarse.
 */

export interface Sample {
  script: number;
  /** Script plus forced style recalculation and layout. */
  layout?: number;
  paint: number;
}

export interface Stats {
  name: string;
  count: number;
  p50: number;
  p95: number;
  max: number;
  mean: number;
  scriptP50: number;
  layoutP50: number;
}

const CAPACITY = 200;

export class PerfLog {
  readonly #samples = new Map<string, Sample[]>();
  #listeners = new Set<() => void>();

  record(name: string, sample: Sample): void {
    let list = this.#samples.get(name);
    if (!list) this.#samples.set(name, (list = []));
    list.push(sample);
    if (list.length > CAPACITY) list.shift();
    for (const l of this.#listeners) l();
  }

  stats(): Stats[] {
    return [...this.#samples].map(([name, samples]) => summarize(name, samples));
  }

  clear(): void {
    this.#samples.clear();
    for (const l of this.#listeners) l();
  }

  subscribe(listener: () => void): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }
}

export function summarize(name: string, samples: readonly Sample[]): Stats {
  const paint = samples.map((s) => s.paint).sort((a, b) => a - b);
  const script = samples.map((s) => s.script).sort((a, b) => a - b);
  return {
    name,
    count: samples.length,
    p50: percentile(paint, 0.5),
    p95: percentile(paint, 0.95),
    max: paint.at(-1) ?? 0,
    mean: paint.length ? paint.reduce((a, b) => a + b, 0) / paint.length : 0,
    scriptP50: percentile(script, 0.5),
    layoutP50: percentile(samples.map((s) => s.layout ?? s.script).sort((a, b) => a - b), 0.5),
  };
}

/** Nearest-rank percentile of an ascending array. */
export function percentile(sorted: readonly number[], p: number): number {
  if (sorted.length === 0) return 0;
  const rank = Math.min(sorted.length - 1, Math.max(0, Math.ceil(p * sorted.length) - 1));
  return sorted[rank]!;
}

export const perfLog = new PerfLog();

function frame(): Promise<number> {
  if (typeof requestAnimationFrame !== "function") {
    return new Promise((resolve) => setTimeout(() => resolve(performance.now()), 16));
  }
  return new Promise((resolve) => requestAnimationFrame(resolve));
}

/** Resolves once the next frame has been rendered. */
export function nextPaint(): Promise<void> {
  if (typeof requestAnimationFrame !== "function" || typeof MessageChannel !== "function") {
    return new Promise((resolve) => setTimeout(resolve, 16));
  }
  return new Promise((resolve) => {
    requestAnimationFrame(() => {
      const channel = new MessageChannel();
      channel.port1.onmessage = () => {
        channel.port1.close();
        resolve();
      };
      channel.port2.postMessage(null);
    });
  });
}

export async function measureInteraction(
  name: string,
  action: () => void | Promise<unknown>,
  log: PerfLog = perfLog,
): Promise<Sample> {
  const start = performance.now();
  const pending = action();
  flushSync();
  const script = performance.now() - start;
  // Reading layout forces style + layout now, separating it from paint.
  if (typeof document !== "undefined") void document.body.offsetHeight;
  const layout = performance.now() - start;
  if (pending) await pending;
  await nextPaint();
  const sample = { script, layout, paint: performance.now() - start };
  log.record(name, sample);
  return sample;
}

export interface FrameStats {
  frames: number;
  durationMs: number;
  fps: number;
  p95FrameMs: number;
  maxFrameMs: number;
  /** Frames longer than 1.5× a 60 Hz frame (25 ms). */
  slowFrames: number;
}

/**
 * Run `onFrame` once per animation frame for `durationMs`, measuring the
 * achieved frame rate. Used to approximate drag/resize rendering cost.
 */
export async function sampleFrames(
  durationMs: number,
  onFrame: (index: number) => void = () => {},
): Promise<FrameStats> {
  const deltas: number[] = [];
  const start = await frame();
  let last = start;
  let index = 0;
  for (;;) {
    onFrame(index++);
    const now = await frame();
    deltas.push(now - last);
    last = now;
    if (now - start >= durationMs) break;
  }
  const sorted = [...deltas].sort((a, b) => a - b);
  const total = last - start;
  return {
    frames: deltas.length,
    durationMs: total,
    fps: total > 0 ? (deltas.length * 1000) / total : 0,
    p95FrameMs: percentile(sorted, 0.95),
    maxFrameMs: sorted.at(-1) ?? 0,
    slowFrames: deltas.filter((d) => d > 25).length,
  };
}

/** Observe long main-thread tasks where the engine supports it (WebKitGTK may not). */
export function observeLongTasks(onTask: (durationMs: number) => void): (() => void) | null {
  if (
    typeof PerformanceObserver === "undefined" ||
    !PerformanceObserver.supportedEntryTypes?.includes("longtask")
  ) {
    return null;
  }
  const observer = new PerformanceObserver((list) => {
    for (const entry of list.getEntries()) onTask(entry.duration);
  });
  observer.observe({ type: "longtask", buffered: false });
  return () => observer.disconnect();
}
