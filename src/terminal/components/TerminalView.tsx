import { Component, createEffect, onCleanup, onMount } from "solid-js";
import { Terminal } from "@xterm/xterm";
import { WebglAddon } from "@xterm/addon-webgl";
import { FitAddon } from "@xterm/addon-fit";
import {
  PtyAPI,
  SessionAPI,
  onPtyOutput,
  onSessionDestroyed,
  onTerminalDetached,
} from "../../shared/ipc";
import { isBrowser, isTauri } from "../../shared/platform";
import {
  registerPtyViewportProbe,
  takeSpawnViewport,
} from "../../shared/terminal-viewport";
import { terminalStore } from "../stores/terminal";
import type {
  PtyOutputEvent,
  PtyScreenSnapshot,
  PtyViewport,
} from "../../shared/types";
import type { UnlistenFn } from "../../shared/transport";
import { updatePromptCapture } from "./prompt-input-capture";
import { createTerminalOptions } from "./terminal-options";
import "@xterm/xterm/css/xterm.css";

interface SessionTerminal {
  container: HTMLDivElement;
  terminal: Terminal;
  fitAddon: FitAddon;
  inputBuffer: string;
  snapshotReplayRequested: boolean;
  snapshotReplayPending: boolean;
  snapshotResizeSuppressed: boolean;
  snapshotSettleTimer: ReturnType<typeof setTimeout> | null;
  hasRenderedOutput: boolean;
  replayStatus: HTMLDivElement;
  pendingSnapshotEvents: PtyOutputEvent[];
  pendingSnapshotBytes: number;
  lastAppliedSequence: number | null;
  spawnViewport: PtyViewport | null;
  lastSentViewport: PtyViewport | null;
  spawnDriftReported: boolean;
  resizeRetryTimer: ReturnType<typeof setTimeout> | null;
  resizeRetryAttempts: number;
}

interface TerminalViewProps {
  lockedSessionId?: string;
}

const SNAPSHOT_UNAVAILABLE_MESSAGE =
  "Terminal buffer unavailable. Resize the window to request a repaint.";

const SNAPSHOT_SETTLE_WARN_MS = 500;
const SNAPSHOT_RECONCILE_LIMIT_BYTES = 2 * 1024 * 1024;

const PTY_RESIZE_RETRY_DELAY_MS = 120;
const PTY_RESIZE_MAX_RETRIES = 3;

const TerminalView: Component<TerminalViewProps> = (props) => {
  let hostRef!: HTMLDivElement;
  let activeSessionId: string | null = null;
  let resizeObserver: ResizeObserver | null = null;
  let unlistenPtyOutput: UnlistenFn | null = null;
  let unlistenSessionDestroyed: UnlistenFn | null = null;
  let unlistenTerminalDetached: UnlistenFn | null = null;

  const terminals = new Map<string, SessionTerminal>();

  const reportSpawnSizeDrift = (
    sessionId: string,
    entry: SessionTerminal,
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

  const clearResizeRetryTimer = (entry: SessionTerminal) => {
    if (entry.resizeRetryTimer !== null) {
      clearTimeout(entry.resizeRetryTimer);
      entry.resizeRetryTimer = null;
    }
  };

  const scheduleResizeRetry = (sessionId: string, entry: SessionTerminal) => {
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

      if (terminals.get(sessionId) !== entry) {
        return;
      }

      sendPtyResize(sessionId, entry, entry.terminal.cols, entry.terminal.rows);
    }, PTY_RESIZE_RETRY_DELAY_MS * attempt);
  };

  const sendPtyResize = (
    sessionId: string,
    entry: SessionTerminal,
    cols: number,
    rows: number
  ) => {
    const previous = entry.lastSentViewport;
    if (previous && previous.cols === cols && previous.rows === rows) {
      return;
    }

    reportSpawnSizeDrift(sessionId, entry, cols, rows);

    const sent: PtyViewport = { cols, rows };
    entry.lastSentViewport = sent;

    void PtyAPI.resize(sessionId, cols, rows)
      .then(() => {
        entry.resizeRetryAttempts = 0;
      })
      .catch((err: unknown) => {
        if (entry.lastSentViewport === sent) {
          entry.lastSentViewport = previous;
        }
        console.warn(`[terminal] pty_resize ${sessionId} failed:`, err);

        scheduleResizeRetry(sessionId, entry);
      });
  };

  const syncViewport = (sessionId: string, skipPtyResize = false) => {
    const entry = terminals.get(sessionId);
    if (!entry) {
      return;
    }

    entry.fitAddon.fit();
    if (!skipPtyResize) {
      sendPtyResize(sessionId, entry, entry.terminal.cols, entry.terminal.rows);
    }
  };

  const measureFittedViewport = (): PtyViewport | null => {
    if (!activeSessionId) {
      return null;
    }

    const entry = terminals.get(activeSessionId);
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

  const scheduleViewportSync = (sessionId: string) => {
    requestAnimationFrame(() => {
      if (sessionId !== activeSessionId) {
        return;
      }

      syncViewport(sessionId);

      requestAnimationFrame(() => {
        if (sessionId === activeSessionId) {
          syncViewport(sessionId);
        }
      });
    });
  };

  const clearSnapshotSettleTimer = (entry: SessionTerminal) => {
    if (entry.snapshotSettleTimer !== null) {
      clearTimeout(entry.snapshotSettleTimer);
      entry.snapshotSettleTimer = null;
    }
  };

  const disposeSessionTerminal = (sessionId: string) => {
    const entry = terminals.get(sessionId);
    if (!entry) {
      return;
    }

    clearSnapshotSettleTimer(entry);
    clearResizeRetryTimer(entry);
    entry.terminal.dispose();
    entry.container.remove();
    terminals.delete(sessionId);

    if (activeSessionId === sessionId) {
      activeSessionId = null;
    }
  };

  const writeAutomationInput = (value: string) => {
    if (!activeSessionId || !value) return;
    const terminalInput = value.replace(/\r?\n/g, "\r");

    const encoder = new TextEncoder();
    void PtyAPI.write(activeSessionId, encoder.encode(terminalInput));

    const entry = terminals.get(activeSessionId);
    if (!entry) return;

    for (const char of terminalInput) {
      const capture = updatePromptCapture(entry.inputBuffer, char);
      entry.inputBuffer = capture.buffer;
      if (capture.submittedPrompt) {
        void SessionAPI.setLastPrompt(activeSessionId, capture.submittedPrompt);
      }
    }
  };

  const setReplayStatus = (entry: SessionTerminal, message: string | null) => {
    entry.replayStatus.textContent = message ?? "";
    entry.replayStatus.hidden = !message;
  };

  const writeTerminalBytes = (entry: SessionTerminal, data: Uint8Array) => {
    entry.hasRenderedOutput = true;
    setReplayStatus(entry, null);
    entry.terminal.write(data);
  };

  const eventSequence = (event: PtyOutputEvent): number | null =>
    typeof event.sequence === "number" ? event.sequence : null;

  const shouldDropAlreadyAppliedEvent = (
    entry: SessionTerminal,
    sequence: number | null
  ) =>
    sequence !== null &&
    entry.lastAppliedSequence !== null &&
    sequence <= entry.lastAppliedSequence;

  const markAppliedSequence = (entry: SessionTerminal, sequence: number | null) => {
    if (sequence === null) {
      return;
    }

    entry.lastAppliedSequence =
      entry.lastAppliedSequence === null
        ? sequence
        : Math.max(entry.lastAppliedSequence, sequence);
  };

  const abandonSnapshotReconcile = (entry: SessionTerminal) => {
    entry.snapshotReplayPending = false;
    entry.pendingSnapshotEvents = [];
    entry.pendingSnapshotBytes = 0;
  };

  const retainForSnapshotReconcile = (
    entry: SessionTerminal,
    event: PtyOutputEvent
  ) => {
    entry.pendingSnapshotEvents.push(event);
    entry.pendingSnapshotBytes += event.data.length;

    if (entry.pendingSnapshotBytes > SNAPSHOT_RECONCILE_LIMIT_BYTES) {
      abandonSnapshotReconcile(entry);
    }
  };

  const writeLivePtyOutput = (entry: SessionTerminal, event: PtyOutputEvent) => {
    const sequence = eventSequence(event);

    if (entry.snapshotReplayPending) {
      retainForSnapshotReconcile(entry, event);
    }

    if (shouldDropAlreadyAppliedEvent(entry, sequence)) {
      return;
    }

    writeTerminalBytes(entry, new Uint8Array(event.data));
    markAppliedSequence(entry, sequence);
  };

  const flushPendingEvents = (
    entry: SessionTerminal,
    events: PtyOutputEvent[]
  ) => {
    for (const event of events) {
      writeLivePtyOutput(entry, event);
    }
  };

  const resizeTerminalForSnapshot = (
    entry: SessionTerminal,
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

  interface SnapshotSettle {
    reconcilable: boolean;
    retainedEvents: PtyOutputEvent[];
  }

  const concludeSnapshotFetch = (
    sessionId: string,
    entry: SessionTerminal
  ): SnapshotSettle | null => {
    if (terminals.get(sessionId) !== entry) {
      return null;
    }

    clearSnapshotSettleTimer(entry);

    const settle: SnapshotSettle = {
      reconcilable: entry.snapshotReplayPending,
      retainedEvents: entry.pendingSnapshotEvents,
    };

    entry.snapshotReplayPending = false;
    entry.pendingSnapshotEvents = [];
    entry.pendingSnapshotBytes = 0;

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

  const rebuildFromSnapshot = (
    entry: SessionTerminal,
    snapshot: PtyScreenSnapshot,
    retainedEvents: PtyOutputEvent[]
  ) => {
    entry.terminal.reset();
    entry.hasRenderedOutput = false;
    entry.lastAppliedSequence = snapshot.sequence;

    writeTerminalBytes(entry, new Uint8Array(snapshot.data));
    flushPendingEvents(entry, retainedEvents);
  };

  const applySnapshot = (
    sessionId: string,
    entry: SessionTerminal,
    snapshot: PtyScreenSnapshot | null,
    settle: SnapshotSettle
  ) => {
    const { reconcilable, retainedEvents } = settle;

    if (!snapshot || snapshot.data.length === 0) {
      flushPendingEvents(entry, retainedEvents);
      if (!entry.hasRenderedOutput) {
        setReplayStatus(entry, SNAPSHOT_UNAVAILABLE_MESSAGE);
      }
      return;
    }

    if (entry.hasRenderedOutput && !reconcilable) {
      console.warn(
        `[terminal] snapshot ${sessionId} discarded: live output outran the reconcile budget`
      );
      return;
    }

    if (
      snapshot.rows !== null &&
      snapshot.cols !== null &&
      (entry.terminal.rows !== snapshot.rows || entry.terminal.cols !== snapshot.cols)
    ) {
      resizeTerminalForSnapshot(entry, snapshot.cols, snapshot.rows);
    }

    if (entry.hasRenderedOutput) {
      rebuildFromSnapshot(entry, snapshot, retainedEvents);
    } else {
      writeTerminalBytes(entry, new Uint8Array(snapshot.data));
      markAppliedSequence(entry, snapshot.sequence);
      flushPendingEvents(entry, retainedEvents);
    }

    if (sessionId === activeSessionId) {
      scheduleViewportSync(sessionId);
    }
  };

  const replayNativeSnapshot = (sessionId: string, entry: SessionTerminal) => {
    if (!isTauri || entry.snapshotReplayRequested) {
      return;
    }

    entry.snapshotReplayRequested = true;
    entry.snapshotReplayPending = true;
    entry.pendingSnapshotEvents = [];
    entry.pendingSnapshotBytes = 0;

    const requestedAt = performance.now();

    clearSnapshotSettleTimer(entry);
    entry.snapshotSettleTimer = setTimeout(() => {
      entry.snapshotSettleTimer = null;
      if (terminals.get(sessionId) !== entry || !entry.snapshotReplayPending) {
        return;
      }

      console.warn(
        `[terminal] snapshot ${sessionId} still pending after ${SNAPSHOT_SETTLE_WARN_MS}ms, pendingEvents=${entry.pendingSnapshotEvents.length}`
      );

      if (!entry.hasRenderedOutput) {
        setReplayStatus(entry, SNAPSHOT_UNAVAILABLE_MESSAGE);
      }
    }, SNAPSHOT_SETTLE_WARN_MS);

    void PtyAPI.getScreenSnapshot(sessionId)
      .then((snapshot) => {
        const settle = concludeSnapshotFetch(sessionId, entry);
        if (!settle) {
          return;
        }

        logSnapshotSettle(sessionId, requestedAt, settle.retainedEvents.length);
        applySnapshot(sessionId, entry, snapshot, settle);
      })
      .catch((err) => {
        const settle = concludeSnapshotFetch(sessionId, entry);
        if (!settle) {
          return;
        }

        logSnapshotSettle(sessionId, requestedAt, settle.retainedEvents.length);
        console.warn("[terminal] snapshot replay failed:", err);
        flushPendingEvents(entry, settle.retainedEvents);

        if (!entry.hasRenderedOutput) {
          setReplayStatus(entry, SNAPSHOT_UNAVAILABLE_MESSAGE);
        }
      });
  };

  const createSessionTerminal = (sessionId: string) => {
    const existing = terminals.get(sessionId);
    if (existing) {
      return existing;
    }

    const container = document.createElement("div");
    container.className = "terminal-instance";
    container.dataset.sessionId = sessionId;
    container.setAttribute("data-ac-testid", `terminal.session.${sessionId}`);
    container.setAttribute("data-ac-role", "surface");
    container.setAttribute("data-ac-session-id", sessionId);
    container.hidden = true;
    hostRef.appendChild(container);

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

    try {
      const webglAddon = new WebglAddon();
      webglAddon.onContextLoss(() => {
        webglAddon.dispose();
      });
      terminal.loadAddon(webglAddon);
    } catch {
    }

    const entry: SessionTerminal = {
      container,
      terminal,
      fitAddon,
      inputBuffer: "",
      snapshotReplayRequested: false,
      snapshotReplayPending: false,
      snapshotResizeSuppressed: false,
      snapshotSettleTimer: null,
      hasRenderedOutput: false,
      replayStatus,
      pendingSnapshotEvents: [],
      pendingSnapshotBytes: 0,
      lastAppliedSequence: null,
      spawnViewport,
      lastSentViewport: spawnViewport,
      spawnDriftReported: false,
      resizeRetryTimer: null,
      resizeRetryAttempts: 0,
    };

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
              if (activeSessionId !== sessionId) return; // session switched during await
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
        if (event.type === "keydown" && activeSessionId === sessionId) {
          const encoder = new TextEncoder();
          void PtyAPI.write(sessionId, encoder.encode("\n"));
        }
        return false; // suppress both keydown and keyup
      }
      return true;
    });

    terminal.onData((data) => {
      if (activeSessionId !== sessionId) {
        return;
      }

      const encoder = new TextEncoder();
      void PtyAPI.write(sessionId, encoder.encode(data));

      const capture = updatePromptCapture(entry.inputBuffer, data);
      entry.inputBuffer = capture.buffer;
      if (capture.submittedPrompt) {
        void SessionAPI.setLastPrompt(sessionId, capture.submittedPrompt);
      }
    });

    terminal.onResize(({ cols, rows }) => {
      if (activeSessionId !== sessionId || entry.snapshotResizeSuppressed) {
        return;
      }

      sendPtyResize(sessionId, entry, cols, rows);
    });

    terminals.set(sessionId, entry);
    replayNativeSnapshot(sessionId, entry);
    return entry;
  };

  const showSessionTerminal = (sessionId: string) => {
    const next = createSessionTerminal(sessionId);

    if (activeSessionId && activeSessionId !== sessionId) {
      const previous = terminals.get(activeSessionId);
      if (previous) {
        previous.container.hidden = true;
      }
    }

    next.container.hidden = false;
    activeSessionId = sessionId;
    next.terminal.focus();

    if (isBrowser) {
      requestAnimationFrame(() => {
        if (sessionId !== activeSessionId) return;
        syncViewport(sessionId);
      });
    } else {
      next.terminal.scrollToBottom();
      scheduleViewportSync(sessionId);
    }
  };

  onMount(async () => {
    resizeObserver = new ResizeObserver(() => {
      if (activeSessionId) {
        scheduleViewportSync(activeSessionId);
      }
    });
    resizeObserver.observe(hostRef);

    unlistenPtyOutput = await onPtyOutput((event) => {
      const { sessionId } = event;
      const entry =
        terminals.get(sessionId) ?? (sessionId === activeSessionId
          ? createSessionTerminal(sessionId)
          : null);

      if (!entry) {
        return;
      }

      writeLivePtyOutput(entry, event);
    });

    unlistenSessionDestroyed = await onSessionDestroyed(({ id }) => {
      disposeSessionTerminal(id);
    });

    if (!props.lockedSessionId) {
      unlistenTerminalDetached = await onTerminalDetached(({ sessionId }) => {
        if (!terminals.has(sessionId)) {
          const entry = createSessionTerminal(sessionId);
          entry.container.hidden = true;
        }
      });
    }
  });

  createEffect(() => {
    const sessionId = terminalStore.activeSessionId;
    if (!sessionId) {
      if (activeSessionId) {
        const activeEntry = terminals.get(activeSessionId);
        if (activeEntry) {
          activeEntry.container.hidden = true;
        }
      }
      activeSessionId = null;
      return;
    }

    showSessionTerminal(sessionId);
  });

  onCleanup(() => {
    unlistenPtyOutput?.();
    unlistenSessionDestroyed?.();
    unlistenTerminalDetached?.();
    resizeObserver?.disconnect();

    for (const sessionId of Array.from(terminals.keys())) {
      disposeSessionTerminal(sessionId);
    }
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
