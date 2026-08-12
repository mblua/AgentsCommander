/**
 * #1283 - terminal instance registry: the bounded four-entry LRU and the
 * exact-once synchronous resource-disposal rule (plan 5.1 / 5.5 / 14.4).
 *
 * This deep module owns the terminal map, LRU recency, DOM removal, Xterm
 * disposal, WebGL disposal, per-entry timers, and the exact-once destruction
 * order:
 *
 *   1. scheduled-work and listener cleanup (timers cancelled),
 *   2. admission seal/drop (injected `beforeResourceDispose` hook: mark any
 *      replay/write gate inert and clear its sole strong state owner BEFORE
 *      zeroing pending/in-flight counters and releasing raw bytes),
 *   3. WebGL addon disposal (exactly once, if present),
 *   4. `terminal.dispose()`,
 *   5. container removal,
 *   6. map deletion.
 *
 * No retired entry, timer, terminal reference, or strong gate owner survives
 * the synchronous return of `remove`/`disposeAll`; every late callback owns
 * only weak/inert token state.
 *
 * TerminalView remains the orchestration module: it supplies the Terminal/
 * addon/status factory, the admission-dispose hook, and the viewport policy.
 * This module never imports TerminalView, a sidebar module, `src/shared/ipc.ts`,
 * or `@tauri-apps/api`.
 */
import type { FitAddon } from "@xterm/addon-fit";
import type { WebglAddon } from "@xterm/addon-webgl";
import type { Terminal } from "@xterm/xterm";
import type { PtyViewport } from "../../shared/types";

/** Plan 5.5 / Section 9: at most four retained terminal instances. */
export const TERMINAL_RETENTION_LIMIT = 4;

export interface SessionTerminalEntry {
  readonly sessionId: string;
  readonly container: HTMLDivElement;
  readonly terminal: Terminal;
  readonly fitAddon: FitAddon;
  webglAddon: WebglAddon | null;
  readonly replayStatus: HTMLDivElement;
  /** True once snapshot or live output reached xterm (drives status text). */
  hasRenderedOutput: boolean;
  /** Suppresses `onResize` -> pty_resize while a snapshot resize is applied. */
  snapshotResizeSuppressed: boolean;
  inputBuffer: string;
  spawnViewport: PtyViewport | null;
  lastSentViewport: PtyViewport | null;
  spawnDriftReported: boolean;
  resizeRetryTimer: ReturnType<typeof setTimeout> | null;
  resizeRetryAttempts: number;
  /** #1283 replay-pending settle warning timer; cancelled at disposal. */
  snapshotSettleTimer: ReturnType<typeof setTimeout> | null;
  /** #1283 generation health-deadline timer; cancelled at disposal. */
  healthTimer: ReturnType<typeof setTimeout> | null;
  /** LRU recency stamp (monotonic, owned by the registry). */
  lastActivatedAt: number;
  destroyed: boolean;
}

/** Read-only registry metrics (plan 5.6 frontend fields). */
export interface TerminalRegistryMetrics {
  retained: number;
  visible: number;
  webglContexts: number;
  webglContextLosses: number;
  lruEvictions: number;
}

export interface TerminalRegistryOptions {
  /** Lazily resolved host element (assigned at render time). */
  host: () => HTMLElement;
  /** #1283 plan 5.5.2: called synchronously BEFORE WebGL/Xterm/DOM disposal so
   *  admission can mark gates inert and clear their sole strong owners first. */
  beforeResourceDispose: (sessionId: string) => void;
}

export interface TerminalRegistry {
  /** Create (or return) the entry for a session. The factory receives the
   *  registry-created container and returns the mutable per-entry resources. */
  ensure(
    sessionId: string,
    create: (sessionId: string, container: HTMLDivElement) => Omit<
      SessionTerminalEntry,
      "sessionId" | "container" | "lastActivatedAt" | "destroyed"
    >
  ): SessionTerminalEntry;
  get(sessionId: string): SessionTerminalEntry | undefined;
  /** LRU touch + visible mark. Creates the entry with `create` when missing.
   *  Evicts the least recently activated NON-visible entry when retention
   *  would exceed TERMINAL_RETENTION_LIMIT. The visible entry is never evicted
   *  (only it can be pinned in ReplayPending). */
  activate(
    sessionId: string,
    create: (sessionId: string, container: HTMLDivElement) => Omit<
      SessionTerminalEntry,
      "sessionId" | "container" | "lastActivatedAt" | "destroyed"
    >
  ): SessionTerminalEntry;
  getVisible(): string | null;
  setVisible(sessionId: string | null): void;
  /** Full synchronous exact-once teardown (destroy, eviction, unmount). */
  remove(sessionId: string): void;
  disposeAll(): void;
  metrics(): TerminalRegistryMetrics;
  noteWebglContextLoss(): void;
}

export function createTerminalSessionRegistry(
  options: TerminalRegistryOptions
): TerminalRegistry {
  const entries = new Map<string, SessionTerminalEntry>();
  let visibleSessionId: string | null = null;
  let lruClock = 0;
  let webglContextLosses = 0;
  let lruEvictions = 0;

  const teardownEntry = (sessionId: string, countedAsEviction: boolean): void => {
    const entry = entries.get(sessionId);
    if (!entry || entry.destroyed) {
      return; // safe duplicate destroy: exactly-once
    }
    entry.destroyed = true;

    // 1. scheduled work / listener cleanup: cancel every per-entry timer.
    if (entry.snapshotSettleTimer !== null) {
      clearTimeout(entry.snapshotSettleTimer);
      entry.snapshotSettleTimer = null;
    }
    if (entry.resizeRetryTimer !== null) {
      clearTimeout(entry.resizeRetryTimer);
      entry.resizeRetryTimer = null;
    }
    if (entry.healthTimer !== null) {
      clearTimeout(entry.healthTimer);
      entry.healthTimer = null;
    }

    // 2. admission seal/drop BEFORE any resource disposal.
    options.beforeResourceDispose(sessionId);

    // 3. WebGL addon disposal (exactly once).
    if (entry.webglAddon !== null) {
      entry.webglAddon.dispose();
      entry.webglAddon = null;
    }

    // 4. Xterm disposal.
    entry.terminal.dispose();

    // 5. DOM removal.
    entry.container.remove();

    // 6. map deletion.
    entries.delete(sessionId);
    if (visibleSessionId === sessionId) {
      visibleSessionId = null;
    }
    if (countedAsEviction) {
      lruEvictions += 1;
    }
  };

  const evictIfOverLimit = (): void => {
    while (entries.size > TERMINAL_RETENTION_LIMIT) {
      let victimId: string | null = null;
      let victimStamp = Number.POSITIVE_INFINITY;
      for (const [sessionId, entry] of entries) {
        if (sessionId === visibleSessionId) {
          continue; // never evict the visible/ReplayPending entry
        }
        if (entry.lastActivatedAt < victimStamp) {
          victimStamp = entry.lastActivatedAt;
          victimId = sessionId;
        }
      }
      if (victimId === null) {
        return; // all remaining entries are pinned; size may exceed the limit
      }
      teardownEntry(victimId, true);
    }
  };

  const registry: TerminalRegistry = {
    ensure(sessionId, create) {
      const existing = entries.get(sessionId);
      if (existing && !existing.destroyed) {
        return existing;
      }

      const container = document.createElement("div");
      container.className = "terminal-instance";
      container.dataset.sessionId = sessionId;
      container.setAttribute("data-ac-testid", `terminal.session.${sessionId}`);
      container.setAttribute("data-ac-role", "surface");
      container.setAttribute("data-ac-session-id", sessionId);
      container.hidden = true;
      options.host().appendChild(container);

      const entry: SessionTerminalEntry = {
        sessionId,
        container,
        lastActivatedAt: 0,
        destroyed: false,
        ...create(sessionId, container),
      };
      entries.set(sessionId, entry);
      return entry;
    },

    get(sessionId) {
      return entries.get(sessionId);
    },

    activate(sessionId, create) {
      const entry = this.ensure(sessionId, create);
      entry.lastActivatedAt = ++lruClock;
      visibleSessionId = sessionId;
      evictIfOverLimit();
      return entry;
    },

    getVisible() {
      return visibleSessionId;
    },

    setVisible(sessionId) {
      visibleSessionId = sessionId;
    },

    remove(sessionId) {
      teardownEntry(sessionId, false);
    },

    disposeAll() {
      for (const sessionId of Array.from(entries.keys())) {
        teardownEntry(sessionId, false);
      }
    },

    metrics() {
      let webglContexts = 0;
      for (const entry of entries.values()) {
        if (entry.webglAddon !== null) {
          webglContexts += 1;
        }
      }
      return {
        retained: entries.size,
        visible: visibleSessionId !== null && entries.has(visibleSessionId) ? 1 : 0,
        webglContexts,
        webglContextLosses,
        lruEvictions,
      };
    },

    noteWebglContextLoss() {
      webglContextLosses += 1;
    },
  };

  return registry;
}
