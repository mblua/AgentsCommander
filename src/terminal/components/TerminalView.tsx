import { Component, createEffect, createSignal, onCleanup, onMount } from "solid-js";
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
  PtyTerminalAttachObservation,
  PtyTerminalAttachOutcome,
  PtyTerminalAttachTransitionKind,
  PtyTerminalOutputActivation,
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

const ATTACHMENT_DEADLINE_MS = 5_000;
const SNAPSHOT_RECONCILE_LIMIT_BYTES = 2 * 1024 * 1024;
const ATTACHMENT_IPC_ATTEMPT_TIMEOUT_MS = 500;
const ATTACHMENT_IPC_RETRY_DELAY_MS = 25;
const ATTACHMENT_OBSERVATION_MAX_ATTEMPTS = 3;
const ATTACHMENT_CLEANUP_BATCH_ATTEMPTS = 3;
const ATTACHMENT_CLEANUP_MAX_ATTEMPTS = 6;
const ATTACHMENT_CLEANUP_MAX_PENDING = 8;

const PTY_RESIZE_RETRY_DELAY_MS = 120;
const PTY_RESIZE_MAX_RETRIES = 3;

interface AttachmentOwner {
  readonly sessionId: string;
  readonly documentEpoch: string;
  readonly generation: number;
}

type AttachmentObservationStage = "postWrite" | "postFit" | "settled" | "aborted";
type AttachmentObservationProgress = "none" | "postWrite" | "postFit" | "terminal";

interface AttachmentObservationFailure {
  readonly stage: AttachmentObservationStage;
  readonly attempts: number;
}

type ExactOwnerCleanupKind = "detach" | "cancel";

interface ExactOwnerCleanupState {
  readonly kind: ExactOwnerCleanupKind;
  readonly owner: AttachmentOwner;
  generationDominanceProven: boolean;
  attempts: number;
  diagnosticReported: boolean;
  active: Promise<boolean> | null;
}

interface AttachmentTransaction {
  readonly owner: AttachmentOwner;
  readonly transitionKind: PtyTerminalAttachTransitionKind;
  readonly startedAt: number;
  fetchMicros?: number;
  writeMicros?: number;
  fitMicros?: number;
  resizeMicros?: number;
  retainedEventCount: number;
  retainedByteCount: number;
  replayBarrierCompleted: boolean;
  retainedBarrierCompleted: boolean;
  expectedActiveScreenHasText?: boolean;
  observedActiveScreenHasText?: boolean;
  expectedBottomLineHasText?: boolean;
  observedBottomLineHasText?: boolean;
  observationProgress: AttachmentObservationProgress;
  observationFailure: AttachmentObservationFailure | null;
  aborting: boolean;
}

interface TerminalBufferObservation {
  readonly activeBuffer: "normal" | "alternate";
  readonly viewportY: number;
  readonly baseY: number;
  readonly bufferLength: number;
  readonly visibleRowCount: number;
  readonly missingVisibleRowCount: number;
  readonly cellsPresent: boolean;
  readonly activeScreenHasText: boolean;
  readonly activeBottomLineHasText: boolean;
}

interface TerminalGeometryObservation {
  readonly containerConnected: boolean;
  readonly xtermConnected: boolean;
  readonly screenConnected: boolean;
  readonly elementWidth: number;
  readonly elementHeight: number;
  readonly screenWidth: number;
  readonly screenHeight: number;
  readonly canvasWidth?: number;
  readonly canvasHeight?: number;
}

type ResizeConfirmation = "confirmed" | "deduplicated";

const TerminalView: Component<TerminalViewProps> = (props) => {
  let hostRef!: HTMLDivElement;
  let visibleSessionId: string | null = null;
  let resizeObserver: ResizeObserver | null = null;
  let unlistenPtyOutput: UnlistenFn | null = null;
  let unlistenSessionDestroyed: UnlistenFn | null = null;
  let unlistenTerminalDetached: UnlistenFn | null = null;
  let unlistenCloseRequested: UnlistenFn | null = null;
  const [documentEpoch, setDocumentEpoch] = createSignal<string | null>(null);
  let lastSelectedSessionId: string | null = null;
  let viewDisposed = false;

  // #1363 - this window's single output attachment, and the serialization that
  // keeps it honest. `attachedOwner` is what the BACKEND holds for this
  // window; `desiredOwner` is what the latest transition asked for. Attach
  // and detach are async invokes whose completion order is not their call
  // order, so a fast A -> B -> A switch could otherwise land the first
  // detach(A) after the final attach(A) and leave this window rendering a
  // session it is no longer attached to: a silent freeze indistinguishable
  // from #1363 itself. Every normal ownership transition therefore runs on
  // one promise chain and re-checks the desired state after each await.
  // Exact-owner cleanup may also clear `attachedOwner`, but only behind its
  // exact same-owner guard.
  let attachedOwner: AttachmentOwner | null = null;
  let desiredOwner: AttachmentOwner | null = null;
  let attachChain: Promise<void> = Promise.resolve();
  const exactOwnerCleanupStates = new Map<string, ExactOwnerCleanupState>();
  let attachChainFailureReported = false;

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
    if (desiredOwner?.sessionId === sessionId) {
      desiredOwner = null;
    }
    if (attachedOwner?.sessionId === sessionId) {
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
  const writeTerminalBytes = (
    entry: SessionTerminalEntry,
    data: Uint8Array,
    callback?: () => void,
  ): void => {
    if (data.length > 0) {
      entry.hasRenderedOutput = true;
      setReplayStatus(entry, null);
    }
    entry.terminal.write(data, callback);
  };

  const createSessionTerminal = (
    sessionId: string,
    container: HTMLDivElement
  ): Omit<
    SessionTerminalEntry,
    "sessionId" | "xtermInstanceId" | "container" | "lastActivatedAt" | "destroyed"
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
    let renderer: "webgl" | "dom" = "dom";
    let contextState: "active" | "unavailable" = "unavailable";
    try {
      webglAddon = new WebglAddon();
      webglAddon.onContextLoss(() => {
        const entry = registry.get(sessionId);
        if (
          !entry ||
          entry.destroyed ||
          entry.terminal !== terminal ||
          entry.webglAddon === null
        ) {
          return;
        }
        registry.noteWebglContextLoss();
        entry.webglAddon.dispose();
        entry.webglAddon = null;
        entry.renderer = "dom";
        entry.contextState = "lost";
      });
      terminal.loadAddon(webglAddon);
      renderer = "webgl";
      contextState = "active";
    } catch {
      webglAddon?.dispose();
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
      if (entry.attachmentSettlePending) {
        entry.deferredViewportSync = true;
        return;
      }
      clearResizeRetry(entry);
      entry.resizeRetryAttempts = 0;
      void requestPtyResize(
        sessionId,
        entry,
        { cols, rows },
        entry.attachGeneration,
        false,
      ).catch(() => undefined);
    });

    return {
      terminal,
      fitAddon,
      webglAddon,
      renderer,
      contextState,
      replayStatus,
      hasRenderedOutput: false,
      snapshotResizeSuppressed: false,
      inputBuffer: "",
      spawnViewport,
      confirmedViewport: spawnViewport,
      resizeOperationToken: 0,
      inFlightResize: null,
      spawnDriftReported: false,
      resizeRetryTimer: null,
      resizeRetryAttempts: 0,
      resizeRetryExhaustion: null,
      attachmentDeadlineTimer: null,
      firstAttachmentRaf: null,
      secondAttachmentRaf: null,
      ordinaryViewportRaf: null,
      attachGeneration: null,
      attachmentAbortController: null,
      bottomSettledGeneration: null,
      attachmentSettlePending: false,
      deferredViewportSync: false,
      observationProgress: "none",
      observationChain: Promise.resolve(),
      snapshotReplayPending: false,
      pendingSnapshotEvents: [],
      pendingSnapshotBytes: 0,
      snapshotReconcileDiscarded: false,
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

  const sameViewport = (left: PtyViewport | null, right: PtyViewport): boolean =>
    left !== null && left.cols === right.cols && left.rows === right.rows;

  const sameOwner = (
    left: AttachmentOwner | null,
    right: AttachmentOwner,
  ): boolean =>
    left !== null &&
    left.sessionId === right.sessionId &&
    left.documentEpoch === right.documentEpoch &&
    left.generation === right.generation;

  const ownerCleanupKey = (
    kind: ExactOwnerCleanupKind,
    owner: AttachmentOwner,
  ): string =>
    JSON.stringify([kind, owner.sessionId, owner.documentEpoch, owner.generation]);

  const waitForAttachmentRetry = (attempt: number): Promise<void> =>
    new Promise((resolve) => {
      setTimeout(resolve, ATTACHMENT_IPC_RETRY_DELAY_MS * attempt);
    });

  const awaitAttachmentIpc = <T,>(operation: Promise<T>): Promise<T> =>
    new Promise<T>((resolve, reject) => {
      const timeout = setTimeout(
        () => reject(new Error("attachmentIpcTimeout")),
        ATTACHMENT_IPC_ATTEMPT_TIMEOUT_MS,
      );
      operation.then(
        (value) => {
          clearTimeout(timeout);
          resolve(value);
        },
        () => {
          clearTimeout(timeout);
          reject(new Error("attachmentIpcRejected"));
        },
      );
    });

  const reportExactOwnerCleanupFailure = (state: ExactOwnerCleanupState): void => {
    if (state.diagnosticReported) {
      return;
    }
    state.diagnosticReported = true;
    console.warn(
      `[terminal-snapshot] event=exact_owner_cleanup kind=${state.kind} outcome=rejected ` +
        `sessionId=${state.owner.sessionId} documentEpoch=${state.owner.documentEpoch} ` +
        `attachGeneration=${state.owner.generation} attempts=${state.attempts}`,
    );
  };

  const invokeExactOwnerCleanup = (
    kind: ExactOwnerCleanupKind,
    owner: AttachmentOwner,
  ): Promise<void> =>
    kind === "detach"
      ? TerminalOutputAPI.detachOutput(
          owner.sessionId,
          owner.documentEpoch,
          owner.generation,
        )
      : TerminalOutputAPI.cancelOutput(
          owner.sessionId,
          owner.documentEpoch,
          owner.generation,
        );

  const reconcileExactOwnerCleanup = (
    kind: ExactOwnerCleanupKind,
    owner: AttachmentOwner,
    generationDominanceProven: boolean,
  ): Promise<boolean> => {
    const key = ownerCleanupKey(kind, owner);
    const dominatedByRetainedState = [...exactOwnerCleanupStates.values()].some(
      (candidate) =>
        candidate.generationDominanceProven &&
        candidate.owner.documentEpoch === owner.documentEpoch &&
        candidate.owner.generation > owner.generation,
    );
    if (dominatedByRetainedState) {
      exactOwnerCleanupStates.delete(key);
      return Promise.resolve(true);
    }
    if (generationDominanceProven) {
      for (const [candidateKey, candidate] of exactOwnerCleanupStates) {
        if (
          candidate.owner.documentEpoch === owner.documentEpoch &&
          candidate.owner.generation < owner.generation
        ) {
          exactOwnerCleanupStates.delete(candidateKey);
        }
      }
    }
    let state = exactOwnerCleanupStates.get(key);
    if (!state) {
      if (exactOwnerCleanupStates.size >= ATTACHMENT_CLEANUP_MAX_PENDING) {
        console.warn(
          `[terminal-snapshot] event=exact_owner_cleanup kind=${kind} outcome=capacity_exceeded ` +
            `sessionId=${owner.sessionId} documentEpoch=${owner.documentEpoch} ` +
            `attachGeneration=${owner.generation}`,
        );
        return Promise.resolve(false);
      }
      state = {
        kind,
        owner,
        generationDominanceProven,
        attempts: 0,
        diagnosticReported: false,
        active: null,
      };
      exactOwnerCleanupStates.set(key, state);
    } else if (generationDominanceProven) {
      state.generationDominanceProven = true;
    }
    if (state.active) {
      return state.active;
    }
    if (state.attempts >= ATTACHMENT_CLEANUP_MAX_ATTEMPTS) {
      reportExactOwnerCleanupFailure(state);
      return Promise.resolve(false);
    }

    const cleanupState = state;
    const remainingAttempts = Math.min(
      ATTACHMENT_CLEANUP_BATCH_ATTEMPTS,
      ATTACHMENT_CLEANUP_MAX_ATTEMPTS - cleanupState.attempts,
    );
    const operation = (async (): Promise<boolean> => {
      for (let attempt = 1; attempt <= remainingAttempts; attempt += 1) {
        cleanupState.attempts += 1;
        try {
          await awaitAttachmentIpc(
            invokeExactOwnerCleanup(cleanupState.kind, cleanupState.owner),
          );
          exactOwnerCleanupStates.delete(key);
          if (sameOwner(attachedOwner, cleanupState.owner)) {
            attachedOwner = null;
          }
          return true;
        } catch {
          if (attempt < remainingAttempts) {
            await waitForAttachmentRetry(attempt);
          }
        }
      }
      reportExactOwnerCleanupFailure(cleanupState);
      return false;
    })();
    cleanupState.active = operation;
    void operation.finally(() => {
      cleanupState.active = null;
    });
    return operation;
  };

  const reconcileFailedExactOwnerCleanups = async (): Promise<boolean> => {
    const pending = [...exactOwnerCleanupStates.values()];
    if (pending.length === 0) {
      return true;
    }
    const results = await Promise.all(
      pending.map((state) =>
        reconcileExactOwnerCleanup(
          state.kind,
          state.owner,
          state.generationDominanceProven,
        ),
      ),
    );
    return results.every(Boolean);
  };

  const isCurrentAttachment = (
    entry: SessionTerminalEntry,
    owner: AttachmentOwner,
  ): boolean =>
    !viewDisposed &&
    registry.get(owner.sessionId) === entry &&
    !entry.destroyed &&
    visibleSessionId === owner.sessionId &&
    sameOwner(desiredOwner, owner) &&
    entry.attachGeneration === owner.generation &&
    entry.attachmentAbortController !== null &&
    !entry.attachmentAbortController.signal.aborted;

  const isCurrentResizeOwner = (
    sessionId: string,
    entry: SessionTerminalEntry,
    generation: number | null,
  ): boolean => {
    if (
      viewDisposed ||
      registry.get(sessionId) !== entry ||
      entry.destroyed ||
      visibleSessionId !== sessionId ||
      entry.attachGeneration !== generation
    ) {
      return false;
    }
    if (generation === null) {
      return isBrowser;
    }
    return (
      desiredOwner?.sessionId === sessionId &&
      desiredOwner.generation === generation &&
      entry.attachmentAbortController !== null &&
      !entry.attachmentAbortController.signal.aborted
    );
  };

  const clearResizeRetry = (entry: SessionTerminalEntry): void => {
    if (entry.resizeRetryTimer !== null) {
      clearTimeout(entry.resizeRetryTimer);
      entry.resizeRetryTimer = null;
    }
  };

  const reportResizeRetryExhaustion = (
    sessionId: string,
    entry: SessionTerminalEntry,
    viewport: PtyViewport,
    generation: number | null,
  ): void => {
    const reported = entry.resizeRetryExhaustion;
    if (
      reported !== null &&
      reported.generation === generation &&
      sameViewport(reported.viewport, viewport)
    ) {
      return;
    }
    entry.resizeRetryExhaustion = {
      generation,
      viewport: { ...viewport },
    };
    console.warn(
      `[terminal-snapshot] event=pty_resize_retry outcome=exhausted ` +
        `sessionId=${sessionId} attachGeneration=${generation} ` +
        `cols=${viewport.cols} rows=${viewport.rows} attempts=${entry.resizeRetryAttempts}`,
    );
  };

  const scheduleResizeRetry = (
    sessionId: string,
    entry: SessionTerminalEntry,
    viewport: PtyViewport,
    generation: number | null,
    failedOperationToken: number,
  ): void => {
    if (
      entry.resizeRetryTimer !== null ||
      !isCurrentResizeOwner(sessionId, entry, generation) ||
      entry.attachmentSettlePending
    ) {
      return;
    }
    if (entry.resizeRetryAttempts >= PTY_RESIZE_MAX_RETRIES) {
      reportResizeRetryExhaustion(sessionId, entry, viewport, generation);
      return;
    }
    entry.resizeRetryAttempts += 1;
    const attempt = entry.resizeRetryAttempts;
    entry.resizeRetryTimer = setTimeout(() => {
      entry.resizeRetryTimer = null;
      if (
        !isCurrentResizeOwner(sessionId, entry, generation) ||
        entry.attachmentSettlePending ||
        entry.resizeOperationToken !== failedOperationToken
      ) {
        return;
      }
      void requestPtyResize(sessionId, entry, viewport, generation, false).catch(
        () => undefined,
      );
    }, PTY_RESIZE_RETRY_DELAY_MS * attempt);
  };

  const requestPtyResize = async (
    sessionId: string,
    entry: SessionTerminalEntry,
    viewport: PtyViewport,
    generation: number | null,
    authoritative: boolean,
  ): Promise<ResizeConfirmation> => {
    if (!isCurrentResizeOwner(sessionId, entry, generation)) {
      throw new Error("staleResizeOwner");
    }
    if (sameViewport(entry.confirmedViewport, viewport)) {
      return "deduplicated";
    }

    reportSpawnSizeDrift(sessionId, entry, viewport.cols, viewport.rows);
    entry.resizeOperationToken += 1;
    const operation = {
      token: entry.resizeOperationToken,
      generation,
      viewport,
    };
    entry.inFlightResize = operation;

    try {
      await PtyAPI.resize(sessionId, viewport.cols, viewport.rows);
    } catch {
      if (
        isCurrentResizeOwner(sessionId, entry, generation) &&
        entry.inFlightResize?.token === operation.token
      ) {
        entry.inFlightResize = null;
        if (!authoritative) {
          scheduleResizeRetry(
            sessionId,
            entry,
            viewport,
            generation,
            operation.token,
          );
        }
      }
      throw new Error("resizeFailed");
    }

    if (
      !isCurrentResizeOwner(sessionId, entry, generation) ||
      entry.inFlightResize?.token !== operation.token
    ) {
      throw new Error("staleResizeCompletion");
    }
    entry.inFlightResize = null;
    entry.confirmedViewport = viewport;
    entry.resizeRetryAttempts = 0;
    entry.resizeRetryExhaustion = null;
    clearResizeRetry(entry);
    return "confirmed";
  };

  const syncViewport = (
    sessionId: string,
    entry: SessionTerminalEntry,
    generation: number | null,
  ): void => {
    if (!isCurrentResizeOwner(sessionId, entry, generation)) {
      return;
    }
    if (entry.attachmentSettlePending) {
      entry.deferredViewportSync = true;
      return;
    }
    entry.snapshotResizeSuppressed = true;
    try {
      entry.fitAddon.fit();
    } finally {
      entry.snapshotResizeSuppressed = false;
    }
    const viewport = { cols: entry.terminal.cols, rows: entry.terminal.rows };
    clearResizeRetry(entry);
    entry.resizeRetryAttempts = 0;
    void requestPtyResize(sessionId, entry, viewport, generation, false).catch(
      () => undefined,
    );
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

  const scheduleViewportSync = (
    sessionId: string,
    generation: number | null,
  ): void => {
    const entry = registry.get(sessionId);
    if (!entry || entry.destroyed || entry.attachGeneration !== generation) {
      return;
    }
    if (entry.attachmentSettlePending) {
      entry.deferredViewportSync = true;
      return;
    }
    if (entry.ordinaryViewportRaf !== null) {
      cancelAnimationFrame(entry.ordinaryViewportRaf);
    }
    entry.ordinaryViewportRaf = requestAnimationFrame(() => {
      entry.ordinaryViewportRaf = null;
      if (!isCurrentResizeOwner(sessionId, entry, generation)) {
        return;
      }
      if (entry.attachmentSettlePending) {
        entry.deferredViewportSync = true;
        return;
      }
      syncViewport(sessionId, entry, generation);
    });
  };

  const resizeTerminalForSnapshot = (
    entry: SessionTerminalEntry,
    owner: AttachmentOwner,
    cols: number,
    rows: number
  ): boolean => {
    if (!isCurrentAttachment(entry, owner)) {
      return false;
    }
    entry.snapshotResizeSuppressed = true;
    try {
      entry.terminal.resize(cols, rows);
    } finally {
      entry.snapshotResizeSuppressed = false;
    }
    return isCurrentAttachment(entry, owner);
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
    entry.snapshotReconcileDiscarded = true;
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

  const takeRetainedEvents = (entry: SessionTerminalEntry): readonly PtyOutputEvent[] => {
    const events = entry.pendingSnapshotEvents;
    entry.snapshotReplayPending = false;
    entry.pendingSnapshotEvents = [];
    entry.pendingSnapshotBytes = 0;
    return events;
  };

  const clearAttachmentDeadline = (entry: SessionTerminalEntry): void => {
    if (entry.attachmentDeadlineTimer !== null) {
      clearTimeout(entry.attachmentDeadlineTimer);
      entry.attachmentDeadlineTimer = null;
    }
  };

  const cancelAttachmentFrames = (entry: SessionTerminalEntry): void => {
    if (entry.firstAttachmentRaf !== null) {
      cancelAnimationFrame(entry.firstAttachmentRaf);
      entry.firstAttachmentRaf = null;
    }
    if (entry.secondAttachmentRaf !== null) {
      cancelAnimationFrame(entry.secondAttachmentRaf);
      entry.secondAttachmentRaf = null;
    }
  };

  const elapsedMicros = (startedAt: number): number =>
    Math.min(60_000_000, Math.max(0, Math.round((performance.now() - startedAt) * 1000)));

  const observeTerminalBuffer = (entry: SessionTerminalEntry): TerminalBufferObservation => {
    const active = entry.terminal.buffer.active;
    let visibleRowCount = 0;
    let missingVisibleRowCount = 0;
    let cellsPresent = true;
    let activeScreenHasText = false;
    let activeBottomLineHasText = false;
    for (let row = 0; row < entry.terminal.rows; row += 1) {
      const line = active.getLine(active.baseY + row);
      if (!line) {
        missingVisibleRowCount += 1;
        cellsPresent = false;
        continue;
      }
      visibleRowCount += 1;
      let lineHasText = false;
      for (let col = 0; col < entry.terminal.cols; col += 1) {
        const cell = line.getCell(col);
        if (!cell) {
          cellsPresent = false;
          continue;
        }
        if (/\S/u.test(cell.getChars())) {
          lineHasText = true;
          activeScreenHasText = true;
        }
      }
      if (row === entry.terminal.rows - 1) {
        activeBottomLineHasText = lineHasText;
      }
    }
    return {
      activeBuffer: active.type,
      viewportY: active.viewportY,
      baseY: active.baseY,
      bufferLength: active.length,
      visibleRowCount,
      missingVisibleRowCount,
      cellsPresent,
      activeScreenHasText,
      activeBottomLineHasText,
    };
  };

  const roundedPixel = (value: number): number =>
    Number.isFinite(value) ? Math.min(131_072, Math.max(0, Math.round(value))) : 0;

  const observeTerminalGeometry = (
    entry: SessionTerminalEntry,
  ): TerminalGeometryObservation => {
    const element = entry.terminal.element;
    const screen = element?.querySelector<HTMLElement>(".xterm-screen") ?? null;
    const canvas =
      entry.renderer === "webgl"
        ? screen?.querySelector<HTMLCanvasElement>("canvas") ?? null
        : null;
    const elementRect = element?.getBoundingClientRect() ?? entry.container.getBoundingClientRect();
    const screenRect = screen?.getBoundingClientRect();
    return {
      containerConnected: entry.container.isConnected,
      xtermConnected: element?.isConnected === true,
      screenConnected: screen?.isConnected === true,
      elementWidth: roundedPixel(elementRect.width),
      elementHeight: roundedPixel(elementRect.height),
      screenWidth: roundedPixel(screenRect?.width ?? 0),
      screenHeight: roundedPixel(screenRect?.height ?? 0),
      ...(canvas
        ? {
            canvasWidth: roundedPixel(canvas.width),
            canvasHeight: roundedPixel(canvas.height),
          }
        : {}),
    };
  };

  const awaitWithAbort = <T,>(promise: Promise<T>, signal: AbortSignal): Promise<T> =>
    new Promise<T>((resolve, reject) => {
      if (signal.aborted) {
        reject(new Error("attachmentAborted"));
        return;
      }
      const onAbort = (): void => reject(new Error("attachmentAborted"));
      signal.addEventListener("abort", onAbort, { once: true });
      promise.then(
        (value) => {
          signal.removeEventListener("abort", onAbort);
          resolve(value);
        },
        () => {
          signal.removeEventListener("abort", onAbort);
          reject(new Error("attachmentOperationFailed"));
        },
      );
    });

  const queueWriteBarrier = (
    entry: SessionTerminalEntry,
    owner: AttachmentOwner,
    data: Uint8Array,
    validateParsedState?: () => void,
  ): Promise<void> =>
    new Promise<void>((resolve, reject) => {
      const controller = entry.attachmentAbortController;
      if (!controller || !isCurrentAttachment(entry, owner)) {
        reject(new Error("staleWriteBarrier"));
        return;
      }
      const onAbort = (): void => reject(new Error("attachmentAborted"));
      controller.signal.addEventListener("abort", onAbort, { once: true });
      try {
        writeTerminalBytes(entry, data, () => {
          controller.signal.removeEventListener("abort", onAbort);
          if (!isCurrentAttachment(entry, owner)) {
            reject(new Error("staleWriteBarrier"));
            return;
          }
          try {
            validateParsedState?.();
            resolve();
          } catch {
            reject(new Error("writeBarrierValidationFailed"));
          }
        });
      } catch {
        controller.signal.removeEventListener("abort", onAbort);
        reject(new Error("writeBarrierFailed"));
      }
    });

  const waitForAttachmentFrame = (
    entry: SessionTerminalEntry,
    owner: AttachmentOwner,
    slot: "first" | "second",
  ): Promise<void> =>
    new Promise<void>((resolve, reject) => {
      const controller = entry.attachmentAbortController;
      if (!controller || !isCurrentAttachment(entry, owner)) {
        reject(new Error("staleAnimationFrame"));
        return;
      }
      const onAbort = (): void => reject(new Error("attachmentAborted"));
      controller.signal.addEventListener("abort", onAbort, { once: true });
      const handle = requestAnimationFrame(() => {
        controller.signal.removeEventListener("abort", onAbort);
        if (slot === "first") entry.firstAttachmentRaf = null;
        else entry.secondAttachmentRaf = null;
        if (!isCurrentAttachment(entry, owner)) {
          reject(new Error("staleAnimationFrame"));
          return;
        }
        resolve();
      });
      if (slot === "first") entry.firstAttachmentRaf = handle;
      else entry.secondAttachmentRaf = handle;
    });

  const activationOutcome = (
    activation: PtyTerminalOutputActivation,
  ): PtyTerminalAttachOutcome => {
    if (activation.snapshot === null) {
      return activation.seedlessReason;
    }
    if (activation.snapshot.replayStage === "semanticHistory") {
      return "success";
    }
    return activation.snapshot.replayStage;
  };

  const buildObservation = (
    entry: SessionTerminalEntry,
    transaction: AttachmentTransaction,
    stage: "postWrite" | "postFit" | "settled" | "aborted",
    outcome: PtyTerminalAttachOutcome,
    snapshot: PtyScreenSnapshot | null,
    buffer: TerminalBufferObservation,
    geometry: TerminalGeometryObservation,
  ): PtyTerminalAttachObservation => {
    const confirmedViewport = entry.confirmedViewport;
    const backendViewport =
      snapshot !== null && (stage === "postWrite" || transaction.fitMicros === undefined)
        ? { rows: snapshot.rows, cols: snapshot.cols }
        : confirmedViewport;
    const resizeConfirmed =
      confirmedViewport !== null &&
      confirmedViewport.cols === entry.terminal.cols &&
      confirmedViewport.rows === entry.terminal.rows;
    const gridAgreement =
      backendViewport !== null &&
      backendViewport.cols === entry.terminal.cols &&
      backendViewport.rows === entry.terminal.rows;
    const visibleRowsPresent =
      buffer.visibleRowCount === entry.terminal.rows &&
      buffer.missingVisibleRowCount === 0 &&
      buffer.cellsPresent;
    const bottomPositionSatisfied = buffer.viewportY === buffer.baseY;
    return {
      sessionId: transaction.owner.sessionId,
      stage,
      documentEpoch: transaction.owner.documentEpoch,
      xtermInstanceId: entry.xtermInstanceId,
      viewKind: props.lockedSessionId ? "externalized" : "embedded",
      transitionKind: transaction.transitionKind,
      attachGeneration: transaction.owner.generation,
      sequence: snapshot?.sequence ?? 0,
      outcome,
      xtermRows: entry.terminal.rows,
      xtermCols: entry.terminal.cols,
      ...(backendViewport === null
        ? {}
        : {
            parserRows: backendViewport.rows,
            parserCols: backendViewport.cols,
            conptyRows: backendViewport.rows,
            conptyCols: backendViewport.cols,
          }),
      historyRequested: true,
      retainedEventCount: transaction.retainedEventCount,
      retainedByteCount: transaction.retainedByteCount,
      renderer: entry.renderer,
      contextState: entry.contextState,
      viewportY: buffer.viewportY,
      baseY: buffer.baseY,
      bufferLength: buffer.bufferLength,
      visibleRowCount: buffer.visibleRowCount,
      missingVisibleRowCount: buffer.missingVisibleRowCount,
      containerConnected: geometry.containerConnected,
      xtermConnected: geometry.xtermConnected,
      screenConnected: geometry.screenConnected,
      elementWidth: geometry.elementWidth,
      elementHeight: geometry.elementHeight,
      screenWidth: geometry.screenWidth,
      screenHeight: geometry.screenHeight,
      ...(geometry.canvasWidth === undefined
        ? {}
        : { canvasWidth: geometry.canvasWidth }),
      ...(geometry.canvasHeight === undefined
        ? {}
        : { canvasHeight: geometry.canvasHeight }),
      ...(transaction.fetchMicros === undefined
        ? {}
        : { fetchMicros: transaction.fetchMicros }),
      ...(transaction.writeMicros === undefined
        ? {}
        : { writeMicros: transaction.writeMicros }),
      ...(transaction.fitMicros === undefined
        ? {}
        : { fitMicros: transaction.fitMicros }),
      ...(transaction.resizeMicros === undefined
        ? {}
        : { resizeMicros: transaction.resizeMicros }),
      settleMicros: elapsedMicros(transaction.startedAt),
      totalMicros: elapsedMicros(transaction.startedAt),
      replayBarrierCompleted: transaction.replayBarrierCompleted,
      retainedBarrierCompleted: transaction.retainedBarrierCompleted,
      gridAgreement,
      resizeConfirmed,
      visibleRowsPresent,
      bottomPositionSatisfied,
      ...(transaction.expectedActiveScreenHasText === undefined
        ? {}
        : {
            expectedActiveScreenHasText:
              transaction.expectedActiveScreenHasText,
          }),
      ...(transaction.observedActiveScreenHasText === undefined
        ? {}
        : {
            observedActiveScreenHasText:
              transaction.observedActiveScreenHasText,
          }),
      ...(transaction.expectedBottomLineHasText === undefined
        ? {}
        : { expectedBottomLineHasText: transaction.expectedBottomLineHasText }),
      ...(transaction.observedBottomLineHasText === undefined
        ? {}
        : { observedBottomLineHasText: transaction.observedBottomLineHasText }),
      ...(snapshot === null
        ? {}
        : {
            snapshotRows: snapshot.rows,
            snapshotCols: snapshot.cols,
            historyIncluded: snapshot.historyIncluded,
            historyTruncated: snapshot.historyTruncated,
            historyTruncationReason: snapshot.historyTruncationReason,
            historyBoundaryHardened: snapshot.historyBoundaryHardened,
            retainedHistoryRows: snapshot.retainedHistoryRows,
            includedHistoryRows: snapshot.includedHistoryRows,
            semanticHistoryBytes: snapshot.semanticHistoryBytes,
            replayBytes: snapshot.replayBytes,
            normalScreenIncluded: snapshot.normalScreenIncluded,
            activeBuffer: snapshot.activeBuffer,
            ...(snapshot.alternateEntryMode === null
              ? {}
              : { alternateEntryMode: snapshot.alternateEntryMode }),
            replayStage: snapshot.replayStage,
            parserPrefixIncluded: true,
          }),
    };
  };

  const reportObservationFailure = (
    transaction: AttachmentTransaction,
    stage: AttachmentObservationStage,
    attempts: number,
    outcome: "rejected" | "stage_rejected",
  ): void => {
    transaction.observationFailure = { stage, attempts };
    console.warn(
      `[terminal-snapshot] event=attach_observation stage=${stage} outcome=${outcome} ` +
        `sessionId=${transaction.owner.sessionId} ` +
        `documentEpoch=${transaction.owner.documentEpoch} ` +
        `attachGeneration=${transaction.owner.generation} attempts=${attempts}`,
    );
  };

  const recordAttachmentObservation = (
    entry: SessionTerminalEntry,
    transaction: AttachmentTransaction,
    stage: AttachmentObservationStage,
    outcome: PtyTerminalAttachOutcome,
    snapshot: PtyScreenSnapshot | null,
    buffer: TerminalBufferObservation,
    geometry: TerminalGeometryObservation,
  ): Promise<boolean> => {
    const observation = Object.freeze(
      buildObservation(
        entry,
        transaction,
        stage,
        outcome,
        snapshot,
        buffer,
        geometry,
      ),
    );
    const operation = entry.observationChain.then(async (): Promise<boolean> => {
      const valid =
        (stage === "postWrite" && transaction.observationProgress === "none") ||
        (stage === "postFit" && transaction.observationProgress === "postWrite") ||
        (stage === "settled" && transaction.observationProgress === "postFit") ||
        (stage === "aborted" && transaction.observationProgress !== "terminal");
      if (!valid) {
        reportObservationFailure(transaction, stage, 0, "stage_rejected");
        return false;
      }

      for (
        let attempt = 1;
        attempt <= ATTACHMENT_OBSERVATION_MAX_ATTEMPTS;
        attempt += 1
      ) {
        try {
          await awaitAttachmentIpc(TerminalOutputAPI.recordObservation(observation));
          transaction.observationProgress =
            stage === "postWrite"
              ? "postWrite"
              : stage === "postFit"
                ? "postFit"
                : "terminal";
          transaction.observationFailure = null;
          if (entry.attachGeneration === transaction.owner.generation) {
            entry.observationProgress = transaction.observationProgress;
          }
          return true;
        } catch {
          if (attempt < ATTACHMENT_OBSERVATION_MAX_ATTEMPTS) {
            await waitForAttachmentRetry(attempt);
          }
        }
      }
      reportObservationFailure(
        transaction,
        stage,
        ATTACHMENT_OBSERVATION_MAX_ATTEMPTS,
        "rejected",
      );
      return false;
    });
    entry.observationChain = operation.then(
      () => undefined,
      () => undefined,
    );
    return operation;
  };

  const cancelBackendActivation = (
    owner: AttachmentOwner,
    generationDominanceProven = false,
  ): Promise<boolean> =>
    reconcileExactOwnerCleanup("cancel", owner, generationDominanceProven);

  const abortAttachmentGeneration = async (
    entry: SessionTerminalEntry,
    transaction: AttachmentTransaction,
    outcome: PtyTerminalAttachOutcome,
    snapshot: PtyScreenSnapshot | null = null,
  ): Promise<void> => {
    if (
      entry.attachGeneration !== transaction.owner.generation ||
      transaction.aborting
    ) {
      return;
    }
    transaction.aborting = true;
    const buffer = observeTerminalBuffer(entry);
    const geometry = observeTerminalGeometry(entry);
    clearAttachmentDeadline(entry);
    cancelAttachmentFrames(entry);
    clearResizeRetry(entry);
    entry.resizeOperationToken += 1;
    entry.inFlightResize = null;
    entry.attachmentAbortController?.abort();
    entry.attachmentSettlePending = false;
    entry.deferredViewportSync = false;
    entry.snapshotReplayPending = false;
    entry.pendingSnapshotEvents = [];
    entry.pendingSnapshotBytes = 0;
    await recordAttachmentObservation(
      entry,
      transaction,
      "aborted",
      outcome,
      snapshot,
      buffer,
      geometry,
    );
    await cancelBackendActivation(
      transaction.owner,
      sameOwner(attachedOwner, transaction.owner),
    );
    if (!entry.hasRenderedOutput) {
      setReplayStatus(entry, SNAPSHOT_UNAVAILABLE_MESSAGE);
    }
  };

  const finishAttachmentGeneration = async (
    entry: SessionTerminalEntry,
    transaction: AttachmentTransaction,
    stage: "settled" | "aborted",
    outcome: PtyTerminalAttachOutcome,
    snapshot: PtyScreenSnapshot | null,
    buffer: TerminalBufferObservation,
    geometry: TerminalGeometryObservation,
  ): Promise<boolean> => {
    if (!isCurrentAttachment(entry, transaction.owner)) {
      return false;
    }
    const accepted = await recordAttachmentObservation(
      entry,
      transaction,
      stage,
      outcome,
      snapshot,
      buffer,
      geometry,
    );
    if (!accepted || !isCurrentAttachment(entry, transaction.owner)) {
      return false;
    }
    clearAttachmentDeadline(entry);
    cancelAttachmentFrames(entry);
    entry.attachmentSettlePending = false;
    entry.snapshotReplayPending = false;
    const runDeferredSync = entry.deferredViewportSync;
    entry.deferredViewportSync = false;
    if (runDeferredSync) {
      scheduleViewportSync(
        transaction.owner.sessionId,
        transaction.owner.generation,
      );
    }
    return true;
  };

  const hasUsableRenderer = (entry: SessionTerminalEntry): boolean =>
    (entry.renderer === "webgl" && entry.contextState === "active") ||
    (entry.renderer === "dom" &&
      (entry.contextState === "lost" || entry.contextState === "unavailable"));

  const strictSettlementSatisfied = (
    entry: SessionTerminalEntry,
    transaction: AttachmentTransaction,
    buffer: TerminalBufferObservation,
    geometry: TerminalGeometryObservation,
  ): boolean => {
    const confirmed = entry.confirmedViewport;
    const rendererGeometrySatisfied =
      entry.renderer === "webgl"
        ? entry.contextState === "active" &&
          geometry.canvasWidth !== undefined &&
          geometry.canvasWidth > 0 &&
          geometry.canvasHeight !== undefined &&
          geometry.canvasHeight > 0
        : (entry.contextState === "lost" || entry.contextState === "unavailable") &&
          geometry.canvasWidth === undefined &&
          geometry.canvasHeight === undefined;
    const semanticPredicatesSatisfied =
      transaction.expectedActiveScreenHasText ===
        transaction.observedActiveScreenHasText &&
      transaction.expectedBottomLineHasText ===
        transaction.observedBottomLineHasText;
    return (
      transaction.replayBarrierCompleted &&
      transaction.retainedBarrierCompleted &&
      hasUsableRenderer(entry) &&
      rendererGeometrySatisfied &&
      geometry.containerConnected &&
      geometry.xtermConnected &&
      geometry.screenConnected &&
      geometry.elementWidth > 0 &&
      geometry.elementHeight > 0 &&
      geometry.screenWidth > 0 &&
      geometry.screenHeight > 0 &&
      buffer.bufferLength >= buffer.baseY + entry.terminal.rows &&
      buffer.visibleRowCount === entry.terminal.rows &&
      buffer.missingVisibleRowCount === 0 &&
      buffer.cellsPresent &&
      confirmed !== null &&
      confirmed.cols === entry.terminal.cols &&
      confirmed.rows === entry.terminal.rows &&
      buffer.viewportY === buffer.baseY &&
      semanticPredicatesSatisfied
    );
  };

  const runAttachmentTransaction = async (
    entry: SessionTerminalEntry,
    transaction: AttachmentTransaction,
    activation: PtyTerminalOutputActivation,
  ): Promise<void> => {
    if (!isCurrentAttachment(entry, transaction.owner)) {
      return;
    }
    const controller = entry.attachmentAbortController;
    if (!controller) {
      return;
    }
    const retainedEvents = takeRetainedEvents(entry);
    transaction.retainedEventCount = retainedEvents.length;
    transaction.retainedByteCount = retainedEvents.reduce(
      (total, event) => total + event.data.length,
      0,
    );

    let snapshot = activation.snapshot;
    let outcome = activationOutcome(activation);
    if (snapshot !== null && entry.snapshotReconcileDiscarded) {
      snapshot = null;
      outcome = "snapshotDiscarded";
    }

    const writeStartedAt = performance.now();
    let authoritativeResizePending = false;
    try {
      let replayBarrier: Promise<void>;
      let retainedBarrier: Promise<void>;
      if (snapshot !== null) {
        if (
          (entry.terminal.cols !== snapshot.cols || entry.terminal.rows !== snapshot.rows) &&
          !resizeTerminalForSnapshot(
            entry,
            transaction.owner,
            snapshot.cols,
            snapshot.rows,
          )
        ) {
          throw new Error("staleSnapshotResize");
        }
        if (!isCurrentAttachment(entry, transaction.owner)) {
          throw new Error("staleSnapshotReset");
        }
        entry.terminal.reset();
        entry.hasRenderedOutput = false;
        entry.lastAppliedSequence = snapshot.sequence;
        replayBarrier = queueWriteBarrier(
          entry,
          transaction.owner,
          new Uint8Array(snapshot.replayData),
          () => {
            const replayObservation = observeTerminalBuffer(entry);
            transaction.expectedActiveScreenHasText = snapshot.activeScreenHasText;
            transaction.observedActiveScreenHasText =
              replayObservation.activeScreenHasText;
            transaction.expectedBottomLineHasText = snapshot.activeBottomLineHasText;
            transaction.observedBottomLineHasText =
              replayObservation.activeBottomLineHasText;
            if (
              replayObservation.activeBuffer !== snapshot.activeBuffer ||
              entry.terminal.cols !== snapshot.cols ||
              entry.terminal.rows !== snapshot.rows ||
              replayObservation.bufferLength <
                replayObservation.baseY + entry.terminal.rows ||
              replayObservation.visibleRowCount !== entry.terminal.rows ||
              replayObservation.missingVisibleRowCount !== 0 ||
              !replayObservation.cellsPresent ||
              replayObservation.activeScreenHasText !== snapshot.activeScreenHasText ||
              replayObservation.activeBottomLineHasText !==
                snapshot.activeBottomLineHasText
            ) {
              throw new Error("snapshotSemanticMismatch");
            }
          },
        );
        await replayBarrier;
        transaction.replayBarrierCompleted = true;
        if (!isCurrentAttachment(entry, transaction.owner)) {
          throw new Error("staleRetainedQueue");
        }
        for (const event of retainedEvents) {
          const sequence = eventSequence(event);
          if (sequence !== null && sequence <= snapshot.sequence) {
            continue;
          }
          writeTerminalBytes(entry, new Uint8Array(event.data));
          markAppliedSequence(entry, sequence);
        }
        retainedBarrier = queueWriteBarrier(
          entry,
          transaction.owner,
          new Uint8Array(),
        );
      } else {
        replayBarrier = queueWriteBarrier(
          entry,
          transaction.owner,
          new Uint8Array(),
        );
        await replayBarrier;
        transaction.replayBarrierCompleted = true;
        retainedBarrier = queueWriteBarrier(
          entry,
          transaction.owner,
          new Uint8Array(),
        );
        if (!entry.hasRenderedOutput) {
          setReplayStatus(entry, SNAPSHOT_UNAVAILABLE_MESSAGE);
        }
      }
      await retainedBarrier;
      transaction.retainedBarrierCompleted = true;
      transaction.writeMicros = elapsedMicros(writeStartedAt);

      const postWriteAccepted = await recordAttachmentObservation(
        entry,
        transaction,
        "postWrite",
        outcome,
        snapshot,
        observeTerminalBuffer(entry),
        observeTerminalGeometry(entry),
      );
      if (!postWriteAccepted) {
        await abortAttachmentGeneration(entry, transaction, "invariantFailed", snapshot);
        return;
      }

      await waitForAttachmentFrame(entry, transaction.owner, "first");
      const fitStartedAt = performance.now();
      entry.snapshotResizeSuppressed = true;
      try {
        entry.fitAddon.fit();
      } finally {
        entry.snapshotResizeSuppressed = false;
      }
      if (!isCurrentAttachment(entry, transaction.owner)) {
        throw new Error("stalePostFit");
      }
      transaction.fitMicros = elapsedMicros(fitStartedAt);

      const viewport = {
        cols: entry.terminal.cols,
        rows: entry.terminal.rows,
      };
      const resizeStartedAt = performance.now();
      authoritativeResizePending = true;
      await awaitWithAbort(
        requestPtyResize(
          transaction.owner.sessionId,
          entry,
          viewport,
          transaction.owner.generation,
          true,
        ),
        controller.signal,
      );
      authoritativeResizePending = false;
      transaction.resizeMicros = elapsedMicros(resizeStartedAt);

      const postFitAccepted = await recordAttachmentObservation(
        entry,
        transaction,
        "postFit",
        outcome,
        snapshot,
        observeTerminalBuffer(entry),
        observeTerminalGeometry(entry),
      );
      if (!postFitAccepted) {
        await abortAttachmentGeneration(entry, transaction, "invariantFailed", snapshot);
        return;
      }

      if (
        isCurrentAttachment(entry, transaction.owner) &&
        entry.bottomSettledGeneration !== transaction.owner.generation
      ) {
        entry.terminal.scrollToBottom();
        entry.bottomSettledGeneration = transaction.owner.generation;
      }
      await waitForAttachmentFrame(entry, transaction.owner, "second");

      const finalBuffer = observeTerminalBuffer(entry);
      const finalGeometry = observeTerminalGeometry(entry);
      if (
        !isCurrentAttachment(entry, transaction.owner) ||
        !strictSettlementSatisfied(entry, transaction, finalBuffer, finalGeometry)
      ) {
        await abortAttachmentGeneration(entry, transaction, "invariantFailed", snapshot);
        return;
      }
      const settled =
        outcome === "success" ||
        outcome === "screenOnlyHistoryDisabled" ||
        outcome === "screenOnlyCheckpointUnavailable";
      const finished = await finishAttachmentGeneration(
        entry,
        transaction,
        settled ? "settled" : "aborted",
        outcome,
        snapshot,
        finalBuffer,
        finalGeometry,
      );
      if (!finished && !transaction.aborting) {
        await abortAttachmentGeneration(entry, transaction, "invariantFailed", snapshot);
      }
    } catch {
      if (controller.signal.aborted) {
        return;
      }
      await abortAttachmentGeneration(
        entry,
        transaction,
        authoritativeResizePending ? "resizeFailed" : "invariantFailed",
        snapshot,
      );
    }
  };

  let currentTransaction: AttachmentTransaction | null = null;

  const retireEntryGeneration = (entry: SessionTerminalEntry): void => {
    clearAttachmentDeadline(entry);
    cancelAttachmentFrames(entry);
    clearResizeRetry(entry);
    if (entry.ordinaryViewportRaf !== null) {
      cancelAnimationFrame(entry.ordinaryViewportRaf);
      entry.ordinaryViewportRaf = null;
    }
    entry.resizeOperationToken += 1;
    entry.inFlightResize = null;
    entry.attachmentAbortController?.abort();
    entry.attachmentAbortController = null;
    entry.attachmentSettlePending = false;
    entry.deferredViewportSync = false;
    entry.snapshotReplayPending = false;
    entry.pendingSnapshotEvents = [];
    entry.pendingSnapshotBytes = 0;
  };

  const beginAttachmentGeneration = (
    sessionId: string,
    entry: SessionTerminalEntry,
    epoch: string,
    transitionKind: PtyTerminalAttachTransitionKind,
  ): AttachmentTransaction | null => {
    const generation = terminalStore.allocateAttachmentGeneration(epoch);
    if (generation === null) {
      if (currentTransaction !== null) {
        const previousEntry = registry.get(currentTransaction.owner.sessionId);
        if (previousEntry) {
          void abortAttachmentGeneration(previousEntry, currentTransaction, "stale");
        }
        currentTransaction = null;
      }
      retireEntryGeneration(entry);
      desiredOwner = null;
      transitionAttachment(null);
      setReplayStatus(entry, SNAPSHOT_UNAVAILABLE_MESSAGE);
      return null;
    }
    if (currentTransaction !== null) {
      const previousEntry = registry.get(currentTransaction.owner.sessionId);
      if (previousEntry) {
        void abortAttachmentGeneration(
          previousEntry,
          currentTransaction,
          "stale",
        );
      }
      currentTransaction = null;
    }
    retireEntryGeneration(entry);
    const owner: AttachmentOwner = {
      sessionId,
      documentEpoch: epoch,
      generation,
    };
    const transaction: AttachmentTransaction = {
      owner,
      transitionKind,
      startedAt: performance.now(),
      retainedEventCount: 0,
      retainedByteCount: 0,
      replayBarrierCompleted: false,
      retainedBarrierCompleted: false,
      observationProgress: "none",
      observationFailure: null,
      aborting: false,
    };
    entry.attachGeneration = owner.generation;
    entry.attachmentAbortController = new AbortController();
    entry.attachmentSettlePending = true;
    entry.deferredViewportSync = false;
    entry.bottomSettledGeneration = null;
    entry.observationProgress = "none";
    entry.snapshotReplayPending = true;
    entry.snapshotReconcileDiscarded = false;
    entry.pendingSnapshotEvents = [];
    entry.pendingSnapshotBytes = 0;
    entry.confirmedViewport = null;
    desiredOwner = owner;
    currentTransaction = transaction;
    entry.attachmentDeadlineTimer = setTimeout(() => {
      entry.attachmentDeadlineTimer = null;
      if (isCurrentAttachment(entry, owner)) {
        void abortAttachmentGeneration(entry, transaction, "timeout");
        if (currentTransaction === transaction) {
          currentTransaction = null;
        }
      }
    }, ATTACHMENT_DEADLINE_MS);
    return transaction;
  };

  const transitionAttachment = (
    target: { readonly entry: SessionTerminalEntry; readonly transaction: AttachmentTransaction } | null,
  ): void => {
    if (isBrowser) {
      return;
    }
    if (target === null) {
      desiredOwner = null;
      if (currentTransaction !== null) {
        const entry = registry.get(currentTransaction.owner.sessionId);
        if (entry) {
          void abortAttachmentGeneration(entry, currentTransaction, "stale");
        }
        currentTransaction = null;
      }
    }
    attachChain = attachChain
      .then(async () => {
        const reconciled = await reconcileFailedExactOwnerCleanups();
        if (!reconciled) {
          if (target !== null && isCurrentAttachment(target.entry, target.transaction.owner)) {
            await abortAttachmentGeneration(
              target.entry,
              target.transaction,
              "invariantFailed",
            );
          }
          return;
        }
        const previous = attachedOwner;
        if (
          previous !== null &&
          (target === null || !sameOwner(previous, target.transaction.owner))
        ) {
          const detached = await reconcileExactOwnerCleanup("detach", previous, true);
          if (!detached) {
            if (
              target !== null &&
              isCurrentAttachment(target.entry, target.transaction.owner)
            ) {
              await abortAttachmentGeneration(
                target.entry,
                target.transaction,
                "invariantFailed",
              );
            }
            return;
          }
        }

        if (target === null) {
          return;
        }
        const { entry, transaction } = target;
        const owner = transaction.owner;
        if (!isCurrentAttachment(entry, owner)) {
          return;
        }
        const controller = entry.attachmentAbortController;
        if (!controller) {
          return;
        }
        try {
          await awaitWithAbort(listenerReady, controller.signal);
        } catch {
          if (!controller.signal.aborted) {
            await abortAttachmentGeneration(entry, transaction, "invariantFailed");
          }
          return;
        }
        if (!isCurrentAttachment(entry, owner)) {
          return;
        }

        const fetchStartedAt = performance.now();
        const activationPromise = TerminalOutputAPI.attachOutput(
          owner.sessionId,
          true,
          owner.documentEpoch,
          owner.generation,
        );
        void activationPromise.then(
          () => {
            if (!isCurrentAttachment(entry, owner)) {
              void cancelBackendActivation(owner, true);
            }
          },
          () => undefined,
        );
        let activation: PtyTerminalOutputActivation;
        try {
          activation = await awaitWithAbort(
            activationPromise,
            controller.signal,
          );
        } catch {
          if (!controller.signal.aborted) {
            await abortAttachmentGeneration(entry, transaction, "invariantFailed");
          }
          return;
        }
        if (!isCurrentAttachment(entry, owner)) {
          void cancelBackendActivation(owner, true);
          return;
        }
        transaction.fetchMicros = elapsedMicros(fetchStartedAt);
        attachedOwner = owner;
        await runAttachmentTransaction(entry, transaction, activation);
        if (currentTransaction === transaction) {
          currentTransaction = null;
        }
      })
      .catch(() => {
        if (attachChainFailureReported) {
          return;
        }
        attachChainFailureReported = true;
        const owner = target?.transaction.owner ?? desiredOwner;
        console.warn(
          owner === null
            ? "[terminal-snapshot] event=attach_chain outcome=rejected"
            : `[terminal-snapshot] event=attach_chain outcome=rejected ` +
                `sessionId=${owner.sessionId} documentEpoch=${owner.documentEpoch} ` +
                `attachGeneration=${owner.generation}`,
        );
      });
  };

  const selectSession = (sessionId: string): void => {
    const previousVisible = visibleSessionId;
    if (visibleSessionId !== null && visibleSessionId !== sessionId) {
      const oldEntry = registry.get(visibleSessionId);
      if (oldEntry) {
        oldEntry.container.hidden = true;
        if (
          currentTransaction === null ||
          currentTransaction.owner.sessionId !== oldEntry.sessionId
        ) {
          retireEntryGeneration(oldEntry);
        }
      }
    }

    visibleSessionId = sessionId;
    const entry = registry.activate(sessionId, createSessionTerminal);
    entry.container.hidden = false;
    registry.setVisible(sessionId);
    entry.terminal.focus();
    if (isBrowser) {
      entry.attachGeneration = null;
      scheduleViewportSync(sessionId, null);
      lastSelectedSessionId = sessionId;
      return;
    }
    const epoch = documentEpoch();
    if (epoch === null) {
      return;
    }
    const transitionKind: PtyTerminalAttachTransitionKind =
      terminalStore.selectionSource === "attach"
        ? "reattach"
        : lastSelectedSessionId === null
          ? "initial"
          : previousVisible === sessionId
            ? "reattach"
            : "switch";
    const transaction = beginAttachmentGeneration(
      sessionId,
      entry,
      epoch,
      transitionKind,
    );
    if (transaction === null) {
      return;
    }
    transitionAttachment({ entry, transaction });
    scheduleViewportSync(sessionId, transaction.owner.generation);
    lastSelectedSessionId = sessionId;
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
        const entry = registry.get(visibleSessionId);
        if (entry) {
          scheduleViewportSync(visibleSessionId, entry.attachGeneration);
        }
      }
    });
    resizeObserver.observe(hostRef);

    try {
      const unlisten = await onPtyOutput(handlePtyOutput);
      if (viewDisposed) {
        unlisten();
        return;
      }
      unlistenPtyOutput = unlisten;
      markListenerReady();
    } catch {
      // Not rethrown: the rest of this mount (session destroy, terminal
      // detach, the close hook) is still worth registering.
      console.warn("[terminal-snapshot] event=pty_output_listener outcome=unavailable");
      failListenerReady(new Error("ptyOutputListenerUnavailable"));
    }

    if (!isBrowser) {
      try {
        const epoch = await TerminalOutputAPI.documentEpoch();
        if (viewDisposed) return;
        setDocumentEpoch(epoch);
      } catch {
        console.warn(
          "[terminal-snapshot] event=terminal_attach_frontend stage=epoch outcome=snapshotDiscarded",
        );
      }
    }

    if (viewDisposed) return;
    const sessionDestroyedUnlisten = await onSessionDestroyed(({ id }) => {
      if (currentTransaction?.owner.sessionId === id) {
        const entry = registry.get(id);
        if (entry) {
          void abortAttachmentGeneration(entry, currentTransaction, "disposed");
        }
        currentTransaction = null;
      }
      registry.remove(id);
    });
    if (viewDisposed) {
      sessionDestroyedUnlisten();
      return;
    }
    unlistenSessionDestroyed = sessionDestroyedUnlisten;

    if (!props.lockedSessionId) {
      const terminalDetachedUnlisten = await onTerminalDetached(({ sessionId }) => {
        // The session moved to its own window: release it here rather than
        // creating a hidden terminal for it.
        if (currentTransaction?.owner.sessionId === sessionId) {
          const entry = registry.get(sessionId);
          if (entry) {
            void abortAttachmentGeneration(entry, currentTransaction, "stale");
          }
          currentTransaction = null;
        }
        if (sessionId === attachedOwner?.sessionId) {
          transitionAttachment(null);
        }
      });
      if (viewDisposed) {
        terminalDetachedUnlisten();
        return;
      }
      unlistenTerminalDetached = terminalDetachedUnlisten;
    }

    if (isTauri) {
      try {
        const { getCurrentWindow } = await import("@tauri-apps/api/window");
        if (viewDisposed) return;
        const closeUnlisten = await getCurrentWindow().onCloseRequested(() => {
          transitionAttachment(null);
        });
        if (viewDisposed) {
          closeUnlisten();
          return;
        }
        unlistenCloseRequested = closeUnlisten;
      } catch {
        console.warn(
          "[terminal-snapshot] event=close_detach_hook outcome=unavailable",
        );
      }
    }
  });

  createEffect(() => {
    const sessionId = terminalStore.activeSessionId;
    const epoch = documentEpoch();
    void terminalStore.selectionSource;
    if (!sessionId) {
      if (visibleSessionId) {
        const entry = registry.get(visibleSessionId);
        if (entry) {
          if (currentTransaction?.owner.sessionId === visibleSessionId) {
            void abortAttachmentGeneration(entry, currentTransaction, "stale");
            currentTransaction = null;
          } else {
            retireEntryGeneration(entry);
          }
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

    if (!isBrowser && epoch === null) {
      return;
    }

    selectSession(sessionId);
  });

  onCleanup(() => {
    viewDisposed = true;
    if (currentTransaction !== null) {
      const entry = registry.get(currentTransaction.owner.sessionId);
      if (entry) {
        void abortAttachmentGeneration(entry, currentTransaction, "disposed");
      }
      currentTransaction = null;
    }
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
