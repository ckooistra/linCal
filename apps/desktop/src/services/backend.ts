/**
 * Chooses where data lives: the Rust engine + SQLite inside the desktop app,
 * or an in-memory mock when the UI runs in a plain browser (preview/tests).
 * Everything above this module works against the same interface.
 */
import type { Account, Calendar } from "../types/calendar";
import {
  isTauri,
  loadSettings,
  loadSimulatedDataset,
  saveSetting,
  setCalendarColor,
  type Settings,
} from "./bridge";
import type { EventSource } from "./eventSource";
import { DEFAULT_SEED, generateDataset } from "./mock/generator";
import { MockEventSource } from "./mock/mockEventSource";
import { TauriEventSource } from "./tauriEventSource";
import { toIsoDate } from "./time";

export const DEFAULT_DATASET_SIZE = 1000;

export interface Backend {
  kind: "desktop" | "preview";
  settings: Settings;
  /** Set when start-up failed in a way the user should see. */
  startupError: string | null;
  source: EventSource;
  /** Present only in preview mode: latency and failure injection. */
  mock: MockEventSource | null;
  loadCalendars(): Promise<{ accounts: Account[]; calendars: Calendar[] }>;
  /** Replace all data with a simulated dataset; returns the source to use. */
  loadDataset(size: number, anchor?: string): Promise<EventSource>;
  setCalendarColor(calendarId: string, color: string): Promise<void>;
  saveSetting(key: string, value: unknown): Promise<void>;
}

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function datasetSize(settings: Settings): number {
  const size = settings.datasetSize;
  return typeof size === "number" && [100, 1000, 5000].includes(size) ? size : DEFAULT_DATASET_SIZE;
}

async function desktopBackend(): Promise<Backend> {
  const source = new TauriEventSource();
  let settings: Settings = {};
  let startupError: string | null = null;
  try {
    settings = await loadSettings();
    // First run: seed a simulated dataset so there is something to look at.
    if ((await source.listAccounts()).length === 0) {
      await loadSimulatedDataset(datasetSize(settings), source.timeZone);
    }
  } catch (error) {
    startupError = message(error);
  }
  return {
    kind: "desktop",
    settings,
    startupError,
    source,
    mock: null,
    async loadCalendars() {
      const [accounts, calendars] = await Promise.all([source.listAccounts(), source.listCalendars()]);
      return { accounts, calendars };
    },
    async loadDataset(size, anchor) {
      await loadSimulatedDataset(size, source.timeZone, anchor);
      return source;
    },
    setCalendarColor,
    saveSetting,
  };
}

function previewBackend(settings: Settings): Backend {
  let mock = createMock(datasetSize(settings));
  function createMock(size: number, anchor = toIsoDate(new Date())): MockEventSource {
    return new MockEventSource(generateDataset({ seed: DEFAULT_SEED, eventCount: size, anchor }));
  }
  const backend: Backend = {
    kind: "preview",
    settings,
    startupError: null,
    source: mock,
    mock,
    loadCalendars: async () => ({ accounts: await mock.listAccounts(), calendars: await mock.listCalendars() }),
    async loadDataset(size, anchor) {
      const latency = mock.latencyMs;
      mock = createMock(size, anchor);
      mock.latencyMs = latency;
      backend.source = mock;
      backend.mock = mock;
      return mock;
    },
    setCalendarColor: async () => {},
    saveSetting,
  };
  return backend;
}

export async function createBackend(): Promise<Backend> {
  if (isTauri()) return desktopBackend();
  return previewBackend(await loadSettings());
}

