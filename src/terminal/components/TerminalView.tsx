import { Component, createEffect, onCleanup, onMount } from "solid-js";
import { Terminal } from "@xterm/xterm";
import { WebglAddon } from "@xterm/addon-webgl";
import { FitAddon } from "@xterm/addon-fit";
import {
  PtyAPI,
  SessionAPI,
  TerminalOutputAPI,
  onPtyOutput,
  onSessionAgentMessage,
  onSessionDestroyed,
  onTerminalDetached,
} from "../../shared/ipc";
import { isBrowser, isTauri } from "../../shared/platform";
import { formatAgentMessageStamp } from "../../shared/time-format";
import {
  registerPtyViewportProbe,
  registerUiTerminalController,
  takeSpawnViewport,
} from "../../shared/terminal-viewport";
import { terminalStore } from "../stores/terminal";
import {
  MAIN_TERMINAL_LAYOUT_PULSE_REQUEST_EVENT,
  type MainTerminalLayoutGeometry,
  type MainTerminalLayoutObserverAck,
  type MainTerminalLayoutPulsePhaseTrace,
  type MainTerminalLayoutPulseRequest,
  type MainTerminalLayoutPulseResult,
  type MainTerminalLayoutPulseSample,
  type MainTerminalLayoutPulseTrace,
  PtyOutputEvent,
  PtyScreenSnapshot,
  PtyViewport,
} from "../../shared/types";
import type { UnlistenFn } from "../../shared/transport";
import { updatePromptCapture } from "./prompt-input-capture";
import { createTerminalOptions } from "./terminal-options";
import {
  createTerminalSessionRegistry,
  type SessionTerminalEntry,
  type TerminalRegistry,
} from "./terminal-session-registry";
import "@xterm/xterm/css/xterm.css";

interface TerminalViewProps {
  lockedSessionId?: string;
}

const SNAPSHOT_UNAVAILABLE_MESSAGE =
  "Terminal buffer unavailable. Resize the window to request a repaint.";

const SNAPSHOT_SETTLE_WARN_MS = 500;
const SNAPSHOT_RECONCILE_LIMIT_BYTES = 2 * 1024 * 1024;

const PTY_RESIZE_RETRY_DELAY_MS = 120;
const PTY_RESIZE_MAX_RETRIES = 3;

type PendingPtyResizeAttempt = {
  cols: number;
  rows: number;
  completion: Promise<"sent" | "failed">;
};

type SettledMainTerminalLayoutPulseTrace = {
  attachGeneration: number;
  trace: MainTerminalLayoutPulseTrace;
};

type TerminalLayoutObserverState = {
  observedObserverEpoch: number;
  completedObserverAck: MainTerminalLayoutObserverAck | null;
};

type TerminalLayoutObserverRecord = {
  epoch: number;
  entry: SessionTerminalEntry;
};

const emptyLayoutPulsePhase = (): MainTerminalLayoutPulsePhaseTrace => ({
  sidebarWidth: null,
  hostWidth: null,
  cols: null,
  rows: null,
  baselineObservedEpoch: null,
  completedObserverAck: null,
});

const makeLocalLayoutPulseResult = (
  requestId: number,
  sessionId: string,
  attachGeneration: number,
  status: "skipped" | "failed",
  reason: "unhandled" | "exception",
): MainTerminalLayoutPulseResult => {
  const trace: MainTerminalLayoutPulseTrace = {
    version: 1,
    requestId,
    sessionId,
    attachGeneration,
    status,
    reason,
    original: emptyLayoutPulsePhase(),
    expanded: emptyLayoutPulsePhase(),
    restored: emptyLayoutPulsePhase(),
    dwellMs: 0,
    settingsWritesDelta: 0,
  };
  return { status, reason, trace };
};

const TerminalView: Component<TerminalViewProps> = (props) => {
  let hostRef!: HTMLDivElement;
  let visibleSessionId: string | null = null;
  let resizeObserver: ResizeObserver | null = null;
  let unlistenPtyOutput: UnlistenFn | null = null;
  let unlistenSessionDestroyed: UnlistenFn | null = null;
  let unlistenAgentMessage: UnlistenFn | null = null;
  let unlistenTerminalDetached: UnlistenFn | null = null;
  let unlistenCloseRequested: UnlistenFn | null = null;

  // #1363 - this window's single output attachment, and the serialization that
  // keeps it honest. `attachedSessionId` is what the BACKEND holds for this
  // window; `desiredSessionId` is what the latest transition asked for. Attach
  // and detach are async invokes whose completion order is not their call
  // order, so a fast A -> B -> A switch could otherwise land the first
  // detach(A) after the final attach(A) and leave this window rendering a
  // session it is no longer attached to: a silent freeze indistinguishable
  // from #1363 itself. Every transition therefore runs on one promise chain,
  // re-checks the desired state after each await, and is the ONLY writer of
  // `attachedSessionId`.
  let attachedSessionId: string | null = null;
  let desiredSessionId: string | null = null;
  let attachChain: Promise<void> = Promise.resolve();

  // #1363 - never ask the backend to start emitting to this window before this
  // window is listening.
  //
  // Issuing the `listen` first is NOT enough to order the two. `plugin:event|
  // listen` is an async Tauri command dispatched onto the async runtime, while
  // `activate_terminal_output` is a sync command on the main thread, so their
  // completion order is not guaranteed even though the listen IPC leaves first.
  // The 16 ms coalescing window usually covers the gap, but a >=64 KiB burst
  // trips the ingest-thread ceiling flush immediately, and that chunk's
  // sequence is ABOVE the seed's, so the watermark never replays it: the hole
  // is silent and permanent. It lands exactly on "attach a busy session",
  // which is criterion C and #1364's scenario.
  //
  // A FAILED registration rejects the gate, which correctly leaves this window
  // unattached: a window with no listener could not render the stream anyway,
  // and attaching would only ask the backend to emit into nothing. The chain's
  // `.catch` keeps that from poisoning later transitions, and only the ATTACH
  // path waits here — detach and teardown run ahead of the gate, so a window
  // that never manages to listen still releases what it holds.
  let markListenerReady!: () => void;
  let failListenerReady!: (error: unknown) => void;
  const listenerReady = new Promise<void>((resolve, reject) => {
    markListenerReady = resolve;
    failListenerReady = reject;
  });
  // Browser mode never awaits this gate, so its rejection would otherwise be
  // an unhandled one. Awaiters still see the rejection.
  listenerReady.catch(() => undefined);

  const beforeResourceDispose = (sessionId: string): void => {
    // Fires for every entry the registry tears down (LRU eviction, session
    // destroy, and every entry of `disposeAll` on unmount): up to five times
    // per switch. Only the attached one owes a detach. This hook cannot move
    // into the registry: `no-terminal-helper-back-edge` forbids that module
    // from reaching `src/shared/ipc.ts`.
    if (sessionId === attachedSessionId) {
      transitionAttachment(null);
    }
  };

  const registry: TerminalRegistry = createTerminalSessionRegistry({
    host: () => hostRef,
    beforeResourceDispose,
  });

  const pendingPtyResizeAttempts = new WeakMap<
    SessionTerminalEntry,
    Map<string, PendingPtyResizeAttempt>
  >();
  const layoutPulseTraces = new WeakMap<
    SessionTerminalEntry,
    SettledMainTerminalLayoutPulseTrace
  >();
  const layoutObserverStates = new WeakMap<
    SessionTerminalEntry,
    TerminalLayoutObserverState
  >();
  let layoutObserverEpoch = 0;
  let layoutPulseRequestId = 0;

  const setReplayStatus = (entry: SessionTerminalEntry, message: string | null) => {
    entry.replayStatus.textContent = message ?? "";
    entry.replayStatus.hidden = !message;
  };

  // #1355 INVARIANT - every write of content to xterm goes through here, so
  // `entry.terminal.write` has exactly one call site in the whole frontend
  // (the line below). Adding a direct `terminal.write` anywhere else (a
  // banner, a reconnect notice, an error message) breaks the seed watermark
  // and reintroduces cumulative history duplication on re-attach. Write
  // through this function instead. Both transports traverse it: the Tauri
  // window and the browser/websocket fallback (#1363 criterion H').
  // `onWritten` (#1489) is the xterm 6.0.0 parse-completion signal: it fires
  // only after the chunk was parsed and applied. Callers must wrap it so it
  // cannot throw — a throwing callback aborts xterm's parse loop and wedges
  // the remaining queue.
  const writeTerminalBytes = (
    entry: SessionTerminalEntry,
    data: Uint8Array,
    onWritten?: () => void
  ) => {
    entry.hasRenderedOutput = true;
    setReplayStatus(entry, null);
    entry.terminal.write(data, onWritten);
  };

  const createSessionTerminal = (
    sessionId: string,
    container: HTMLDivElement
  ): Omit<
    SessionTerminalEntry,
    "sessionId" | "container" | "lastActivatedAt" | "destroyed"
  > => {
    const spawnViewport = takeSpawnViewport(sessionId);
    const terminal = new Terminal({
      ...createTerminalOptions(isTauri),
      ...(spawnViewport
        ? { cols: spawnViewport.cols, rows: spawnViewport.rows }
        : {}),
    });

    const fitAddon = new FitAddon();
    terminal.loadAddon(fitAddon);
    terminal.open(container);

    const replayStatus = document.createElement("div");
    replayStatus.className = "terminal-replay-status";
    replayStatus.hidden = true;
    replayStatus.setAttribute("data-ac-testid", `terminal.replay-status.${sessionId}`);
    container.appendChild(replayStatus);

    // #1682 - the bottom status strip. The stamp sits immediately LEFT of the
    // COLS/ROWS chip, and the chip's width varies with its digit count, so a
    // second independently right-anchored sibling would drift: one flex row
    // right-anchored as a whole is what keeps the gap exact. The positioning,
    // masking and padding therefore belong to the strip, and the chip keeps
    // only its own identity and typography.
    const statusStrip = document.createElement("div");
    statusStrip.className = "terminal-status-strip";
    statusStrip.style.position = "absolute";
    statusStrip.style.right = "8px";
    statusStrip.style.bottom = "4px";
    statusStrip.style.zIndex = "2";
    statusStrip.style.pointerEvents = "none";
    statusStrip.style.display = "flex";
    statusStrip.style.alignItems = "center";
    statusStrip.style.gap = "10px";
    statusStrip.style.padding = "1px 3px";
    statusStrip.style.background = "var(--terminal-bg)";

    // `hidden` and NOT an inline `display`: an inline display would beat the
    // user-agent `[hidden] { display: none }` rule. While hidden it is not a
    // flex item, so the strip's `gap` does not show either.
    const agentMessageStatus = document.createElement("div");
    agentMessageStatus.className = "terminal-agent-message-status";
    agentMessageStatus.setAttribute("data-testid", `terminal-agent-message-${sessionId}`);
    agentMessageStatus.setAttribute("aria-label", "Last coding agent message time");
    agentMessageStatus.style.color = "var(--statusbar-fg)";
    agentMessageStatus.style.fontFamily = "var(--font-mono)";
    agentMessageStatus.style.fontSize = "10px";
    agentMessageStatus.style.opacity = "0.75";
    agentMessageStatus.textContent = "";
    agentMessageStatus.hidden = true;

    const gridStatus = document.createElement("div");
    gridStatus.className = "terminal-grid-status";
    gridStatus.setAttribute("data-testid", `terminal-grid-size-${sessionId}`);
    gridStatus.setAttribute("aria-label", "Terminal grid size");
    gridStatus.style.color = "var(--statusbar-fg)";
    gridStatus.style.fontFamily = "var(--font-mono)";
    gridStatus.style.fontSize = "10px";
    gridStatus.style.opacity = "0.75";
    const updateGridStatus = (cols: number, rows: number) => {
      gridStatus.textContent = `COLS: ${cols} ROWS: ${rows}`;
    };
    updateGridStatus(terminal.cols, terminal.rows);

    statusStrip.appendChild(agentMessageStatus);
    statusStrip.appendChild(gridStatus);
    container.appendChild(statusStrip);

    let webglAddon: WebglAddon | null = null;
    try {
      webglAddon = new WebglAddon();
      webglAddon.onContextLoss(() => {
        registry.noteWebglContextLoss();
        webglAddon?.dispose();
      });
      terminal.loadAddon(webglAddon);
    } catch {
      webglAddon = null;
    }

    terminal.attachCustomKeyEventHandler((event) => {
      if (event.isComposing) return true;

      const isCtrlShift = event.ctrlKey && event.shiftKey;
      const key = event.key.toLowerCase();

      if (isCtrlShift && key === "c") {
        if (event.type === "keydown") {
          if (!terminal.hasSelection()) return true;
          event.preventDefault();
          event.stopPropagation();
          navigator.clipboard
            .writeText(terminal.getSelection())
            .catch((err) => console.warn("[copy] write failed:", err?.name ?? "Error"));
          return false;
        }
        return terminal.hasSelection() ? false : true;
      }

      if (isCtrlShift && key === "v") {
        if (event.type === "keydown") {
          event.preventDefault();
          event.stopPropagation();
          navigator.clipboard
            .readText()
            .then((text) => {
              if (!text) return;
              if (visibleSessionId !== sessionId) return; // session switched during await
              if (terminal.element?.isConnected !== true) return; // terminal not in DOM (pre-open or post-dispose detached)
              const sanitized = text.replace(/\x9b20[01]~|\x1b\[20[01]~/g, "");
              terminal.paste(sanitized);
            })
            .catch((err) => console.warn("[paste] read failed:", err?.name ?? "Error"));
          return false;
        }
        return false;
      }

      if (event.key === "Enter" && event.shiftKey) {
        if (event.type === "keydown" && visibleSessionId === sessionId) {
          const encoder = new TextEncoder();
          void PtyAPI.write(sessionId, encoder.encode("\n"));
        }
        return false; // suppress both keydown and keyup
      }
      return true;
    });

    terminal.onData((data) => {
      if (visibleSessionId !== sessionId) {
        return;
      }

      const encoder = new TextEncoder();
      void PtyAPI.write(sessionId, encoder.encode(data));

      const entry = registry.get(sessionId);
      if (!entry) return;
      const capture = updatePromptCapture(entry.inputBuffer, data);
      entry.inputBuffer = capture.buffer;
      if (capture.submittedPrompt) {
        void SessionAPI.setLastPrompt(sessionId, capture.submittedPrompt);
      }
    });

    terminal.onResize(({ cols, rows }) => {
      updateGridStatus(cols, rows);
      const entry = registry.get(sessionId);
      if (
        visibleSessionId !== sessionId ||
        !entry ||
        entry.snapshotResizeSuppressed
      ) {
        return;
      }

      void sendPtyResize(sessionId, entry, cols, rows);
    });

    // #1489 user-scroll guard: an intentional wheel scroll in the
    // post-replay/pre-settle window must never be overridden by the one-shot
    // attach settlement. The marker is read by the settle and cleared by
    // `beginSeed`. The entry is resolved by session id here because the entry
    // object does not exist yet at registration time; the listener dies with
    // the element on teardown.
    terminal.element?.addEventListener(
      "wheel",
      () => {
        const entry = registry.get(sessionId);
        const drain = entry && attachDrains.get(entry);
        if (drain) {
          drain.userScrolled = true;
        }
      },
      { passive: true }
    );

    return {
      terminal,
      fitAddon,
      webglAddon,
      replayStatus,
      agentMessageStatus,
      agentMessageAtMs: null,
      hasRenderedOutput: false,
      snapshotResizeSuppressed: false,
      inputBuffer: "",
      spawnViewport,
      lastSentViewport: spawnViewport,
      spawnDriftReported: false,
      resizeRetryTimer: null,
      resizeRetryAttempts: 0,
      snapshotSettleTimer: null,
      snapshotReplayPending: false,
      pendingSnapshotEvents: [],
      pendingSnapshotBytes: 0,
      lastAppliedSequence: null,
    };
  };

  // ── viewport / resize policy (unchanged #973 behavior) ────────────────────

  const reportSpawnSizeDrift = (
    sessionId: string,
    entry: SessionTerminalEntry,
    cols: number,
    rows: number
  ) => {
    const spawn = entry.spawnViewport;
    if (!spawn || entry.spawnDriftReported) {
      return;
    }
    if (spawn.cols === cols && spawn.rows === rows) {
      return;
    }

    entry.spawnDriftReported = true;
    console.warn(
      `[terminal] spawn-size drift ${sessionId}: PTY opened at ${spawn.cols}x${spawn.rows}, ` +
        `view fitted to ${cols}x${rows} — a resize will reach the child during startup (#973)`
    );
  };

  const scheduleResizeRetry = (sessionId: string, entry: SessionTerminalEntry) => {
    if (entry.resizeRetryTimer !== null) {
      return;
    }

    if (entry.resizeRetryAttempts >= PTY_RESIZE_MAX_RETRIES) {
      console.error(
        `[terminal] pty_resize ${sessionId} failed ${entry.resizeRetryAttempts} times: ` +
          `giving up with the PTY at a size this terminal is not (#973)`
      );
      return;
    }

    entry.resizeRetryAttempts += 1;
    const attempt = entry.resizeRetryAttempts;

    entry.resizeRetryTimer = setTimeout(() => {
      entry.resizeRetryTimer = null;

      if (registry.get(sessionId) !== entry) {
        return;
      }

      void sendPtyResize(sessionId, entry, entry.terminal.cols, entry.terminal.rows);
    }, PTY_RESIZE_RETRY_DELAY_MS * attempt);
  };

  const ptyResizeAttemptKey = (cols: number, rows: number): string => `${cols}:${rows}`;

  const startPtyResizeAttempt = (
    sessionId: string,
    entry: SessionTerminalEntry,
    cols: number,
    rows: number,
  ): Promise<"sent" | "failed"> => {
    const previous = entry.lastSentViewport;
    reportSpawnSizeDrift(sessionId, entry, cols, rows);

    const sent: PtyViewport = { cols, rows };
    entry.lastSentViewport = sent;

    let attempts = pendingPtyResizeAttempts.get(entry);
    if (!attempts) {
      attempts = new Map<string, PendingPtyResizeAttempt>();
      pendingPtyResizeAttempts.set(entry, attempts);
    }
    const key = ptyResizeAttemptKey(cols, rows);
    const completion = (async (): Promise<"sent" | "failed"> => {
      try {
        await PtyAPI.resize(sessionId, cols, rows);
        entry.resizeRetryAttempts = 0;
        return "sent";
      } catch (error: unknown) {
        if (entry.lastSentViewport === sent) {
          entry.lastSentViewport = previous;
        }
        console.warn(`[terminal] pty_resize ${sessionId} failed:`, error);

        scheduleResizeRetry(sessionId, entry);
        return "failed";
      }
    })();
    attempts.set(key, { cols, rows, completion });
    void completion.then(() => {
      const currentAttempts = pendingPtyResizeAttempts.get(entry);
      const current = currentAttempts?.get(key);
      if (current?.completion !== completion) {
        return;
      }
      currentAttempts!.delete(key);
      if (currentAttempts!.size === 0) {
        pendingPtyResizeAttempts.delete(entry);
      }
    });
    return completion;
  };

  // #1489: ordinary callers preserve optimistic tuple dedup. Only the final
  // attach confirmation below deliberately requires a real transport success.
  const sendPtyResize = async (
    sessionId: string,
    entry: SessionTerminalEntry,
    cols: number,
    rows: number,
  ): Promise<"sent" | "deduplicated" | "failed"> => {
    const previous = entry.lastSentViewport;
    if (previous && previous.cols === cols && previous.rows === rows) {
      return "deduplicated";
    }

    return startPtyResizeAttempt(sessionId, entry, cols, rows);
  };

  const confirmFinalPtyResize = (
    sessionId: string,
    entry: SessionTerminalEntry,
    cols: number,
    rows: number,
  ): Promise<"sent" | "failed"> => {
    const pending = pendingPtyResizeAttempts
      .get(entry)
      ?.get(ptyResizeAttemptKey(cols, rows));
    if (pending && pending.cols === cols && pending.rows === rows) {
      return pending.completion;
    }
    return startPtyResizeAttempt(sessionId, entry, cols, rows);
  };

  const syncViewport = (sessionId: string, skipPtyResize = false) => {
    const entry = registry.get(sessionId);
    if (!entry) {
      return;
    }

    entry.fitAddon.fit();
    if (!skipPtyResize) {
      void sendPtyResize(sessionId, entry, entry.terminal.cols, entry.terminal.rows);
    }
  };

  const isLiveVisibleEntry = (
    sessionId: string,
    entry: SessionTerminalEntry,
  ): boolean =>
    visibleSessionId === sessionId &&
    registry.get(sessionId) === entry &&
    registry.getVisible() === sessionId &&
    !entry.destroyed &&
    !entry.container.hidden &&
    hostRef.isConnected &&
    entry.container.isConnected &&
    hostRef.contains(entry.container);

  const captureLayoutGeometry = (
    entry: SessionTerminalEntry,
  ): MainTerminalLayoutGeometry => ({
    hostWidth: entry.container.getBoundingClientRect().width,
    cols: entry.terminal.cols,
    rows: entry.terminal.rows,
  });

  const isValidLayoutGeometry = (
    geometry: MainTerminalLayoutGeometry,
  ): boolean =>
    Number.isFinite(geometry.hostWidth) &&
    geometry.hostWidth >= 0 &&
    Number.isSafeInteger(geometry.cols) &&
    geometry.cols >= 0 &&
    Number.isSafeInteger(geometry.rows) &&
    geometry.rows >= 0;

  const measureFittedViewport = (): PtyViewport | null => {
    if (!visibleSessionId) {
      return null;
    }

    const entry = registry.get(visibleSessionId);
    if (!entry || entry.container.hidden) {
      return null;
    }

    const proposed = entry.fitAddon.proposeDimensions();
    if (!proposed) {
      return null;
    }

    return { cols: proposed.cols, rows: proposed.rows };
  };

  onCleanup(registerPtyViewportProbe(measureFittedViewport));

  const scheduleViewportSync = (
    sessionId: string,
    observerRecord?: TerminalLayoutObserverRecord,
  ) => {
    requestAnimationFrame(() => {
      if (!observerRecord) {
        if (sessionId !== visibleSessionId) {
          return;
        }
        syncViewport(sessionId);
        requestAnimationFrame(() => {
          if (sessionId === visibleSessionId) {
            syncViewport(sessionId);
          }
        });
        return;
      }

      const { entry, epoch } = observerRecord;
      if (!isLiveVisibleEntry(sessionId, entry)) {
        return;
      }
      try {
        syncViewport(sessionId);
      } catch {
        return;
      }
      if (!isLiveVisibleEntry(sessionId, entry)) {
        return;
      }
      const first = captureLayoutGeometry(entry);
      if (!isValidLayoutGeometry(first)) {
        return;
      }

      requestAnimationFrame(() => {
        if (!isLiveVisibleEntry(sessionId, entry)) {
          return;
        }
        try {
          syncViewport(sessionId);
        } catch {
          return;
        }
        if (!isLiveVisibleEntry(sessionId, entry)) {
          return;
        }
        const second = captureLayoutGeometry(entry);
        if (!isValidLayoutGeometry(second) || !isLiveVisibleEntry(sessionId, entry)) {
          return;
        }

        const state = layoutObserverStates.get(entry);
        if (
          !state ||
          (state.completedObserverAck !== null &&
            state.completedObserverAck.epoch >= epoch)
        ) {
          return;
        }
        state.completedObserverAck = { epoch, first, second };
      });
    });
  };

  const resizeTerminalForSnapshot = (
    entry: SessionTerminalEntry,
    cols: number,
    rows: number
  ) => {
    entry.snapshotResizeSuppressed = true;
    try {
      entry.terminal.resize(cols, rows);
    } finally {
      entry.snapshotResizeSuppressed = false;
    }
  };

  const writeAutomationInput = (value: string) => {
    if (!visibleSessionId || !value) return;
    const terminalInput = value.replace(/\r?\n/g, "\r");

    const encoder = new TextEncoder();
    void PtyAPI.write(visibleSessionId, encoder.encode(terminalInput));

    const entry = registry.get(visibleSessionId);
    if (!entry) return;

    for (const char of terminalInput) {
      const capture = updatePromptCapture(entry.inputBuffer, char);
      entry.inputBuffer = capture.buffer;
      if (capture.submittedPrompt) {
        void SessionAPI.setLastPrompt(visibleSessionId, capture.submittedPrompt);
      }
    }
  };

  // ── #961 seed / reconcile (restored from 4de8e11) ─────────────────────────
  //
  // Live PTY bytes are never gated. They reach xterm on arrival, whether the
  // seed settles late, fails, or never settles. The seed is a seed, not a
  // gate, and it reconciles AFTER the fact against its own sequence:
  // `parser.process()` and `output_sequence += 1` happen under the same mutex
  // that the snapshot read holds, so `snapshot.sequence` is exactly "the last
  // event whose bytes are in this snapshot" — no off-by-one.

  const eventSequence = (event: PtyOutputEvent): number | null =>
    typeof event.sequence === "number" ? event.sequence : null;

  const shouldDropAlreadyAppliedEvent = (
    entry: SessionTerminalEntry,
    sequence: number | null
  ) =>
    sequence !== null &&
    entry.lastAppliedSequence !== null &&
    sequence <= entry.lastAppliedSequence;

  const markAppliedSequence = (entry: SessionTerminalEntry, sequence: number | null) => {
    if (sequence === null) {
      return;
    }

    entry.lastAppliedSequence =
      entry.lastAppliedSequence === null
        ? sequence
        : Math.max(entry.lastAppliedSequence, sequence);
  };

  const abandonSnapshotReconcile = (entry: SessionTerminalEntry) => {
    entry.snapshotReplayPending = false;
    entry.pendingSnapshotEvents = [];
    entry.pendingSnapshotBytes = 0;
  };

  const retainForSnapshotReconcile = (
    entry: SessionTerminalEntry,
    event: PtyOutputEvent
  ) => {
    entry.pendingSnapshotEvents.push(event);
    entry.pendingSnapshotBytes += event.data.length;

    if (entry.pendingSnapshotBytes > SNAPSHOT_RECONCILE_LIMIT_BYTES) {
      abandonSnapshotReconcile(entry);
    }
  };

  const writeLivePtyOutput = (entry: SessionTerminalEntry, event: PtyOutputEvent) => {
    const sequence = eventSequence(event);

    if (entry.snapshotReplayPending) {
      retainForSnapshotReconcile(entry, event);
    }

    if (shouldDropAlreadyAppliedEvent(entry, sequence)) {
      // A retained event dropped by the watermark never writes, so it must
      // never count against the drain (#1489): counting it would hang the
      // drain and the replay with it.
      return;
    }

    // #1489 write fence: while the reconcile window is open, every byte this
    // entry queues in xterm is counted in the shared per-entry drain, so the
    // attach pipeline can prove that everything queued before the reset has
    // parsed before it replays. The drain always exists from `beginSeed`; the
    // 0->1 replacement is load-bearing: a write arriving between a drain
    // resolve and the drain loop's continuation must present a fresh,
    // unresolved promise for the loop to await.
    const drain = entry.snapshotReplayPending ? attachDrains.get(entry) : undefined;
    if (drain) {
      if (drain.inFlight === 0) {
        drain.promise = new Promise<void>((resolve) => {
          drain.resolve = resolve;
        });
      }
      drain.inFlight += 1;
      const onWritten = () => {
        drain.inFlight -= 1;
        if (drain.inFlight === 0 && drain.resolve) {
          drain.resolve();
          drain.resolve = null;
        }
      };
      try {
        writeTerminalBytes(entry, new Uint8Array(event.data), onWritten);
      } catch (error) {
        // xterm's 50-MiB queue guard throws synchronously BEFORE queueing the
        // chunk: the byte was never queued, so the drain must not wait for it.
        onWritten();
        throw error;
      }
      markAppliedSequence(entry, sequence);
      return;
    }

    writeTerminalBytes(entry, new Uint8Array(event.data));
    markAppliedSequence(entry, sequence);
  };

  // #1489: closes the reconcile window (retention, budget, and the pending
  // flag). The drain is deliberately NOT deleted: it must keep fencing this
  // entry's queued writes for the next generation, and the write path's
  // `snapshotReplayPending` gate already prevents any post-close registration.
  const closeSnapshotReconcile = (entry: SessionTerminalEntry) => {
    entry.snapshotReplayPending = false;
    entry.pendingSnapshotEvents = [];
    entry.pendingSnapshotBytes = 0;
  };

  // #1489: awaits parse completion of every live write queued since the
  // attach started. `reset()` does not discard xterm's pending queue, so the
  // fence must prove the queue is empty before the reset runs — otherwise
  // older bytes would parse into the fresh buffer and straddle the snapshot.
  // Aborted (stale) drains leave the count intact for the next generation.
  const drainPendingWrites = async (
    entry: SessionTerminalEntry,
    sessionId: string,
    generation: number
  ): Promise<void> => {
    const drain = attachDrains.get(entry);
    if (!drain) {
      return;
    }

    while (drain.inFlight > 0) {
      await drain.promise;
      if (!isCurrentAttach(sessionId, entry, generation)) {
        return;
      }
    }
  };

  const flushPendingEventsFenced = (
    entry: SessionTerminalEntry,
    events: PtyOutputEvent[],
    sessionId: string,
    generation: number,
    snapshot: PtyScreenSnapshot
  ) => {
    // Same predicate as live arrivals: no divergent second source of truth.
    // Events at or below the snapshot sequence are inside the snapshot and
    // are not replayed; events above it are replayed exactly once.
    const kept = events.filter(
      (event) => !shouldDropAlreadyAppliedEvent(entry, eventSequence(event))
    );

    // Wrapped per #1489 4.2: a callback passed to `write` must not throw into
    // xterm's parse loop. `beginAttachSettle` only registers an animation
    // frame, so this cannot throw in practice; the guard keeps the contract.
    const finalize = () => {
      try {
        if (isCurrentAttach(sessionId, entry, generation)) {
          beginAttachSettle(sessionId, entry, generation, snapshot);
        }
      } catch (error) {
        console.warn(`[terminal] attach replay ${sessionId} failed:`, error);
      }
    };

    if (kept.length === 0) {
      finalize();
      return;
    }

    // Every fenced replay write registers in the shared drain, unconditionally
    // (4.4): the reconcile window is already closed here, so the registration
    // must not reuse the `snapshotReplayPending` gate of 4.2. A later
    // generation's fence awaits an older generation's still-queued replay
    // bytes before its reset; without it, a fast re-attach could reset while
    // the older replay bytes are still in the FIFO, and those bytes would
    // parse into the fresh buffer ahead of the newer snapshot and duplicate
    // it (the issue's own straddle class).
    const drain = attachDrains.get(entry);
    const releaseDrain = () => {
      if (!drain) {
        return;
      }
      drain.inFlight -= 1;
      if (drain.inFlight === 0 && drain.resolve) {
        drain.resolve();
        drain.resolve = null;
      }
    };
    for (let index = 0; index < kept.length; index += 1) {
      const isLast = index === kept.length - 1;
      if (drain) {
        if (drain.inFlight === 0) {
          drain.promise = new Promise<void>((resolve) => {
            drain.resolve = resolve;
          });
        }
        drain.inFlight += 1;
      }
      const onWritten = () => {
        releaseDrain();
        if (isLast) {
          finalize();
        }
      };
      try {
        writeTerminalBytes(entry, new Uint8Array(kept[index].data), onWritten);
      } catch (error) {
        // xterm's 50-MiB queue guard throws synchronously BEFORE queueing the
        // chunk: the byte was never queued, so the drain must not wait for it,
        // and this generation's pipeline stops (live output continues; the
        // next attach re-seeds) - never a rethrow, never an unhandled
        // rejection.
        releaseDrain();
        console.warn(`[terminal] attach replay ${sessionId} failed:`, error);
        return;
      }
      markAppliedSequence(entry, eventSequence(kept[index]));
    }
  };

  // #1489 generation guard: every async continuation of the attach
  // transaction (drain loop, write callbacks, flush finalize, settle frame,
  // resize continuation) validates that the capture that started it is still
  // the current one. A switch away, a superseding attach, a detach, an
  // unmount, or an entry disposal flips at least one clause, so late
  // continuations become inert and can never mutate a newer replay.
  const isCurrentAttach = (
    sessionId: string,
    entry: SessionTerminalEntry,
    generation: number
  ): boolean =>
    registry.get(sessionId) === entry &&
    !entry.destroyed &&
    attachGenerations.get(entry) === generation &&
    desiredSessionId === sessionId &&
    attachedSessionId === sessionId &&
    visibleSessionId === sessionId;

  const sampleMainLayoutPulse = (
    sessionId: string,
    entry: SessionTerminalEntry,
    generation: number,
  ): MainTerminalLayoutPulseSample | null => {
    if (
      !isCurrentAttach(sessionId, entry, generation) ||
      !isLiveVisibleEntry(sessionId, entry)
    ) {
      return null;
    }
    const geometry = captureLayoutGeometry(entry);
    const observerState = layoutObserverStates.get(entry);
    return {
      ...geometry,
      observedObserverEpoch: observerState?.observedObserverEpoch ?? 0,
      completedObserverAck: observerState?.completedObserverAck ?? null,
    };
  };

  const requestMainLayoutPulse = (
    sessionId: string,
    entry: SessionTerminalEntry,
    generation: number,
  ): Promise<MainTerminalLayoutPulseResult> => {
    const requestId = layoutPulseRequestId;
    layoutPulseRequestId =
      layoutPulseRequestId < Number.MAX_SAFE_INTEGER ? layoutPulseRequestId + 1 : 0;

    return new Promise((resolve) => {
      let completed = false;
      const complete = (result: MainTerminalLayoutPulseResult): void => {
        if (completed) {
          return;
        }
        completed = true;
        resolve(result);
      };
      const request: MainTerminalLayoutPulseRequest = {
        requestId,
        sessionId,
        attachGeneration: generation,
        accepted: false,
        sample: () => sampleMainLayoutPulse(sessionId, entry, generation),
        complete,
      };

      try {
        window.dispatchEvent(
          new CustomEvent<MainTerminalLayoutPulseRequest>(
            MAIN_TERMINAL_LAYOUT_PULSE_REQUEST_EVENT,
            { detail: request },
          ),
        );
      } catch {
        complete(
          makeLocalLayoutPulseResult(
            requestId,
            sessionId,
            generation,
            "failed",
            "exception",
          ),
        );
        return;
      }
      if (!request.accepted) {
        complete(
          makeLocalLayoutPulseResult(
            requestId,
            sessionId,
            generation,
            "skipped",
            "unhandled",
          ),
        );
      }
    });
  };

  const clearSnapshotSettleTimer = (entry: SessionTerminalEntry) => {
    if (entry.snapshotSettleTimer !== null) {
      clearTimeout(entry.snapshotSettleTimer);
      entry.snapshotSettleTimer = null;
    }
  };

  interface SnapshotSettle {
    reconcilable: boolean;
    retainedEvents: PtyOutputEvent[];
  }

  // #1489 attach state, keyed by the entry object: the generation counter
  // (`attachGenerations`) and the shared write drain (`attachDrains`). The
  // drain is per-entry, never replaced and never deleted: `inFlight` counts
  // every byte still queued in this entry's xterm FIFO ACROSS generations,
  // because `reset()` does not discard the queue — a newer generation's fence
  // must also await an older generation's still-unparsed writes. `userScrolled`
  // is the attach-window user-scroll marker read by the settle and cleared by
  // `beginSeed`.
  interface AttachDrain {
    inFlight: number;
    promise: Promise<void> | null;
    resolve: (() => void) | null;
    userScrolled: boolean;
  }

  const attachGenerations = new WeakMap<SessionTerminalEntry, number>();
  const attachDrains = new WeakMap<SessionTerminalEntry, AttachDrain>();

  onCleanup(
    registerUiTerminalController(({ element, sessionId, operation }) => {
      if (
        element.getAttribute("data-ac-testid") !== `terminal.session.${sessionId}` ||
        element.getAttribute("data-ac-session-id") !== sessionId
      ) {
        return {
          ok: false,
          error: "terminal_target_mismatch",
          message: "Terminal target metadata no longer matches the requested session.",
        };
      }

      const entry = registry.get(sessionId);
      if (!entry || entry.destroyed) {
        return {
          ok: false,
          error: "terminal_entry_stale",
          message: "The requested terminal entry is no longer live.",
        };
      }
      if (entry.container !== element) {
        return {
          ok: false,
          error: "terminal_target_mismatch",
          message: "The requested terminal container is not the current session entry.",
        };
      }

      const terminalElement = entry.terminal.element;
      if (
        !entry.container.isConnected ||
        terminalElement?.isConnected !== true ||
        !entry.container.contains(terminalElement)
      ) {
        return {
          ok: false,
          error: "terminal_entry_stale",
          message: "The requested terminal entry is disconnected.",
        };
      }
      if (
        visibleSessionId !== sessionId ||
        registry.getVisible() !== sessionId ||
        entry.container.hidden
      ) {
        return {
          ok: false,
          error: "terminal_session_not_visible",
          message: "The requested terminal session is not visible in this WebView.",
        };
      }

      switch (operation.kind) {
        case "query":
          break;
        case "top":
          entry.terminal.scrollToTop();
          break;
        case "bottom":
          entry.terminal.scrollToBottom();
          break;
        case "line":
          entry.terminal.scrollToLine(operation.value);
          break;
        case "lines":
          entry.terminal.scrollLines(operation.value);
          break;
        case "pages":
          entry.terminal.scrollPages(operation.value);
          break;
      }

      const active = entry.terminal.buffer.active;
      if (operation.kind !== "query" && active.viewportY < active.baseY) {
        const drain = attachDrains.get(entry);
        if (drain) {
          drain.userScrolled = true;
        }
      }

      const currentGeneration = attachGenerations.get(entry);
      const settledLayoutPulse = layoutPulseTraces.get(entry);
      const layoutPulse =
        currentGeneration !== undefined &&
        settledLayoutPulse?.attachGeneration === currentGeneration &&
        settledLayoutPulse.trace.attachGeneration === currentGeneration
          ? settledLayoutPulse.trace
          : undefined;

      return {
        ok: true,
        target: {
          sessionId,
          baseY: active.baseY,
          viewportY: active.viewportY,
          length: active.length,
          cols: entry.terminal.cols,
          rows: entry.terminal.rows,
          type: active.type,
          atBottom: active.viewportY === active.baseY,
          ...(layoutPulse ? { layoutPulse } : {}),
        },
      };
    }),
  );

  const concludeSnapshotFetch = (
    sessionId: string,
    entry: SessionTerminalEntry
  ): SnapshotSettle | null => {
    if (registry.get(sessionId) !== entry || entry.destroyed) {
      return null;
    }

    clearSnapshotSettleTimer(entry);

    // #1489: the window (pending flag, retention, byte budget) is no longer
    // cleared here — consumers close it explicitly via `closeSnapshotReconcile`
    // once the reset/close decision is made.
    const settle: SnapshotSettle = {
      reconcilable: entry.snapshotReplayPending,
      retainedEvents: entry.pendingSnapshotEvents,
    };

    return settle;
  };

  const logSnapshotSettle = (
    sessionId: string,
    requestedAt: number,
    pendingEvents: number
  ) => {
    const elapsedMs = Math.round(performance.now() - requestedAt);
    const line = `[terminal] snapshot ${sessionId} settled in ${elapsedMs}ms, pendingEvents=${pendingEvents}`;

    if (elapsedMs > SNAPSHOT_SETTLE_WARN_MS) {
      console.warn(line);
      return;
    }

    console.debug(line);
  };

  // #1363 plan 3.3 rule 2: the reset is load-bearing and must not be
  // "simplified" away. Live bytes are written BEFORE the seed arrives, so
  // writing the seed on top of them would leave the earlier bytes on screen
  // twice; the reset is also what makes the #1355 history ring safe to replay
  // on every attach.
  //
  // #1489 replaces `rebuildFromSnapshot`: the pipeline is now write-fenced and
  // generation-scoped. Fire-and-forget (`void`) so the drain never blocks the
  // attach chain; a switch issued during a drain proceeds immediately and the
  // drain loop's own `isCurrentAttach` check aborts on the first interleaving.
  const replaySnapshotFenced = async (
    sessionId: string,
    entry: SessionTerminalEntry,
    snapshot: PtyScreenSnapshot,
    generation: number
  ): Promise<void> => {
    // 1. Drain: every live byte queued since `beginSeed` must have parsed
    //    BEFORE the reset, or it would parse into the fresh buffer after the
    //    reset and before the snapshot (the straddle/duplication regression).
    await drainPendingWrites(entry, sessionId, generation);
    if (!isCurrentAttach(sessionId, entry, generation)) {
      return;
    }

    // 2. Re-check the reconcile budget: the retention may have been abandoned
    //    during the drain (live output outran the budget).
    if (!entry.snapshotReplayPending) {
      console.warn(
        `[terminal] snapshot ${sessionId} discarded: live output outran the reconcile budget`
      );
      return;
    }

    // 3. Capture the retention, then close the window; reset and re-seed.
    const retained = entry.pendingSnapshotEvents;
    closeSnapshotReconcile(entry);

    entry.terminal.reset();
    entry.hasRenderedOutput = false;
    entry.lastAppliedSequence = snapshot.sequence;

    // 4. Snapshot bytes with a parse-completion callback, registered in the
    //    shared drain (4.1) with the same shape as the replay-write
    //    registration: the 0->1 promise replacement, the `inFlight`
    //    increment, and the throw-safe release (4.2). The registration makes
    //    a later generation's fence await an older generation's still-queued
    //    snapshot bytes before its reset; without it, a fast re-attach could
    //    reset while the older snapshot bytes are still in the FIFO and those
    //    bytes would parse into the fresh buffer ahead of the newer snapshot.
    //    The callback is throw-free by construction (4.2): it first releases
    //    the drain and, if the generation is still current, runs the flush
    //    trigger. A synchronous throw from `terminal.write` (the 50-MiB queue
    //    guard throws before queueing) releases the drain, logs the warn, and
    //    stops this generation's pipeline - never an unhandled rejection
    //    (live output continues; the next attach re-seeds).
    const drain = attachDrains.get(entry);
    const releaseDrain = () => {
      if (!drain) {
        return;
      }
      drain.inFlight -= 1;
      if (drain.inFlight === 0 && drain.resolve) {
        drain.resolve();
        drain.resolve = null;
      }
    };
    if (drain) {
      if (drain.inFlight === 0) {
        drain.promise = new Promise<void>((resolve) => {
          drain.resolve = resolve;
        });
      }
      drain.inFlight += 1;
    }
    const onSnapshotWritten = () => {
      releaseDrain();
      if (!isCurrentAttach(sessionId, entry, generation)) {
        return;
      }
      try {
        flushPendingEventsFenced(entry, retained, sessionId, generation, snapshot);
      } catch (error) {
        console.warn(`[terminal] attach replay ${sessionId} failed:`, error);
      }
    };
    try {
      writeTerminalBytes(entry, new Uint8Array(snapshot.data), onSnapshotWritten);
    } catch (error) {
      releaseDrain();
      console.warn(`[terminal] attach replay ${sessionId} failed:`, error);
    }
  };

  // #1489 one-shot settle: one animation frame after the replay completed, so
  // the fitted grid is applied before the viewport bottoms.
  const beginAttachSettle = (
    sessionId: string,
    entry: SessionTerminalEntry,
    generation: number,
    snapshot: PtyScreenSnapshot
  ) => {
    requestAnimationFrame(() => {
      if (!isCurrentAttach(sessionId, entry, generation)) {
        return;
      }
      void settleAttachViewport(sessionId, entry, generation, snapshot).catch((error) => {
        console.warn(`[terminal] attach settle ${sessionId} failed:`, error);
      });
    });
  };

  // #1489: the attach settle sequence. Fit first, then the awaitable typed
  // resize, then (unless the user scrolled in the settle window) exactly one
  // `scrollToBottom` — the only bottoming call site in the frontend — and one
  // bounded, content-free instrumentation line covering the issue's in-scope
  // evidence: viewport metrics, terminal grid, snapshot grid, seed size,
  // buffer type, and the resize outcome.
  const settleAttachViewport = async (
    sessionId: string,
    entry: SessionTerminalEntry,
    generation: number,
    snapshot: PtyScreenSnapshot
  ): Promise<void> => {
    let pulseTrace: MainTerminalLayoutPulseTrace | null = null;
    let resizeOutcome: "sent" | "deduplicated" | "failed";
    if (snapshot.data.length > 0) {
      const pulseResult = await requestMainLayoutPulse(sessionId, entry, generation);
      pulseTrace = pulseResult.trace;
      if (!isCurrentAttach(sessionId, entry, generation)) {
        return;
      }

      entry.fitAddon.fit();
      const finalCols = entry.terminal.cols;
      const finalRows = entry.terminal.rows;
      resizeOutcome = await confirmFinalPtyResize(
        sessionId,
        entry,
        finalCols,
        finalRows,
      );
    } else {
      entry.fitAddon.fit();
      resizeOutcome = await sendPtyResize(
        sessionId,
        entry,
        entry.terminal.cols,
        entry.terminal.rows,
      );
    }
    if (!isCurrentAttach(sessionId, entry, generation)) {
      return;
    }

    const drain = attachDrains.get(entry);
    if (drain && !drain.userScrolled) {
      entry.terminal.scrollToBottom();
    }

    const buffer = entry.terminal.buffer.active;
    console.debug(
      `[terminal] attach ${sessionId} settled: viewportY=${buffer.viewportY} ` +
        `baseY=${buffer.baseY} bufferLength=${buffer.length} ` +
        `cols=${entry.terminal.cols} rows=${entry.terminal.rows} ` +
        `type=${buffer.type} snapshotCols=${String(snapshot.cols)} ` +
        `snapshotRows=${String(snapshot.rows)} seedBytes=${snapshot.data.length} ` +
        `resize=${resizeOutcome}`
    );

    if (!isCurrentAttach(sessionId, entry, generation)) {
      return;
    }
    if (
      resizeOutcome === "sent" &&
      pulseTrace !== null &&
      pulseTrace.sessionId === sessionId &&
      pulseTrace.attachGeneration === generation
    ) {
      layoutPulseTraces.set(entry, { attachGeneration: generation, trace: pulseTrace });
    }
  };

  const applySnapshot = (
    sessionId: string,
    entry: SessionTerminalEntry,
    snapshot: PtyScreenSnapshot | null,
    settle: SnapshotSettle
  ) => {
    // #1439: a viewport sent before a detach must never dedup the
    // re-imposition after the re-attach; the other window may have driven the
    // PTY elsewhere while this key sat stale. Cleared once per attach settle
    // (this function's only caller); within an attached interval the dedup
    // operates unchanged.
    entry.lastSentViewport = null;

    // A retained event was ALREADY written live on arrival, so replaying the
    // retention is meaningful only after `rebuildFromSnapshot`'s reset wiped
    // it off the screen. An attach can resolve without a snapshot either
    // because the parser is unavailable (every event unsequenced) or because
    // the #1439 grid reconcile refused the seed (parser still Available, live
    // events still sequenced); replaying retention without a reset stays a
    // no-op for sequenced events (the watermark drops them) and a duplicate
    // write for unsequenced ones, which is why this branch replays nothing.
    if (!snapshot || snapshot.data.length === 0) {
      closeSnapshotReconcile(entry);
      if (!entry.hasRenderedOutput) {
        setReplayStatus(entry, SNAPSHOT_UNAVAILABLE_MESSAGE);
      }
      // #1439: a seedless attach must still re-impose this window's grid on
      // the PTY; otherwise live bytes keep arriving for the other window's
      // grid and garble this xterm until the user resizes something.
      if (sessionId === visibleSessionId) {
        scheduleViewportSync(sessionId);
      }
      return;
    }

    if (entry.hasRenderedOutput && !settle.reconcilable) {
      console.warn(
        `[terminal] snapshot ${sessionId} discarded: live output outran the reconcile budget`
      );
      closeSnapshotReconcile(entry);
      return;
    }

    // #1489 invariant: capturing the generation here is equivalent to
    // capturing it at `beginSeed` time — `transitionAttachment` serializes
    // every continuation on `attachChain`, and this function runs
    // synchronously inside the seed's own continuation, so no later
    // `beginSeed` (generation bump) can execute before the capture.
    const generation = attachGenerations.get(entry) ?? 0;

    if (
      snapshot.rows !== null &&
      snapshot.cols !== null &&
      (entry.terminal.rows !== snapshot.rows || entry.terminal.cols !== snapshot.cols)
    ) {
      resizeTerminalForSnapshot(entry, snapshot.cols, snapshot.rows);
    }

    // #1363 plan 3.4.2: EVERY attach re-seeds, reset first. Dropping the seed
    // on re-attach would preserve the retained xterm's deeper scrollback but
    // hide everything the session produced while this window was detached —
    // a silent content gap, which is the worse failure. The settle sequence
    // replaces the snapshot path's `scheduleViewportSync` (the seedless path
    // above keeps it).
    void replaySnapshotFenced(sessionId, entry, snapshot, generation);
  };

  // ── #1363 attach / detach ─────────────────────────────────────────────────

  const beginSeed = (sessionId: string, entry: SessionTerminalEntry): number => {
    // #1489: every attach bumps the generation (invalidating every async
    // continuation of older attachments of this session) and resets the
    // attach-window user-scroll marker. The drain itself is created once and
    // reused across generations: it must keep fencing this entry's queued
    // writes, because `reset()` does not discard xterm's pending queue.
    layoutPulseTraces.delete(entry);
    const previousGeneration = attachGenerations.get(entry) ?? 0;
    attachGenerations.set(
      entry,
      previousGeneration < Number.MAX_SAFE_INTEGER ? previousGeneration + 1 : 0,
    );

    const existing = attachDrains.get(entry);
    if (existing) {
      existing.userScrolled = false;
    } else {
      attachDrains.set(entry, {
        inFlight: 0,
        promise: null,
        resolve: null,
        userScrolled: false,
      });
    }

    entry.snapshotReplayPending = true;
    entry.pendingSnapshotEvents = [];
    entry.pendingSnapshotBytes = 0;

    clearSnapshotSettleTimer(entry);
    entry.snapshotSettleTimer = setTimeout(() => {
      entry.snapshotSettleTimer = null;
      if (registry.get(sessionId) !== entry || !entry.snapshotReplayPending) {
        return;
      }

      console.warn(
        `[terminal] snapshot ${sessionId} still pending after ${SNAPSHOT_SETTLE_WARN_MS}ms, pendingEvents=${entry.pendingSnapshotEvents.length}`
      );

      if (!entry.hasRenderedOutput) {
        setReplayStatus(entry, SNAPSHOT_UNAVAILABLE_MESSAGE);
      }
    }, SNAPSHOT_SETTLE_WARN_MS);

    return performance.now();
  };

  const settleSeed = (
    sessionId: string,
    entry: SessionTerminalEntry,
    requestedAt: number,
    snapshot: PtyScreenSnapshot | null
  ): void => {
    const settle = concludeSnapshotFetch(sessionId, entry);
    if (!settle) {
      return;
    }
    logSnapshotSettle(sessionId, requestedAt, settle.retainedEvents.length);
    applySnapshot(sessionId, entry, snapshot, settle);
  };

  const failSeed = (
    sessionId: string,
    entry: SessionTerminalEntry,
    requestedAt: number,
    error: unknown
  ): void => {
    const settle = concludeSnapshotFetch(sessionId, entry);
    if (!settle) {
      return;
    }
    logSnapshotSettle(sessionId, requestedAt, settle.retainedEvents.length);
    console.warn(`[terminal] attach_terminal_output ${sessionId} failed:`, error);
    // No reset ran, so the retention is not replayed here either: see
    // `applySnapshot`. The reconcile window must still close.
    closeSnapshotReconcile(entry);

    if (!entry.hasRenderedOutput) {
      setReplayStatus(entry, SNAPSHOT_UNAVAILABLE_MESSAGE);
    }
  };

  /**
   * The single writer of `attachedSessionId`, and the only issuer of attach and
   * detach. Never blocks the caller: it appends to the per-window chain and
   * returns, so a wedged invoke can delay the next transition but can never
   * hold up a selection change or a window close.
   */
  const transitionAttachment = (target: string | null): void => {
    if (isBrowser) {
      // Browser mode never attaches: the websocket broadcaster sits upstream
      // of the backend gate, so an attachment would buy nothing, and a browser
      // client can vanish with no close event, so nothing would release it.
      return;
    }

    desiredSessionId = target;
    attachChain = attachChain
      .then(async () => {
        if (desiredSessionId !== target || attachedSessionId === target) {
          return; // superseded, or already where this transition wanted to be
        }

        const previous = attachedSessionId;
        if (previous !== null) {
          attachedSessionId = null;
          await TerminalOutputAPI.detachOutput(previous);
          if (desiredSessionId !== target) {
            return;
          }
        }

        if (target === null) {
          return;
        }

        await listenerReady;
        if (desiredSessionId !== target) {
          return;
        }

        const entry = registry.get(target);
        if (!entry || entry.destroyed) {
          return; // the entry went away while this transition was queued
        }

        // #1528 - request the seed snapshot only after this window's fitted
        // grid is imposed on the PTY. The snapshot then arrives at the fitted
        // grid, `resizeTerminalForSnapshot` no-ops, the settle `fit()` no-ops,
        // and replay never resizes a populated buffer: the initial attach
        // converges on the same geometry as detach -> reattach. Wait one
        // animation frame so xterm has measured its cells against the
        // now-visible container (ResizeObserver delivery precedes rAF
        // callbacks in the same frame); fit to that container; clear the
        // dedup key so this imposition can never be deduplicated away by a
        // stale key (#1439) or by the fit's own fire-and-forget onResize
        // send; then await the typed resize so the backend parser grid is
        // updated before `attachOutput` captures the snapshot. The frame wait
        // is skipped while the document is hidden: Chromium/WebView2 suspend
        // rAF exactly for hidden (minimized/occluded) pages, so a hidden
        // window must not stall the attach; without the wait the fit no-ops
        // on unmeasured cells, the send carries the current grid, and the
        // attach degrades exactly to the status-quo path, converging when the
        // window becomes visible (G3). Every outcome of the awaited send is
        // tolerated exactly as the settle tolerates it: "failed" leaves the
        // PTY at its previous grid (the existing retry and the settle
        // re-imposition still run), "deduplicated" cannot occur after the key
        // clear. `attachedSessionId` is deliberately set only AFTER this
        // block: it means "a backend attachment is pending or active", and
        // an early return on a superseded or disposed target must not leave
        // a stale value behind (the "owes a detach" invariant of the chain).
        if (!document.hidden) {
          await new Promise<void>((resolve) => {
            requestAnimationFrame(() => resolve());
          });
        }
        if (
          desiredSessionId !== target ||
          registry.get(target) !== entry ||
          entry.destroyed
        ) {
          return;
        }
        entry.fitAddon.fit();
        entry.lastSentViewport = null;
        await sendPtyResize(target, entry, entry.terminal.cols, entry.terminal.rows);
        if (
          desiredSessionId !== target ||
          registry.get(target) !== entry ||
          entry.destroyed
        ) {
          return;
        }
        attachedSessionId = target;
        const requestedAt = beginSeed(target, entry);
        try {
          const snapshot = await TerminalOutputAPI.attachOutput(target);
          settleSeed(target, entry, requestedAt, snapshot);
        } catch (error) {
          // A rejected attach left the backend map unchanged: this window owes
          // no detach for it.
          if (attachedSessionId === target) {
            attachedSessionId = null;
          }
          failSeed(target, entry, requestedAt, error);
        }
      })
      // Without this the first rejected invoke would poison every subsequent
      // transition for the life of the window.
      .catch((error: unknown) => {
        console.warn("[terminal] attachment transition failed:", error);
      });
  };

  // #1682 - the only writer of the stamp element. Monotonic: a value applies
  // only when it parses AND is strictly newer than what is rendered, so a late
  // hydration can never overwrite a newer event that already landed on this
  // entry, and a null or unparseable value can never blank a good rendering.
  const applyAgentMessage = (
    entry: SessionTerminalEntry,
    at: string | null | undefined
  ): void => {
    const ms = at ? Date.parse(at) : Number.NaN;
    if (Number.isNaN(ms)) return;
    if (entry.agentMessageAtMs !== null && ms <= entry.agentMessageAtMs) return;
    entry.agentMessageAtMs = ms;
    entry.agentMessageStatus.textContent = formatAgentMessageStamp(at);
    entry.agentMessageStatus.hidden = false;
  };

  // #1682 - reconcile a newly visible session against disk, so a terminal that
  // missed the event still shows the persisted value. Rejection is expected in
  // browser mode (no dispatcher entry): a blank chip is the correct degradation.
  // Both guards answer different questions: the identity guard drops a result
  // belonging to an entry that was evicted and recreated (a NEW entry's
  // watermark is `null`, so it would otherwise accept anything), and the
  // watermark drops a result merely older than what this same entry shows.
  const hydrateAgentMessage = (sessionId: string, entry: SessionTerminalEntry): void => {
    void SessionAPI.getLastAgentMessage(sessionId)
      .then((at) => {
        if (registry.get(sessionId) !== entry || entry.destroyed) return;
        applyAgentMessage(entry, at);
      })
      .catch(() => {});
  };

  const selectSession = (sessionId: string): void => {
    if (visibleSessionId !== null && visibleSessionId !== sessionId) {
      const oldEntry = registry.get(visibleSessionId);
      if (oldEntry) {
        oldEntry.container.hidden = true;
      }
    }

    visibleSessionId = sessionId;
    const entry = registry.activate(sessionId, createSessionTerminal);
    hydrateAgentMessage(sessionId, entry);
    entry.container.hidden = false;
    registry.setVisible(sessionId);
    entry.terminal.focus();

    scheduleViewportSync(sessionId);
    transitionAttachment(sessionId);
  };

  // One writer for both transports (#1363 criterion H'). The visibility filter
  // is the #1283 fix F keeps: a retained but hidden terminal never receives a
  // write. Browser events carry no `sequence` and never enter a seed, so they
  // are written straight through.
  const handlePtyOutput = (event: PtyOutputEvent): void => {
    if (event.sessionId !== visibleSessionId) {
      return;
    }
    const entry = registry.get(event.sessionId);
    if (!entry || entry.destroyed) {
      return; // post-removal chunk: no state may be recreated
    }
    writeLivePtyOutput(entry, event);
  };

  onMount(async () => {
    resizeObserver = new ResizeObserver(() => {
      const sessionId = visibleSessionId;
      if (!sessionId) {
        return;
      }
      const entry = registry.get(sessionId);
      if (!entry || !isLiveVisibleEntry(sessionId, entry)) {
        return;
      }
      const nextEpoch = layoutObserverEpoch + 1;
      if (!Number.isSafeInteger(nextEpoch)) {
        return;
      }
      layoutObserverEpoch = nextEpoch;

      let state = layoutObserverStates.get(entry);
      if (!state) {
        state = { observedObserverEpoch: nextEpoch, completedObserverAck: null };
        layoutObserverStates.set(entry, state);
      } else {
        state.observedObserverEpoch = nextEpoch;
      }
      scheduleViewportSync(sessionId, { epoch: nextEpoch, entry });
    });
    resizeObserver.observe(hostRef);

    try {
      unlistenPtyOutput = await onPtyOutput(handlePtyOutput);
      markListenerReady();
    } catch (error) {
      // Not rethrown: the rest of this mount (session destroy, terminal
      // detach, the close hook) is still worth registering.
      console.warn("[terminal] pty_output listener registration failed:", error);
      failListenerReady(error);
    }

    unlistenSessionDestroyed = await onSessionDestroyed(({ id }) => {
      registry.remove(id);
    });

    // #1682 - the lookup is by the payload's `sessionId`, not by
    // `visibleSessionId`: a retained but hidden terminal stays correct and shows
    // the right value the moment it is shown again.
    unlistenAgentMessage = await onSessionAgentMessage(({ sessionId, at }) => {
      const entry = registry.get(sessionId);
      if (!entry || entry.destroyed) return;
      applyAgentMessage(entry, at);
    });

    if (!props.lockedSessionId) {
      unlistenTerminalDetached = await onTerminalDetached(({ sessionId }) => {
        // The session moved to its own window: release it here rather than
        // creating a hidden terminal for it.
        if (sessionId === attachedSessionId) {
          transitionAttachment(null);
        }
      });
    }

    if (isTauri) {
      // Net-new: there is no other close hook in `src/`. Fire-and-forget, with
      // no `preventDefault()` — awaiting it would let a wedged invoke make the
      // close button do nothing, and the backend's `WindowEvent::Destroyed`
      // reap is the real guarantee. This detach is a bandwidth optimization,
      // never a correctness dependency, so failing to register it is a warning
      // and nothing more.
      try {
        const { getCurrentWindow } = await import("@tauri-apps/api/window");
        unlistenCloseRequested = await getCurrentWindow().onCloseRequested(() => {
          transitionAttachment(null);
        });
      } catch (error) {
        console.warn("[terminal] close-requested detach hook unavailable:", error);
      }
    }
  });

  createEffect(() => {
    const sessionId = terminalStore.activeSessionId;
    if (!sessionId) {
      if (visibleSessionId) {
        const entry = registry.get(visibleSessionId);
        if (entry) {
          entry.container.hidden = true;
        }
        visibleSessionId = null;
        registry.setVisible(null);
      }
      // A cleared or dormant selection releases the attachment: nothing in
      // this window is rendering that session any more.
      transitionAttachment(null);
      return;
    }

    selectSession(sessionId);
  });

  onCleanup(() => {
    // Settle the gate so an attach still queued behind it cannot hang the
    // chain, and settle it as a FAILURE so it can never authorize an attach
    // for a view that is going away (settling twice is a no-op).
    failListenerReady(new Error("TerminalView unmounted"));
    unlistenPtyOutput?.();
    unlistenSessionDestroyed?.();
    unlistenAgentMessage?.();
    unlistenTerminalDetached?.();
    unlistenCloseRequested?.();
    resizeObserver?.disconnect();

    // Unmount is frequent and separate from window close: `shouldMountTerminal`
    // drops this component whenever the selection leaves live mode.
    transitionAttachment(null);
    registry.disposeAll();
  });

  return (
    <div
      class="terminal-host"
      ref={hostRef!}
      data-ac-testid="terminal.host"
      data-ac-role="surface"
    >
      <textarea
        class="terminal-automation-input"
        aria-label="Terminal automation input"
        disabled={!terminalStore.activeSessionId}
        tabIndex={-1}
        data-ac-testid="terminal.input"
        data-ac-role="textbox"
        data-ac-state={terminalStore.activeSessionId ? "ready" : "disabled"}
        onInput={(event) => {
          const value = event.currentTarget.value;
          event.currentTarget.value = "";
          writeAutomationInput(value);
        }}
      />
    </div>
  );
};

export default TerminalView;
