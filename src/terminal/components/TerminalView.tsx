import { Component, createEffect, onCleanup, onMount } from "solid-js";
import { Terminal } from "@xterm/xterm";
import { WebglAddon } from "@xterm/addon-webgl";
import { FitAddon } from "@xterm/addon-fit";
import {
  PtyAPI,
  SessionAPI,
  TerminalOutputAPI,
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

const TerminalView: Component<TerminalViewProps> = (props) => {
  let hostRef!: HTMLDivElement;
  let visibleSessionId: string | null = null;
  let resizeObserver: ResizeObserver | null = null;
  let unlistenPtyOutput: UnlistenFn | null = null;
  let unlistenSessionDestroyed: UnlistenFn | null = null;
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
  const writeTerminalBytes = (entry: SessionTerminalEntry, data: Uint8Array) => {
    entry.hasRenderedOutput = true;
    setReplayStatus(entry, null);
    entry.terminal.write(data);
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
      const entry = registry.get(sessionId);
      if (
        visibleSessionId !== sessionId ||
        !entry ||
        entry.snapshotResizeSuppressed
      ) {
        return;
      }

      sendPtyResize(sessionId, entry, cols, rows);
    });

    return {
      terminal,
      fitAddon,
      webglAddon,
      replayStatus,
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

      sendPtyResize(sessionId, entry, entry.terminal.cols, entry.terminal.rows);
    }, PTY_RESIZE_RETRY_DELAY_MS * attempt);
  };

  const sendPtyResize = (
    sessionId: string,
    entry: SessionTerminalEntry,
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
    const entry = registry.get(sessionId);
    if (!entry) {
      return;
    }

    entry.fitAddon.fit();
    if (!skipPtyResize) {
      sendPtyResize(sessionId, entry, entry.terminal.cols, entry.terminal.rows);
    }
  };

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

  const scheduleViewportSync = (sessionId: string) => {
    requestAnimationFrame(() => {
      if (sessionId !== visibleSessionId) {
        return;
      }

      syncViewport(sessionId);

      requestAnimationFrame(() => {
        if (sessionId === visibleSessionId) {
          syncViewport(sessionId);
        }
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
      return;
    }

    writeTerminalBytes(entry, new Uint8Array(event.data));
    markAppliedSequence(entry, sequence);
  };

  const flushPendingEvents = (
    entry: SessionTerminalEntry,
    events: PtyOutputEvent[]
  ) => {
    for (const event of events) {
      writeLivePtyOutput(entry, event);
    }
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

  const concludeSnapshotFetch = (
    sessionId: string,
    entry: SessionTerminalEntry
  ): SnapshotSettle | null => {
    if (registry.get(sessionId) !== entry || entry.destroyed) {
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

  // #1363 plan 3.3 rule 2: the reset is load-bearing and must not be
  // "simplified" away. Live bytes are written BEFORE the seed arrives, so
  // writing the seed on top of them would leave the earlier bytes on screen
  // twice; the reset is also what makes the #1355 history ring safe to replay
  // on every attach.
  const rebuildFromSnapshot = (
    entry: SessionTerminalEntry,
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
      return;
    }

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
    // a silent content gap, which is the worse failure.
    rebuildFromSnapshot(entry, snapshot, settle.retainedEvents);

    if (sessionId === visibleSessionId) {
      scheduleViewportSync(sessionId);
    }
  };

  // ── #1363 attach / detach ─────────────────────────────────────────────────

  const beginSeed = (sessionId: string, entry: SessionTerminalEntry): number => {
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
    // `applySnapshot`.

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

  const selectSession = (sessionId: string): void => {
    if (visibleSessionId !== null && visibleSessionId !== sessionId) {
      const oldEntry = registry.get(visibleSessionId);
      if (oldEntry) {
        oldEntry.container.hidden = true;
      }
    }

    visibleSessionId = sessionId;
    const entry = registry.activate(sessionId, createSessionTerminal);
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
      if (visibleSessionId) {
        scheduleViewportSync(visibleSessionId);
      }
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
