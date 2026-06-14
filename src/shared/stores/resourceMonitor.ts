import { createSignal } from "solid-js";
import { ResourceMonitorAPI } from "../ipc";
import type { ResourceSnapshot } from "../types";

export interface ResourceMonitorPollOptions {
  activeIntervalMs?: number;
  idleIntervalMs?: number;
  backoffIntervalMs?: number;
  backoffWhenIdle?: boolean;
  keepLastSnapshot?: boolean;
}

interface NormalizedPollOptions {
  activeIntervalMs: number;
  idleIntervalMs: number;
  backoffIntervalMs: number;
  backoffWhenIdle: boolean;
  keepLastSnapshot: boolean;
}

const DEFAULT_POLL_OPTIONS: NormalizedPollOptions = {
  activeIntervalMs: 2_000,
  idleIntervalMs: 10_000,
  backoffIntervalMs: 15_000,
  backoffWhenIdle: true,
  keepLastSnapshot: true,
};

const [snapshot, setSnapshot] = createSignal<ResourceSnapshot | null>(null);
const [loading, setLoading] = createSignal(false);
const [error, setError] = createSignal<string | null>(null);
const [lastUpdatedAt, setLastUpdatedAt] = createSignal<number | null>(null);
const [stale, setStale] = createSignal(false);
const [polling, setPolling] = createSignal(false);

const pollRequests = new Map<number, NormalizedPollOptions>();
let nextPollRequestId = 0;
let timer: ReturnType<typeof setTimeout> | null = null;
let inFlight: Promise<ResourceSnapshot | null> | null = null;
let failureCount = 0;
let visibilityListenerAttached = false;

const normalizeOptions = (
  options?: ResourceMonitorPollOptions
): NormalizedPollOptions => ({
  activeIntervalMs:
    options?.activeIntervalMs ?? DEFAULT_POLL_OPTIONS.activeIntervalMs,
  idleIntervalMs: options?.idleIntervalMs ?? DEFAULT_POLL_OPTIONS.idleIntervalMs,
  backoffIntervalMs:
    options?.backoffIntervalMs ?? DEFAULT_POLL_OPTIONS.backoffIntervalMs,
  backoffWhenIdle:
    options?.backoffWhenIdle ?? DEFAULT_POLL_OPTIONS.backoffWhenIdle,
  keepLastSnapshot:
    options?.keepLastSnapshot ?? DEFAULT_POLL_OPTIONS.keepLastSnapshot,
});

const normalizeError = (err: unknown): string =>
  err instanceof Error ? err.message : String(err);

const documentIsHidden = (): boolean =>
  typeof document !== "undefined" ? document.hidden : false;

const clearPollTimer = () => {
  if (timer) {
    clearTimeout(timer);
    timer = null;
  }
};

const chooseInterval = (): number => {
  if (pollRequests.size === 0) return DEFAULT_POLL_OPTIONS.idleIntervalMs;

  const current = snapshot();
  const hidden = documentIsHidden();
  const idle = (current?.activeAgentGroups ?? 0) === 0;
  const backedOff = failureCount > 0;

  return Math.min(
    ...Array.from(pollRequests.values()).map((options) => {
      if (backedOff) return options.backoffIntervalMs;
      if (hidden || (idle && options.backoffWhenIdle)) {
        return options.idleIntervalMs;
      }
      return options.activeIntervalMs;
    })
  );
};

const shouldKeepLastSnapshot = (): boolean =>
  pollRequests.size === 0 ||
  Array.from(pollRequests.values()).every((options) => options.keepLastSnapshot);

const scheduleNextPoll = (delayMs?: number) => {
  clearPollTimer();
  if (pollRequests.size === 0) {
    setPolling(false);
    return;
  }

  setPolling(true);
  const delay = delayMs ?? chooseInterval();
  timer = setTimeout(() => {
    timer = null;
    void runPollCycle();
  }, Math.max(delay, 250));
};

const onVisibilityChange = () => {
  if (pollRequests.size > 0) scheduleNextPoll(250);
};

const attachVisibilityListener = () => {
  if (visibilityListenerAttached || typeof document === "undefined") return;
  document.addEventListener("visibilitychange", onVisibilityChange);
  visibilityListenerAttached = true;
};

const detachVisibilityListener = () => {
  if (!visibilityListenerAttached || typeof document === "undefined") return;
  document.removeEventListener("visibilitychange", onVisibilityChange);
  visibilityListenerAttached = false;
};

async function runPollCycle() {
  await resourceMonitorStore.refresh();
  scheduleNextPoll();
}

export const resourceMonitorStore = {
  get snapshot() {
    return snapshot();
  },

  get loading() {
    return loading();
  },

  get error() {
    return error();
  },

  get lastUpdatedAt() {
    return lastUpdatedAt();
  },

  get stale() {
    return stale();
  },

  get polling() {
    return polling();
  },

  async refresh(): Promise<ResourceSnapshot | null> {
    if (inFlight) return inFlight;

    setLoading(true);
    inFlight = ResourceMonitorAPI.snapshot()
      .then((next) => {
        setSnapshot(next);
        setLastUpdatedAt(Date.now());
        setError(null);
        setStale(false);
        failureCount = 0;
        return next;
      })
      .catch((err) => {
        setError(normalizeError(err));
        setStale(true);
        if (!shouldKeepLastSnapshot()) {
          setSnapshot(null);
        }
        failureCount += 1;
        return null;
      })
      .finally(() => {
        setLoading(false);
        inFlight = null;
      });

    return inFlight;
  },

  startPolling(options?: ResourceMonitorPollOptions): () => void {
    const id = ++nextPollRequestId;
    pollRequests.set(id, normalizeOptions(options));
    attachVisibilityListener();

    scheduleNextPoll(snapshot() ? undefined : 250);

    return () => {
      pollRequests.delete(id);
      if (pollRequests.size === 0) {
        clearPollTimer();
        detachVisibilityListener();
        setPolling(false);
      } else {
        scheduleNextPoll();
      }
    };
  },

  stopPolling() {
    pollRequests.clear();
    clearPollTimer();
    detachVisibilityListener();
    setPolling(false);
  },
};
