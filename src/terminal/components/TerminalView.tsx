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
  TerminalOutputActivation,
  TerminalOutputActivationResult,
  TerminalOutputControlState,
  TerminalRendererMetrics,
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
import {
  createTerminalOutputAdmission,
  parseCanonicalCounter,
  TELEMETRY_INTERVAL_MS,
  type CanonicalCounter,
  type TerminalOutputAdmission,
} from "./terminal-output-admission";
import "@xterm/xterm/css/xterm.css";

interface TerminalViewProps {
  lockedSessionId?: string;
}

const SNAPSHOT_UNAVAILABLE_MESSAGE =
  "Terminal buffer unavailable. Resize the window to request a repaint.";

const SNAPSHOT_SETTLE_WARN_MS = 500;

const PTY_RESIZE_RETRY_DELAY_MS = 120;
const PTY_RESIZE_MAX_RETRIES = 3;

/**
 * #1283 - one session's local desired-selection record (plan 14.2). The
 * admission controller is the sole strong owner of the current callback gate;
 * TerminalView owns only primitive control identity and counters.
 */
interface ReconcileRecord {
  causeGeneration: CanonicalCounter; // gA
  revision: number;
  bKnownGeneration: CanonicalCounter | null;
  bAttempt: {
    attempt: number;
    outcome: "pending" | TerminalOutputActivationResult;
  } | null;
  aDeactivation: "pending" | TerminalOutputControlState;
}

interface EntryControl {
  sessionId: string;
  entry: SessionTerminalEntry;
  admission: TerminalOutputAdmission;
  epoch: number;
  attempt: number;
  activation: "idle" | "activating" | "active";
  generation: CanonicalCounter | null;
  generationStr: string | null;
  detached: boolean;
  recoveryInFlight: boolean;
  reconciliation: ReconcileRecord | null;
  healthInFlight: boolean;
  healthPollToken: number;
  // #1283 telemetry counters owned by the orchestrator (plan 5.6.1).
  activationReadyAcknowledgements: number;
  activationReadyRejections: number;
  activationReadyTimeouts: number;
  generationHealthPollsScheduled: number;
  generationHealthPollsStarted: number;
  generationHealthPollsCancelled: number;
}

const TerminalView: Component<TerminalViewProps> = (props) => {
  let hostRef!: HTMLDivElement;
  let visibleSessionId: string | null = null;
  let selectionEpoch = 0;
  let resizeObserver: ResizeObserver | null = null;
  let unlistenPtyOutput: UnlistenFn | null = null;
  let unlistenSessionDestroyed: UnlistenFn | null = null;
  let unlistenTerminalDetached: UnlistenFn | null = null;
  let lastOrdinaryReportAt = 0;

  const controls = new Map<string, EntryControl>();

  const beforeResourceDispose = (sessionId: string): void => {
    const control = controls.get(sessionId);
    if (!control) {
      return;
    }
    // Cancel the generation health deadline and invalidate in-flight tokens.
    if (control.entry.healthTimer !== null) {
      clearTimeout(control.entry.healthTimer);
      control.entry.healthTimer = null;
      control.generationHealthPollsCancelled += 1;
    }
    control.healthInFlight = false;
    const generationStr = control.generationStr;
    // Admission seal/drop BEFORE any terminal/addon/DOM disposal (plan 5.5.2).
    control.admission.dispose();
    if (generationStr !== null) {
      // Matching deactivation when applicable; stale results are a no-op.
      void TerminalOutputAPI.deactivate(sessionId, generationStr).catch(() => undefined);
    }
    controls.delete(sessionId);
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
  // (the line below). That is what makes `hasRenderedOutput` a sound
  // discriminator for the `includeHistory` argument of activate(): an entry
  // with content can never have the flag `false`. Adding a direct
  // `terminal.write` anywhere else (a banner, a reconnect notice, an error
  // message) silently reintroduces cumulative history duplication on replay
  // (plan 6.1 / 12.1). Write through this function instead.
  const writeTerminalBytes = (
    entry: SessionTerminalEntry,
    data: Uint8Array,
    callback?: () => void
  ) => {
    entry.hasRenderedOutput = true;
    setReplayStatus(entry, null);
    entry.terminal.write(data, callback);
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
      healthTimer: null,
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

  const clearResizeRetryTimer = (entry: SessionTerminalEntry) => {
    if (entry.resizeRetryTimer !== null) {
      clearTimeout(entry.resizeRetryTimer);
      entry.resizeRetryTimer = null;
    }
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

  // ── #1283 activation / admission orchestration ────────────────────────────

  const createControl = (
    sessionId: string,
    entry: SessionTerminalEntry
  ): EntryControl => {
    const control: EntryControl = {
      sessionId,
      entry,
      admission: null as unknown as TerminalOutputAdmission,
      epoch: 0,
      attempt: 0,
      activation: "idle",
      generation: null,
      generationStr: null,
      detached: false,
      recoveryInFlight: false,
      reconciliation: null,
      healthInFlight: false,
      healthPollToken: 0,
      activationReadyAcknowledgements: 0,
      activationReadyRejections: 0,
      activationReadyTimeouts: 0,
      generationHealthPollsScheduled: 0,
      generationHealthPollsStarted: 0,
      generationHealthPollsCancelled: 0,
    };
    control.admission = createTerminalOutputAdmission(sessionId, {
      // Resolved at call time: startActivationAttempt may replace the entry.
      write: (bytes, callback) => writeTerminalBytes(control.entry, bytes, callback),
      scheduleFrame: (callback) => requestAnimationFrame(callback),
      cancelFrame: (handle) => cancelAnimationFrame(handle),
      now: () => performance.now(),
      acknowledge: (sid, generation, firstSequence, sequence) => {
        void TerminalOutputAPI.acknowledgeDelivery(
          sid,
          generation,
          firstSequence,
          sequence
        ).catch(() => undefined);
      },
      resync: (sid) => {
        const current = controls.get(sid);
        if (current) {
          sealAndRecover(current);
        }
      },
    });
    return control;
  };

  const disposeEntry = (sessionId: string): void => {
    // `beforeResourceDispose` synchronously seals admission, sends the matching
    // deactivation, and deletes the control; then resources are disposed once.
    registry.remove(sessionId);
  };

  const cancelHealth = (control: EntryControl): void => {
    if (control.entry.healthTimer !== null) {
      clearTimeout(control.entry.healthTimer);
      control.entry.healthTimer = null;
      control.generationHealthPollsCancelled += 1;
    }
    control.healthInFlight = false;
  };

  const armHealthDeadline = (control: EntryControl): void => {
    cancelHealth(control);
    control.generationHealthPollsScheduled += 1;
    control.entry.healthTimer = setTimeout(
      () => fireHealthPoll(control),
      TELEMETRY_INTERVAL_MS
    );
  };

  const composeMetrics = (control: EntryControl): TerminalRendererMetrics => {
    const rm = registry.metrics();
    const am = control.admission.metrics();
    return {
      retainedTerminalCount: rm.retained,
      visibleTerminalCount: rm.visible,
      webglContextCount: rm.webglContexts,
      webglContextLossCount: rm.webglContextLosses,
      lruEvictionCount: rm.lruEvictions,
      outputEventsReceived: am.outputEventsReceived,
      inactiveOrStaleEventsRejected: am.inactiveOrStaleEventsRejected,
      bytesAccepted: am.bytesAccepted,
      bytesWritten: am.bytesWritten,
      replayPendingBytes: am.replayPendingBytes,
      livePendingBytes: am.livePendingBytes,
      writeInFlightBytes: am.writeInFlightBytes,
      combinedAdmissionHighWaterBytes: am.combinedAdmissionHighWaterBytes,
      pendingHighWaterBytes: am.pendingHighWaterBytes,
      resyncCount: am.resyncCount,
      activationReadyAcknowledgements: control.activationReadyAcknowledgements,
      activationReadyRejections: control.activationReadyRejections,
      activationReadyTimeouts: control.activationReadyTimeouts,
      generationHealthPollsScheduled: control.generationHealthPollsScheduled,
      generationHealthPollsStarted: control.generationHealthPollsStarted,
      generationHealthPollsCancelled: control.generationHealthPollsCancelled,
      replayPendingLivenessRecoveries: am.replayPendingLivenessRecoveries,
      snapshotReplayDurationMs: am.snapshotReplayDurationMs,
      retiredWriteCallbacksIgnoredAfterDisposal:
        am.retiredWriteCallbacksIgnoredAfterDisposal,
      maxAnimationFrameLagMs: am.maxAnimationFrameLagMs,
    };
  };

  const maybeReportOrdinaryTelemetry = (control: EntryControl): void => {
    const now = performance.now();
    if (now - lastOrdinaryReportAt < TELEMETRY_INTERVAL_MS) {
      return;
    }
    if (control.healthInFlight) {
      return; // one metrics control lane: never a second in-flight report
    }
    lastOrdinaryReportAt = now;
    const generationStr = control.generationStr ?? control.generation?.toString() ?? "0";
    void TerminalOutputAPI.reportMetrics(
      control.sessionId,
      generationStr,
      composeMetrics(control)
    ).catch(() => undefined);
  };

  const fireHealthPoll = (control: EntryControl): void => {
    const entry = control.entry;
    entry.healthTimer = null;
    if (
      control.epoch !== selectionEpoch ||
      control.activation !== "active" ||
      control.generation === null ||
      entry.destroyed
    ) {
      control.generationHealthPollsCancelled += 1;
      return;
    }
    const kind = control.admission.stateKind();
    if (kind !== "replayPending" && kind !== "live") {
      control.generationHealthPollsCancelled += 1;
      return;
    }
    if (control.healthInFlight) {
      // A poll is already in flight: advance only the fixed cadence.
      entry.healthTimer = setTimeout(
        () => fireHealthPoll(control),
        TELEMETRY_INTERVAL_MS
      );
      return;
    }
    control.healthInFlight = true;
    control.healthPollToken += 1;
    const token = control.healthPollToken;
    control.generationHealthPollsStarted += 1;
    const generationStr = control.generationStr ?? control.generation.toString();
    void TerminalOutputAPI.reportMetrics(control.sessionId, generationStr, composeMetrics(control))
      .then((state) => settleHealthResponse(control, token, state))
      .catch(() => {
        if (token !== control.healthPollToken || !control.healthInFlight) {
          return;
        }
        control.healthInFlight = false;
        // Ordinary transport error: at most the next fixed-cadence deadline.
        control.entry.healthTimer = setTimeout(
          () => fireHealthPoll(control),
          TELEMETRY_INTERVAL_MS
        );
      });
  };

  const settleHealthResponse = (
    control: EntryControl,
    token: number,
    state: TerminalOutputControlState
  ): void => {
    if (token !== control.healthPollToken || !control.healthInFlight) {
      return; // stale/in-flight token invalidated by seal or replacement
    }
    control.healthInFlight = false;
    if (
      control.epoch !== selectionEpoch ||
      control.activation !== "active" ||
      control.generation === null
    ) {
      return;
    }
    switch (state.kind) {
      case "resyncRequired":
      case "awaitingFrontendReady": {
        control.generationHealthPollsCancelled += 1;
        if (control.admission.isReplayPending()) {
          control.admission.noteLivenessRecovery();
        }
        sealAndRecover(control);
        break;
      }
      case "active":
        // Matching active response: schedule the next fixed-cadence deadline.
        control.entry.healthTimer = setTimeout(
          () => fireHealthPoll(control),
          TELEMETRY_INTERVAL_MS
        );
        break;
      case "recoveryError":
        sealAndRecover(control);
        break;
      case "inactive":
      case "stale":
        // Sealed/stale responses cannot schedule a new poll.
        control.generationHealthPollsCancelled += 1;
        break;
    }
  };

  const sealAndRecover = (control: EntryControl): void => {
    if (control.recoveryInFlight) {
      return; // exactly one recovery lane at a time
    }
    control.admission.sealGeneration();
    cancelHealth(control);
    const generationStr = control.generationStr;
    control.recoveryInFlight = true;
    if (!control.entry.hasRenderedOutput) {
      setReplayStatus(control.entry, SNAPSHOT_UNAVAILABLE_MESSAGE);
    }
    if (generationStr === null) {
      control.recoveryInFlight = false;
      return;
    }
    void TerminalOutputAPI.deactivate(control.sessionId, generationStr)
      .then(() => {
        control.recoveryInFlight = false;
        if (control.epoch !== selectionEpoch) {
          return; // selection moved on: no replacement activation
        }
        // Matching inactive (or stale for an already-retired generation): the
        // old-generation output barrier has passed; serialize one replacement.
        startActivationAttempt(control.sessionId);
      })
      .catch(() => {
        // Fail closed: no tight retry loop, no auto re-activation.
        control.recoveryInFlight = false;
      });
  };

  const startActivationAttempt = (sessionId: string): void => {
    const control = controls.get(sessionId);
    if (!control || control.epoch !== selectionEpoch) {
      return; // C selected / destroyed / unmounted: discard, never reissue
    }
    // The entry may have been disposed (displaced-B, recovery after eviction).
    // #1355 - this reassignment MUST precede the activate() call below:
    // reconcileStaleSuccess parks a destroyed entry in control.entry (:887-891),
    // and only refreshing it here keeps includeHistory off a stale flag.
    const entry = registry.activate(sessionId, createSessionTerminal);
    control.entry = entry;
    control.attempt += 1;
    control.activation = "activating";
    control.generation = null;
    control.generationStr = null;
    control.reconciliation = null;
    entry.container.hidden = false;
    entry.terminal.focus();
    registry.setVisible(sessionId);
    scheduleViewportSync(sessionId);
    const attempt = control.attempt;
    void TerminalOutputAPI.activate(sessionId, !entry.hasRenderedOutput)
      .then((result) => settleActivationResult(sessionId, attempt, result))
      .catch((error) => settleActivationError(sessionId, attempt, error));
  };

  const commitActivation = (
    control: EntryControl,
    generation: CanonicalCounter,
    activation: TerminalOutputActivation
  ): void => {
    control.activation = "active";
    control.generation = generation;
    control.generationStr = activation.generation;
    // Synchronous local transition BEFORE readiness: commit ReplayPending and
    // its strong replay gate, then arm the generation health deadline, then
    // send readyTerminalOutput with the same S (plan 5.4.2).
    control.admission.beginSnapshotReplay(
      control.epoch,
      generation,
      activation.snapshot.sequence
    );
    const entry = control.entry;
    if (entry.snapshotSettleTimer !== null) {
      clearTimeout(entry.snapshotSettleTimer);
    }
    entry.snapshotSettleTimer = setTimeout(() => {
      entry.snapshotSettleTimer = null;
      if (control.admission.isReplayPending() && !entry.hasRenderedOutput) {
        setReplayStatus(entry, SNAPSHOT_UNAVAILABLE_MESSAGE);
      }
    }, SNAPSHOT_SETTLE_WARN_MS);
    armHealthDeadline(control);
    void TerminalOutputAPI.ready(
      control.sessionId,
      activation.generation,
      activation.snapshot.sequence
    )
      .then((state) => settleReady(control, activation, state))
      .catch(() => settleReadyFailure(control));
  };

  const settleReady = (
    control: EntryControl,
    activation: TerminalOutputActivation,
    state: TerminalOutputControlState
  ): void => {
    if (control.epoch !== selectionEpoch || control.activation !== "active") {
      return; // superseded before the ready result linearized
    }
    switch (state.kind) {
      case "active": {
        if (
          state.sessionId !== control.sessionId ||
          parseCanonicalCounter(state.generation) !== control.generation
        ) {
          control.activationReadyRejections += 1;
          sealAndRecover(control);
          return;
        }
        control.activationReadyAcknowledgements += 1;
        const snapshot = activation.snapshot;
        if (
          snapshot.rows !== control.entry.terminal.rows ||
          snapshot.cols !== control.entry.terminal.cols
        ) {
          resizeTerminalForSnapshot(control.entry, snapshot.cols, snapshot.rows);
        }
        // Render ONLY the exact activation payload; never a second fetch.
        control.admission.renderSnapshot(snapshot.data, snapshot.sequence);
        // The snapshot may have resized xterm to the PTY's dimensions; re-fit
        // the tile and let the existing sendPtyResize dedup suppress the
        // redundant same-size resize (#973).
        if (visibleSessionId === control.sessionId) {
          scheduleViewportSync(control.sessionId);
        }
        break;
      }
      case "resyncRequired":
      case "stale":
        control.activationReadyRejections += 1;
        sealAndRecover(control);
        break;
      case "awaitingFrontendReady":
        control.activationReadyTimeouts += 1;
        sealAndRecover(control);
        break;
      case "recoveryError":
        control.activationReadyRejections += 1;
        sealAndRecover(control);
        break;
      case "inactive":
        control.activationReadyRejections += 1;
        control.admission.sealGeneration();
        control.activation = "idle";
        break;
    }
  };

  const settleReadyFailure = (control: EntryControl): void => {
    if (control.epoch !== selectionEpoch || control.activation !== "active") {
      return;
    }
    // Transport failure on readiness: seal the just-committed state and enter
    // the one recovery lane (plan 5.4.2).
    sealAndRecover(control);
  };

  const settleReconciliation = (control: EntryControl): void => {
    const record = control.reconciliation;
    if (!record || record.aDeactivation === "pending") {
      return;
    }
    const outcome = record.bAttempt?.outcome;
    if (outcome === undefined || outcome === "pending") {
      return;
    }
    control.reconciliation = null;

    if (outcome.kind !== "activated") {
      // B recoveryError: a failed b1 attempt that never auto-retries.
      return;
    }
    const gB = parseCanonicalCounter(outcome.activation.generation);
    const sB = parseCanonicalCounter(outcome.activation.snapshot.sequence);
    if (gB === null || sB === null) {
      control.activation = "idle"; // malformed payload: fail closed
      return;
    }
    if (gB > record.causeGeneration) {
      // B linearized after A: commit B exactly once from its own snapshot.
      if (
        control.epoch === selectionEpoch &&
        control.activation === "activating"
      ) {
        commitActivation(control, gB, outcome.activation);
      }
      return;
    }
    if (gB < record.causeGeneration) {
      // A displaced B: discard b1 (no ready/ack/write/commit); issue exactly
      // one b2 only after A teardown has settled (it has, here).
      if (control.epoch === selectionEpoch) {
        startActivationAttempt(control.sessionId);
      }
      return;
    }
    // Equal generations are impossible: fail-closed invariant error.
    control.activation = "idle";
  };

  const reconcileStaleSuccess = (
    aSessionId: string,
    aGeneration: CanonicalCounter,
    aActivation: TerminalOutputActivation
  ): void => {
    const bControl = controls.get(visibleSessionId ?? "");
    if (!bControl || bControl.epoch !== selectionEpoch) {
      return; // no current desired selection
    }
    if (bControl.activation === "active" && bControl.generation !== null) {
      const gB = bControl.generation;
      if (gB > aGeneration) {
        // B linearized after A: preserve B unchanged; A deactivation is stale.
        void TerminalOutputAPI.deactivate(aSessionId, aActivation.generation).catch(
          () => undefined
        );
        return;
      }
      // gA > gB: A displaced B. Synchronously seal/dispose B/gB before A
      // teardown, then issue exactly one fresh B attempt (b2) if B's epoch is
      // still current. The control record survives with its epoch so the
      // later re-activation can validate it.
      const bSessionId = bControl.sessionId;
      const bEpoch = bControl.epoch;
      const bAttempt = bControl.attempt;
      disposeEntry(bSessionId);
      const b2Control = createControl(
        bSessionId,
        registry.get(bSessionId) ?? bControl.entry
      );
      b2Control.epoch = bEpoch;
      b2Control.attempt = bAttempt;
      controls.set(bSessionId, b2Control);
      void TerminalOutputAPI.deactivate(aSessionId, aActivation.generation)
        .then(() => {
          if (
            b2Control.epoch === selectionEpoch &&
            controls.get(bSessionId) === b2Control
          ) {
            startActivationAttempt(bSessionId);
          }
        })
        .catch(() => undefined);
      return;
    }
    if (bControl.reconciliation) {
      return; // one reconciliation record at a time
    }
    // B is Activating(b1) (or idle): retain the outstanding attempt/result.
    const record: ReconcileRecord = {
      causeGeneration: aGeneration,
      revision: bControl.epoch,
      bKnownGeneration: null,
      bAttempt:
        bControl.activation === "activating"
          ? { attempt: bControl.attempt, outcome: "pending" }
          : null,
      aDeactivation: "pending",
    };
    bControl.reconciliation = record;
    void TerminalOutputAPI.deactivate(aSessionId, aActivation.generation)
      .then((state) => {
        if (bControl.reconciliation !== record) {
          return;
        }
        record.aDeactivation = state;
        settleReconciliation(bControl);
      })
      .catch(() => {
        if (bControl.reconciliation !== record) {
          return;
        }
        // Transport error on A teardown: fail closed, treat as settled-stale.
        record.aDeactivation = { kind: "stale" };
        settleReconciliation(bControl);
      });
  };

  const settleActivationResult = (
    sessionId: string,
    attempt: number,
    result: TerminalOutputActivationResult
  ): void => {
    const control = controls.get(sessionId);
    if (!control) {
      return;
    }
    const current =
      control.epoch === selectionEpoch &&
      control.attempt === attempt &&
      control.activation === "activating";

    if (!current) {
      // Stale-A result.
      if (result.kind === "activated") {
        const generation = parseCanonicalCounter(result.activation.generation);
        if (generation !== null) {
          reconcileStaleSuccess(sessionId, generation, result.activation);
        }
      }
      // A stale recoveryError is preflight non-mutating: nothing to do.
      return;
    }

    if (result.kind === "recoveryError") {
      // Matching preflight failure: discard only this local attempt and
      // reclaim its partially created entry (plan 7 failure matrix).
      if (
        control.reconciliation?.bAttempt?.attempt === attempt &&
        control.reconciliation?.bAttempt?.outcome === "pending"
      ) {
        control.reconciliation.bAttempt = { attempt, outcome: result };
        settleReconciliation(control);
        return;
      }
      control.activation = "idle";
      control.generation = null;
      if (control.admission.stateKind() === "idle") {
        disposeEntry(sessionId);
      }
      return;
    }

    const activation = result.activation;
    if (activation.sessionId !== sessionId) {
      control.activation = "idle"; // fail-closed invariant
      return;
    }
    const generation = parseCanonicalCounter(activation.generation);
    const snapshotSeq = parseCanonicalCounter(activation.snapshot.sequence);
    if (generation === null || snapshotSeq === null) {
      control.activation = "idle"; // malformed payload: fail closed
      return;
    }

    if (
      control.reconciliation !== null &&
      control.reconciliation.bAttempt !== null &&
      control.reconciliation.bAttempt.attempt === attempt
    ) {
      // B(b1) settles through the reconciliation record.
      const record = control.reconciliation;
      record.bKnownGeneration = generation;
      record.bAttempt = { attempt, outcome: result };
      settleReconciliation(control);
      return;
    }

    commitActivation(control, generation, activation);
  };

  const settleActivationError = (
    sessionId: string,
    attempt: number,
    error: unknown
  ): void => {
    const control = controls.get(sessionId);
    if (!control || control.attempt !== attempt || control.epoch !== selectionEpoch) {
      return;
    }
    // Fail closed: discard the local attempt; the session stays eligible for
    // the next explicit user activation. No automatic retry loop.
    control.activation = "idle";
    control.generation = null;
    console.warn(`[terminal] activate_terminal_output ${sessionId} failed:`, error);
  };

  const selectSession = (sessionId: string): void => {
    if (visibleSessionId !== null && visibleSessionId !== sessionId) {
      const oldControl = controls.get(visibleSessionId);
      const oldEntry = registry.get(visibleSessionId);
      if (oldEntry) {
        if (
          oldControl &&
          (oldControl.admission.isReplayPending() ||
            oldControl.admission.stateKind() === "sealed" ||
            oldControl.recoveryInFlight)
        ) {
          // A newer selection cancels an older replay before allocating its
          // entry (plan 14.4); sealed/recovering entries are dead weight.
          disposeEntry(visibleSessionId);
        } else {
          oldEntry.container.hidden = true;
        }
      }
    }

    visibleSessionId = sessionId;
    selectionEpoch += 1;
    // #1355 - as in startActivationAttempt, this must precede the activate()
    // call below so includeHistory reads a freshly resolved entry.
    const entry = registry.activate(sessionId, createSessionTerminal);
    entry.container.hidden = false;
    registry.setVisible(sessionId);
    entry.terminal.focus();

    let control = controls.get(sessionId);
    if (!control) {
      control = createControl(sessionId, entry);
      controls.set(sessionId, control);
    } else {
      control.entry = entry;
    }
    control.epoch = selectionEpoch;
    control.detached = false;
    cancelHealth(control);
    control.activation = "activating";
    control.generation = null;
    control.generationStr = null;
    control.recoveryInFlight = false;
    control.reconciliation = null;
    control.admission.reset();

    scheduleViewportSync(sessionId);

    if (isBrowser) {
      // Legacy browser/websocket fallback: no #1283 activation protocol.
      control.activation = "active";
      return;
    }

    control.attempt += 1;
    const attempt = control.attempt;
    void TerminalOutputAPI.activate(sessionId, !entry.hasRenderedOutput)
      .then((result) => settleActivationResult(sessionId, attempt, result))
      .catch((error) => settleActivationError(sessionId, attempt, error));
  };

  const handleBrowserPtyOutput = (event: PtyOutputEvent): void => {
    // Legacy websocket fallback: raw bytes without the #1283 protocol.
    const legacy = event as unknown as { sessionId: string; data?: number[] };
    if (!Array.isArray(legacy.data)) {
      return;
    }
    if (legacy.sessionId !== visibleSessionId) {
      return; // only the visible terminal receives writes
    }
    const entry = registry.get(legacy.sessionId);
    if (!entry || entry.destroyed) {
      return;
    }
    writeTerminalBytes(entry, new Uint8Array(legacy.data));
  };

  const handlePtyOutput = (event: PtyOutputEvent): void => {
    if (isBrowser) {
      handleBrowserPtyOutput(event);
      return;
    }
    const control = controls.get(event.sessionId);
    if (!control) {
      return; // post-removal chunk: no state may be recreated
    }
    // Guard: current selection, active generation, matching epoch, unsealed
    // admission, not detached (plan 14.4).
    if (
      control.epoch !== selectionEpoch ||
      control.activation !== "active" ||
      control.generation === null ||
      control.detached
    ) {
      control.admission.noteRejected();
      return;
    }
    if (event.kind === "data") {
      const generation = parseCanonicalCounter(event.generation);
      if (generation === null || generation !== control.generation) {
        control.admission.noteRejected();
        return;
      }
    }
    const verdict = control.admission.accept(event);
    if (verdict === "accepted" || verdict === "ackedSnapshotRepresented") {
      maybeReportOrdinaryTelemetry(control);
    }
  };

  onMount(async () => {
    resizeObserver = new ResizeObserver(() => {
      if (visibleSessionId) {
        scheduleViewportSync(visibleSessionId);
      }
    });
    resizeObserver.observe(hostRef);

    unlistenPtyOutput = await onPtyOutput(handlePtyOutput);

    unlistenSessionDestroyed = await onSessionDestroyed(({ id }) => {
      disposeEntry(id);
    });

    if (!props.lockedSessionId) {
      unlistenTerminalDetached = await onTerminalDetached(({ sessionId }) => {
        // Plan 5.5.3: record detached state only; never create a hidden
        // Xterm/WebGL terminal merely because the event arrived.
        const control = controls.get(sessionId);
        if (control) {
          control.detached = true;
        }
      });
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
      return;
    }

    selectSession(sessionId);
  });

  onCleanup(() => {
    unlistenPtyOutput?.();
    unlistenSessionDestroyed?.();
    unlistenTerminalDetached?.();
    resizeObserver?.disconnect();

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
