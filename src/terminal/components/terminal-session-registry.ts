/**
 * #1283 - terminal instance registry: the bounded four-entry LRU and the
 * exact-once synchronous resource-disposal rule (plan 5.1 / 5.5 / 14.4).
 *
 * This deep module owns the terminal map, LRU recency, DOM removal, Xterm
 * disposal, WebGL disposal, per-entry timers, and the exact-once destruction
 * order:
 *
 *   1. scheduled-work and listener cleanup (timers cancelled),
 *   2. the injected `beforeResourceDispose` hook (TerminalView releases this
 *      window's output attachment there, because the dependency-cruiser rule
 *      `no-terminal-helper-back-edge` forbids this module from reaching
 *      `src/shared/ipc.ts`),
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
 * addon/status factory, the pre-dispose hook, and the viewport policy.
 * This module never imports TerminalView, a sidebar module, `src/shared/ipc.ts`,
 * or `@tauri-apps/api`.
 */
import type { FitAddon } from "@xterm/addon-fit";
import type { WebglAddon } from "@xterm/addon-webgl";
import type { Terminal } from "@xterm/xterm";
import type {
  PtyOutputEvent,
  PtyTerminalAttachContextState,
  PtyTerminalAttachRenderer,
  PtyViewport,
} from "../../shared/types";

/** Plan 5.5 / Section 9: at most four retained terminal instances. */
export const TERMINAL_RETENTION_LIMIT = 4;

export interface TerminalResizeOperation {
  readonly token: number;
  readonly generation: number | null;
  readonly viewport: PtyViewport;
}

export type TerminalObservationProgress = "none" | "postWrite" | "postFit" | "terminal";

export interface SessionTerminalEntry {
  readonly sessionId: string;
  readonly xtermInstanceId: number;
  readonly container: HTMLDivElement;
  readonly terminal: Terminal;
  readonly fitAddon: FitAddon;
  webglAddon: WebglAddon | null;
  renderer: PtyTerminalAttachRenderer;
  contextState: PtyTerminalAttachContextState;
  readonly replayStatus: HTMLDivElement;
  /** True once snapshot or live output reached xterm (drives status text). */
  hasRenderedOutput: boolean;
  /** Suppresses `onResize` -> pty_resize while a snapshot resize is applied. */
  snapshotResizeSuppressed: boolean;
  inputBuffer: string;
  spawnViewport: PtyViewport | null;
  /** Last viewport whose backend resize promise actually resolved. */
  confirmedViewport: PtyViewport | null;
  resizeOperationToken: number;
  inFlightResize: TerminalResizeOperation | null;
  spawnDriftReported: boolean;
  resizeRetryTimer: ReturnType<typeof setTimeout> | null;
  resizeRetryAttempts: number;
  resizeRetryExhaustion: {
    readonly generation: number | null;
    readonly viewport: PtyViewport;
  } | null;
  attachmentDeadlineTimer: ReturnType<typeof setTimeout> | null;
  firstAttachmentRaf: number | null;
  secondAttachmentRaf: number | null;
  ordinaryViewportRaf: number | null;
  attachGeneration: number | null;
  attachmentAbortController: AbortController | null;
  bottomSettledGeneration: number | null;
  attachmentSettlePending: boolean;
  deferredViewportSync: boolean;
  observationProgress: TerminalObservationProgress;
  observationChain: Promise<void>;
  /** #961 seed/reconcile state. While an attach is in flight the live events
   *  are BOTH written and retained, so the seed that lands afterwards can be
   *  applied by reset-then-replay instead of gating the live bytes behind it. */
  snapshotReplayPending: boolean;
  pendingSnapshotEvents: PtyOutputEvent[];
  pendingSnapshotBytes: number;
  snapshotReconcileDiscarded: boolean;
  /** Watermark: the highest sequence already on screen. `null` until the first
   *  sequenced write (browser mode never sets it). */
  lastAppliedSequence: number | null;
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
  /** Called synchronously BEFORE WebGL/Xterm/DOM disposal, so TerminalView can
   *  release this window's output attachment while the entry still exists. */
  beforeResourceDispose: (sessionId: string) => void;
}

export interface TerminalRegistry {
  /** Create (or return) the entry for a session. The factory receives the
   *  registry-created container and returns the mutable per-entry resources. */
  ensure(
    sessionId: string,
    create: (sessionId: string, container: HTMLDivElement) => Omit<
      SessionTerminalEntry,
      "sessionId" | "xtermInstanceId" | "container" | "lastActivatedAt" | "destroyed"
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
      "sessionId" | "xtermInstanceId" | "container" | "lastActivatedAt" | "destroyed"
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
  let instanceClock = 0;
  let webglContextLosses = 0;
  let lruEvictions = 0;

  const teardownEntry = (sessionId: string, countedAsEviction: boolean): void => {
    const entry = entries.get(sessionId);
    if (!entry || entry.destroyed) {
      return; // safe duplicate destroy: exactly-once
    }
    entry.destroyed = true;

    // 1. scheduled work / listener cleanup: cancel every per-entry timer.
    if (entry.resizeRetryTimer !== null) {
      clearTimeout(entry.resizeRetryTimer);
      entry.resizeRetryTimer = null;
    }
    if (entry.attachmentDeadlineTimer !== null) {
      clearTimeout(entry.attachmentDeadlineTimer);
      entry.attachmentDeadlineTimer = null;
    }
    if (entry.firstAttachmentRaf !== null) {
      cancelAnimationFrame(entry.firstAttachmentRaf);
      entry.firstAttachmentRaf = null;
    }
    if (entry.secondAttachmentRaf !== null) {
      cancelAnimationFrame(entry.secondAttachmentRaf);
      entry.secondAttachmentRaf = null;
    }
    if (entry.ordinaryViewportRaf !== null) {
      cancelAnimationFrame(entry.ordinaryViewportRaf);
      entry.ordinaryViewportRaf = null;
    }
    entry.attachmentAbortController?.abort();
    entry.attachmentAbortController = null;
    entry.resizeOperationToken += 1;
    entry.inFlightResize = null;
    entry.attachmentSettlePending = false;
    entry.deferredViewportSync = false;
    entry.snapshotReplayPending = false;
    entry.pendingSnapshotEvents = [];
    entry.pendingSnapshotBytes = 0;

    // 2. the orchestrator's pre-dispose hook, BEFORE any resource disposal.
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

      if (instanceClock >= 4_294_967_295) {
        container.remove();
        throw new Error("Terminal xterm instance id space exhausted");
      }
      instanceClock += 1;

      const entry: SessionTerminalEntry = {
        sessionId,
        xtermInstanceId: instanceClock,
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
        if (entry.renderer === "webgl" && entry.contextState === "active") {
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
