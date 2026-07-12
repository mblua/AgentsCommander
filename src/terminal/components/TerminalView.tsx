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
  /**
   * A `get_screen_snapshot` round-trip is in flight AND the screen can still be
   * rebuilt from it. Never gates rendering (#955) — it only says whether the
   * retained live events are still complete enough to reconcile against.
   */
  snapshotReplayPending: boolean;
  snapshotResizeSuppressed: boolean;
  snapshotSettleTimer: ReturnType<typeof setTimeout> | null;
  hasRenderedOutput: boolean;
  replayStatus: HTMLDivElement;
  /** Live events already written to xterm, retained only to rebuild (#955). */
  pendingSnapshotEvents: PtyOutputEvent[];
  pendingSnapshotBytes: number;
  lastAppliedSequence: number | null;
  /**
   * #973. The size the PTY was OPENED at, when this is the first terminal built
   * for a session the view sized itself. Null for a re-attach, and for every
   * session created with no tile to measure.
   */
  spawnViewport: PtyViewport | null;
  /**
   * #973. The size the PTY is at, as far as this view knows: the size it was
   * opened at, then whatever resize we last sent. Null means "unknown", and the
   * next resize is always sent.
   */
  lastSentViewport: PtyViewport | null;
  /** #973. One drift warning per terminal, not one per resize. */
  spawnDriftReported: boolean;
}

interface TerminalViewProps {
  /**
   * If set, this TerminalView is inside a detached window locked to one
   * session. Disables the main-window pre-warm listener (plan §A2.3.G6).
   */
  lockedSessionId?: string;
}

const SNAPSHOT_UNAVAILABLE_MESSAGE =
  "Terminal buffer unavailable. Resize the window to request a repaint.";

// #955: `get_screen_snapshot` is an IPC round-trip, and it used to gate every
// byte a new terminal rendered. On a saturated webview main thread it was
// measured taking seconds — and, in the capture on the issue, never settling at
// all: the agent painted continuously into a permanently black tile.
//
// The gate is gone. Live bytes render on arrival, and the snapshot reconciles
// afterwards. These two bounds are what remains of the round-trip:
//
// 1. A settle deadline, purely diagnostic. It gates nothing; it sizes the
//    round-trip in the log and lets a terminal that has still shown *nothing*
//    say so instead of sitting black.
const SNAPSHOT_SETTLE_WARN_MS = 500;
// 2. A retention budget. Live events are held (in addition to being rendered)
//    so the screen can be rebuilt into the snapshot's frame of reference when
//    the snapshot loses the race. Past this many bytes the live stream has
//    repainted the screen many times over, the snapshot is worthless, and
//    holding more would leak if the round-trip never settles.
const SNAPSHOT_RECONCILE_LIMIT_BYTES = 2 * 1024 * 1024;

// WebGL context budget: ~16 per document. Canvas fallback activates silently
// when the budget is exhausted (e.g. after ~16 concurrent sessions in main).
// See plan §DW.9.
const TerminalView: Component<TerminalViewProps> = (props) => {
  let hostRef!: HTMLDivElement;
  let activeSessionId: string | null = null;
  let resizeObserver: ResizeObserver | null = null;
  let unlistenPtyOutput: UnlistenFn | null = null;
  let unlistenSessionDestroyed: UnlistenFn | null = null;
  let unlistenTerminalDetached: UnlistenFn | null = null;

  const terminals = new Map<string, SessionTerminal>();

  /**
   * #973 diagnostic. The PTY was opened at the size the view had already fitted
   * to, so a later fit that disagrees means the two measurements drifted apart —
   * a scrollbar, a font metric settling late, an extra layout pass, a DPI quirk.
   * The resize still goes out (correctness first), but it lands inside the
   * child's startup, which is the whole bug. This is the line that says it
   * happened, on a real box, instead of us assuming it never does.
   */
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

  /**
   * #973 — the ONE place a resize reaches the PTY, and it only does so when the
   * size actually moved.
   *
   * A single attach used to fire 5-20 `pty_resize` calls: `syncViewport` sent one
   * unconditionally, from a double `requestAnimationFrame` AND a `ResizeObserver`,
   * and xterm's own `onResize` sent another. dev-rust measured that a redundant
   * SAME-size burst is harmless (0/10 blank) while one row of real drift is not
   * (6/10) — so both halves matter: send nothing when nothing changed, and when
   * something did change, say it exactly once.
   */
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

    void PtyAPI.resize(sessionId, cols, rows).catch((err: unknown) => {
      // A FAILED resize must not poison the cache: if it did, the retry would be
      // deduped away as a no-op and the PTY would sit at the wrong size forever.
      // (dev-rust hit exactly this trap in the backend's own dedup.)
      if (entry.lastSentViewport === sent) {
        entry.lastSentViewport = previous;
      }
      console.warn(`[terminal] pty_resize ${sessionId} failed:`, err);
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

  /**
   * #973 — the size a terminal created RIGHT NOW would fit to, handed to
   * `create_session` so the PTY is opened at it and nothing has to resize a child
   * that is still starting up.
   *
   * This is not a prediction. `proposeDimensions()` is the exact computation the
   * new terminal's own post-mount `fit()` will run, and it is run here against a
   * tile that is already on screen in the same `.terminal-host`: every
   * `.terminal-instance` is `position: absolute; inset: 0`, so a new one gets the
   * identical box; `createTerminalOptions` is the same for all of them; and the
   * font metrics are already resolved, because this terminal has been rendering
   * with them. Same inputs, same computation, same answer.
   *
   * Null whenever any of that stops being true — nothing is active, the tile is
   * `display: none` (a pre-warmed or backgrounded session measures a 0 box), or
   * xterm itself reports it has not been laid out yet. The caller then sends no
   * size, which is strictly better than sending a wrong one.
   */
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

  /**
   * Retain a live event that has ALREADY been written to xterm, so the screen
   * can be rebuilt from the snapshot's frame of reference once it lands. This
   * buffer never withholds output (#955) — it exists only to reconcile.
   */
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

  /**
   * #955: live PTY bytes are written the instant they arrive. They are NEVER
   * gated on the snapshot round-trip — that gate was the defect: every newly
   * created terminal stayed black for as long as `get_screen_snapshot` took to
   * settle, which on a saturated IPC channel was forever, while the agent
   * painted underneath and the bytes piled up in an array.
   */
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
    /** The retained live events were still complete: a rebuild is safe. */
    reconcilable: boolean;
    retainedEvents: PtyOutputEvent[];
  }

  /**
   * Conclude the snapshot round-trip, whatever its outcome. Returns the live
   * events retained while it was in flight, or null if the terminal was
   * replaced/disposed underneath it.
   */
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

  /**
   * Live output already painted this terminal, so the snapshot lost the race.
   * A snapshot is a FULL-SCREEN repaint (`vt100::Screen::contents_formatted`),
   * so writing it on top of live bytes would duplicate them. Rebuild instead:
   * reset, lay down the snapshot (everything up to its sequence), then replay
   * the live events that came after it. The sequence dedup drops the ones the
   * snapshot already contains, so the result is exactly what the un-raced path
   * renders — no duplicated and no missing output.
   */
  const rebuildFromSnapshot = (
    entry: SessionTerminal,
    snapshot: PtyScreenSnapshot,
    retainedEvents: PtyOutputEvent[]
  ) => {
    entry.terminal.reset();
    entry.hasRenderedOutput = false;
    // Rewind, do not raise: the retained events after the snapshot's sequence
    // must replay, and markAppliedSequence() only ever moves the watermark up.
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
      // Nothing to seed with. Whatever live output has arrived stands, and the
      // dedup makes this flush a no-op for anything already on screen.
      flushPendingEvents(entry, retainedEvents);
      if (!entry.hasRenderedOutput) {
        setReplayStatus(entry, SNAPSHOT_UNAVAILABLE_MESSAGE);
      }
      return;
    }

    if (entry.hasRenderedOutput && !reconcilable) {
      // The retention budget was spent, so the live events after the snapshot's
      // sequence are no longer held and a rebuild would drop them. The live
      // stream has long since repainted the screen: keep it, drop the snapshot.
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
      // The snapshot won the race into a clean terminal: seed it and let the
      // retained events (if any) land on top. This is the re-attach path.
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

    // #955 diagnostic: size the round-trip. The gate is gone, but a slow settle
    // still points straight at a saturated IPC channel / webview main thread.
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

      // Live output keeps rendering regardless — that is the fix. Only a
      // terminal that has shown nothing at all has anything to report.
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

    // #973. The PTY was opened at the size this view fitted to, so start xterm AT
    // that size. Without this it would start at xterm's 80x24 default and the very
    // first fit would resize it — firing, through `onResize`, exactly the resize
    // into the child's startup that opening at the right size exists to avoid.
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
      // Canvas renderer fallback is automatic.
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
      // The PTY is already at the size it was opened at. That is the baseline the
      // post-mount fit is deduped against, and the reason it sends nothing at all.
      lastSentViewport: spawnViewport,
      spawnDriftReported: false,
    };

    // Per-terminal keyboard shortcuts. Match keys via event.key (layout-aware,
    // matches the convention in shared/shortcuts.ts) so Dvorak/Colemak/AZERTY
    // users press the key labeled C/V in their layout, not the QWERTY position.
    terminal.attachCustomKeyEventHandler((event) => {
      // IME composition: hand the keystroke back to xterm so its internal
      // _compositionHelper can finish the multi-keystroke sequence. Some CJK
      // IMEs chord Ctrl+Shift during composition; intercepting would corrupt
      // the IME state.
      if (event.isComposing) return true;

      const isCtrlShift = event.ctrlKey && event.shiftKey;
      const key = event.key.toLowerCase();

      // Ctrl+Shift+C → copy xterm selection to system clipboard. With no
      // selection, return true so the event falls through (in dev, WebView2
      // opens DevTools; in production, nothing happens).
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
        // keyup mirror: re-check selection (stateless; selection persists post-copy).
        return terminal.hasSelection() ? false : true;
      }

      // Ctrl+Shift+V → paste system clipboard via terminal.paste() so the
      // payload is wrapped in bracketed-paste markers. clipboard.readText()
      // is async; re-check session and dispose state inside .then.
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
              // Strip both 7-bit ESC[200~/201~ and 8-bit C1 \x9b 200~/201~
              // forms — defense-in-depth against future shells that activate
              // 8-bit recognition. xterm@6.0.0 does NOT sanitize internally
              // (CVE-2019-11848 redux). See plan #104 §1.1.D / §6.8.
              const sanitized = text.replace(/\x9b20[01]~|\x1b\[20[01]~/g, "");
              terminal.paste(sanitized);
            })
            .catch((err) => console.warn("[paste] read failed:", err?.name ?? "Error"));
          return false;
        }
        return false;
      }

      // Shift+Enter → send LF (soft newline) instead of CR (submit)
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
      // Browser mode: fit xterm to container and resize PTY to match.
      // The program receives SIGWINCH and redraws at the new size.
      // We skip the snapshot replay because it was rendered for the
      // native terminal's dimensions and would look garbled here.
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

    // Plan §A2.3.G6 pre-warm: in main-window mode, subscribe to
    // terminal_detached events and pre-create hidden xterm entries so
    // pty_output for detached sessions accumulates in main's cache. On
    // re-attach, showSessionTerminal promotes the pre-warmed entry to
    // visible with full scrollback intact. Skip in detached windows.
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
