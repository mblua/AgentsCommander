import { Component, createSignal, onMount, onCleanup, Show } from "solid-js";
import type { UnlistenFn } from "../shared/transport";
import {
  MAIN_TERMINAL_LAYOUT_PULSE_REQUEST_EVENT,
  type MainSidebarSide,
  type MainTerminalLayoutGeometry,
  type MainTerminalLayoutObserverAck,
  type MainTerminalLayoutPulsePhaseTrace,
  type MainTerminalLayoutPulseReason,
  type MainTerminalLayoutPulseRequest,
  type MainTerminalLayoutPulseResult,
  type MainTerminalLayoutPulseSample,
  type MainTerminalLayoutPulseStatus,
  type MainTerminalLayoutPulseTrace,
} from "../shared/types";
import { SettingsAPI } from "../shared/ipc";
import { isTauri } from "../shared/platform";
import { initZoom } from "../shared/zoom";
import { initWindowGeometry } from "../shared/window-geometry";
import { startNonStopWatchdogClient } from "../sidebar/watchdog/non-stop-watchdog-client";
import SidebarApp from "../sidebar/App";
import TerminalApp from "../terminal/App";
import ResourceMonitorApp from "../resource-monitor/App";
import { centralViewStore } from "./stores/centralView";
import { wireCentralViewListeners } from "./listeners-central-view";
import Titlebar from "../sidebar/components/Titlebar";
import QuitConfirmModal from "./components/QuitConfirmModal";
import ErrorModal from "./components/ErrorModal";
import ExternalLinkConfirm from "../shared/components/ExternalLinkConfirm";
import { wireHomeListeners } from "./listeners-home";
import {
  DEFAULT_MAIN_SIDEBAR_WIDTH,
  MAIN_SIDEBAR_MAX_WIDTH,
  MAIN_SIDEBAR_MIN_WIDTH,
  MAIN_TERMINAL_MIN_WIDTH,
  clampMainSidebarWidth,
} from "../shared/sidebar-layout";
import "./styles/main.css";
import "../shared/styles/external-link-confirm.css";

const DEFAULT_SIDEBAR_SIDE: MainSidebarSide = "right";

const SIDEBAR_PULSE_DELTA_PX = 16;
const SIDEBAR_PULSE_DWELL_MS = 200;
const SIDEBAR_PULSE_LEG_TIMEOUT_MS = 2000;
const SIDEBAR_PULSE_REQUEST_TIMEOUT_MS = 8000;

type SidebarPulseWaitOutcome = "matched" | "timeout" | "cancelled";

type SidebarPulseWait = {
  frame: number | null;
  timer: ReturnType<typeof setTimeout> | null;
  resolve: (outcome: SidebarPulseWaitOutcome) => void;
};

type SidebarPulseOwner = {
  request: MainTerminalLayoutPulseRequest;
  trace: MainTerminalLayoutPulseTrace;
  settingsWritesAtAcceptance: number;
  requestWatchdog: ReturnType<typeof setTimeout> | null;
  wait: SidebarPulseWait | null;
  started: boolean;
  completed: boolean;
  originalWidth: number | null;
  nudgedWidth: number | null;
  ownsTemporaryWidth: boolean;
};

type LivePulseSample =
  | { kind: "sample"; sample: MainTerminalLayoutPulseSample }
  | {
      kind: "stop";
      status: MainTerminalLayoutPulseStatus;
      reason: MainTerminalLayoutPulseReason;
      invalidNumbers?: boolean;
    };

const emptyPulsePhase = (): MainTerminalLayoutPulsePhaseTrace => ({
  sidebarWidth: null,
  hostWidth: null,
  cols: null,
  rows: null,
  baselineObservedEpoch: null,
  completedObserverAck: null,
});

const isNonNegativeSafeInteger = (value: number): boolean =>
  Number.isSafeInteger(value) && value >= 0;

const cloneLayoutGeometry = (
  geometry: MainTerminalLayoutGeometry,
): MainTerminalLayoutGeometry | null => {
  if (
    !Number.isFinite(geometry.hostWidth) ||
    geometry.hostWidth < 0 ||
    !isNonNegativeSafeInteger(geometry.cols) ||
    !isNonNegativeSafeInteger(geometry.rows)
  ) {
    return null;
  }
  return {
    hostWidth: geometry.hostWidth,
    cols: geometry.cols,
    rows: geometry.rows,
  };
};

const cloneObserverAck = (
  ack: MainTerminalLayoutObserverAck | null,
): MainTerminalLayoutObserverAck | null | undefined => {
  if (ack === null) {
    return null;
  }
  if (!isNonNegativeSafeInteger(ack.epoch)) {
    return undefined;
  }
  const first = cloneLayoutGeometry(ack.first);
  const second = cloneLayoutGeometry(ack.second);
  if (!first || !second) {
    return undefined;
  }
  return { epoch: ack.epoch, first, second };
};

const clonePulseSample = (
  sample: MainTerminalLayoutPulseSample,
): MainTerminalLayoutPulseSample | null => {
  const geometry = cloneLayoutGeometry(sample);
  const completedObserverAck = cloneObserverAck(sample.completedObserverAck);
  if (
    !geometry ||
    !isNonNegativeSafeInteger(sample.observedObserverEpoch) ||
    completedObserverAck === undefined
  ) {
    return null;
  }
  return {
    ...geometry,
    observedObserverEpoch: sample.observedObserverEpoch,
    completedObserverAck,
  };
};

const sameLayoutGeometry = (
  left: MainTerminalLayoutGeometry,
  right: MainTerminalLayoutGeometry,
): boolean =>
  left.hostWidth === right.hostWidth &&
  left.cols === right.cols &&
  left.rows === right.rows;

const pulsePhase = (
  sidebarWidth: number,
  sample: MainTerminalLayoutPulseSample,
  baselineObservedEpoch: number | null,
  completedObserverAck: MainTerminalLayoutObserverAck | null,
): MainTerminalLayoutPulsePhaseTrace => ({
  sidebarWidth,
  hostWidth: sample.hostWidth,
  cols: sample.cols,
  rows: sample.rows,
  baselineObservedEpoch,
  completedObserverAck,
});

const MainApp: Component = () => {
  const [sidebarWidth, setSidebarWidth] = createSignal(DEFAULT_MAIN_SIDEBAR_WIDTH);
  const [sidebarSide, setSidebarSide] = createSignal<MainSidebarSide>(DEFAULT_SIDEBAR_SIDE);
  const [dragging, setDragging] = createSignal(false);
  const [quitModalCount, setQuitModalCount] = createSignal<number | null>(null);

  let sidebarPaneRef!: HTMLDivElement;
  const unlisteners: UnlistenFn[] = [];
  let cleanupZoom: (() => void) | null = null;
  let cleanupGeometry: (() => void) | null = null;
  let quitInProgress = false;
  let splitterSaveTimeout: ReturnType<typeof setTimeout> | null = null;
  let splitterPersistenceInFlightCount = 0;
  let splitterSettingsUpdateCount = 0;
  let sidebarInitializationSettled = false;
  let disposed = false;
  let pulseOwner: SidebarPulseOwner | null = null;

  const cancelOwnerWait = (owner: SidebarPulseOwner): void => {
    const wait = owner.wait;
    if (!wait) {
      return;
    }
    owner.wait = null;
    if (wait.frame !== null) {
      cancelAnimationFrame(wait.frame);
      wait.frame = null;
    }
    if (wait.timer !== null) {
      clearTimeout(wait.timer);
      wait.timer = null;
    }
    wait.resolve("cancelled");
  };

  const finishPulse = (
    owner: SidebarPulseOwner,
    requestedStatus: MainTerminalLayoutPulseStatus,
    requestedReason: MainTerminalLayoutPulseReason,
  ): void => {
    if (owner.completed) {
      return;
    }
    owner.completed = true;

    if (owner.requestWatchdog !== null) {
      clearTimeout(owner.requestWatchdog);
      owner.requestWatchdog = null;
    }
    cancelOwnerWait(owner);

    if (
      owner.ownsTemporaryWidth &&
      owner.originalWidth !== null &&
      owner.nudgedWidth !== null
    ) {
      if (sidebarWidth() === owner.nudgedWidth) {
        setSidebarWidth(owner.originalWidth);
      }
      owner.ownsTemporaryWidth = false;
    }

    if (pulseOwner === owner) {
      pulseOwner = null;
    }

    const rawSettingsWritesDelta =
      splitterSettingsUpdateCount - owner.settingsWritesAtAcceptance;
    const settingsWritesDelta =
      isNonNegativeSafeInteger(rawSettingsWritesDelta) ? rawSettingsWritesDelta : 0;
    const status =
      requestedStatus === "completed" && rawSettingsWritesDelta !== 0
        ? "failed"
        : requestedStatus;
    const reason =
      requestedStatus === "completed" && rawSettingsWritesDelta !== 0
        ? "exception"
        : requestedReason;

    owner.trace.status = status;
    owner.trace.reason = reason;
    owner.trace.settingsWritesDelta = settingsWritesDelta;
    if (
      !Number.isFinite(owner.trace.dwellMs) ||
      owner.trace.dwellMs < 0 ||
      owner.trace.dwellMs > SIDEBAR_PULSE_REQUEST_TIMEOUT_MS
    ) {
      owner.trace.dwellMs = 0;
      owner.trace.status = "failed";
      owner.trace.reason = "exception";
    }

    const result: MainTerminalLayoutPulseResult = {
      status: owner.trace.status,
      reason: owner.trace.reason,
      trace: owner.trace,
    };
    try {
      owner.request.complete(result);
    } catch (error) {
      console.warn("[terminal] main layout pulse completion callback failed:", error);
    }
  };

  const failPulseForInvalidNumbers = (owner: SidebarPulseOwner): void => {
    owner.trace.original = emptyPulsePhase();
    owner.trace.expanded = emptyPulsePhase();
    owner.trace.restored = emptyPulsePhase();
    owner.trace.dwellMs = 0;
    finishPulse(owner, "failed", "exception");
  };

  const createPulseOwner = (
    request: MainTerminalLayoutPulseRequest,
  ): SidebarPulseOwner => {
    const requestId = isNonNegativeSafeInteger(request.requestId) ? request.requestId : 0;
    const attachGeneration = isNonNegativeSafeInteger(request.attachGeneration)
      ? request.attachGeneration
      : 0;
    const owner: SidebarPulseOwner = {
      request,
      trace: {
        version: 1,
        requestId,
        sessionId: typeof request.sessionId === "string" ? request.sessionId : "",
        attachGeneration,
        status: "failed",
        reason: "exception",
        original: emptyPulsePhase(),
        expanded: emptyPulsePhase(),
        restored: emptyPulsePhase(),
        dwellMs: 0,
        settingsWritesDelta: 0,
      },
      settingsWritesAtAcceptance: splitterSettingsUpdateCount,
      requestWatchdog: null,
      wait: null,
      started: false,
      completed: false,
      originalWidth: null,
      nudgedWidth: null,
      ownsTemporaryWidth: false,
    };
    owner.requestWatchdog = setTimeout(() => {
      finishPulse(
        owner,
        "failed",
        owner.started ? "request_timeout" : "initialization_timeout",
      );
    }, SIDEBAR_PULSE_REQUEST_TIMEOUT_MS);
    return owner;
  };

  const readLivePulseSample = (
    owner: SidebarPulseOwner,
    expectedWidth: number,
  ): LivePulseSample => {
    if (owner.completed || pulseOwner !== owner || disposed) {
      return { kind: "stop", status: "cancelled", reason: "stale" };
    }
    if (dragging()) {
      return { kind: "stop", status: "cancelled", reason: "dragging" };
    }
    if (splitterSaveTimeout !== null || splitterPersistenceInFlightCount > 0) {
      return { kind: "stop", status: "cancelled", reason: "persistence_owned" };
    }
    if (sidebarWidth() !== expectedWidth) {
      return { kind: "stop", status: "cancelled", reason: "width_changed" };
    }

    let rawSample: MainTerminalLayoutPulseSample | null;
    try {
      rawSample = owner.request.sample();
    } catch {
      return {
        kind: "stop",
        status: "failed",
        reason: "exception",
        invalidNumbers: true,
      };
    }
    if (rawSample === null) {
      return { kind: "stop", status: "cancelled", reason: "stale" };
    }
    const sample = clonePulseSample(rawSample);
    if (!sample) {
      return {
        kind: "stop",
        status: "failed",
        reason: "exception",
        invalidNumbers: true,
      };
    }
    if (sample.hostWidth <= 0 || sample.cols < 1 || sample.rows < 1) {
      return { kind: "stop", status: "failed", reason: "invalid_sample" };
    }
    return { kind: "sample", sample };
  };

  const stopFromLiveSample = (
    owner: SidebarPulseOwner,
    live: Exclude<LivePulseSample, { kind: "sample" }>,
  ): void => {
    if (live.invalidNumbers) {
      failPulseForInvalidNumbers(owner);
      return;
    }
    finishPulse(owner, live.status, live.reason);
  };

  const waitForPulseLeg = (
    owner: SidebarPulseOwner,
    expectedWidth: number,
    matches: (sample: MainTerminalLayoutPulseSample) => boolean,
    onMatch: (sample: MainTerminalLayoutPulseSample) => void,
  ): Promise<SidebarPulseWaitOutcome> =>
    new Promise((resolve) => {
      let settled = false;
      let wait!: SidebarPulseWait;
      const complete = (outcome: SidebarPulseWaitOutcome): void => {
        if (settled) {
          return;
        }
        settled = true;
        if (wait.frame !== null) {
          cancelAnimationFrame(wait.frame);
          wait.frame = null;
        }
        if (wait.timer !== null) {
          clearTimeout(wait.timer);
          wait.timer = null;
        }
        if (owner.wait === wait) {
          owner.wait = null;
        }
        resolve(outcome);
      };
      const poll = (): void => {
        wait.frame = null;
        const live = readLivePulseSample(owner, expectedWidth);
        if (live.kind === "stop") {
          stopFromLiveSample(owner, live);
          return;
        }
        try {
          if (matches(live.sample)) {
            onMatch(live.sample);
            complete("matched");
            return;
          }
        } catch {
          finishPulse(owner, "failed", "exception");
          return;
        }
        wait.frame = requestAnimationFrame(poll);
      };

      wait = { frame: null, timer: null, resolve: complete };
      owner.wait = wait;
      wait.timer = setTimeout(() => complete("timeout"), SIDEBAR_PULSE_LEG_TIMEOUT_MS);
      wait.frame = requestAnimationFrame(poll);
    });

  const waitForPulseDwell = (
    owner: SidebarPulseOwner,
    expectedWidth: number,
    expectedGeometry: MainTerminalLayoutGeometry,
  ): Promise<SidebarPulseWaitOutcome> =>
    new Promise((resolve) => {
      let settled = false;
      let firstFrameTimestamp: number | null = null;
      let wait!: SidebarPulseWait;
      const complete = (outcome: SidebarPulseWaitOutcome): void => {
        if (settled) {
          return;
        }
        settled = true;
        if (wait.frame !== null) {
          cancelAnimationFrame(wait.frame);
          wait.frame = null;
        }
        if (owner.wait === wait) {
          owner.wait = null;
        }
        resolve(outcome);
      };
      const poll = (timestamp: number): void => {
        wait.frame = null;
        if (!Number.isFinite(timestamp) || timestamp < 0) {
          failPulseForInvalidNumbers(owner);
          return;
        }
        const live = readLivePulseSample(owner, expectedWidth);
        if (live.kind === "stop") {
          stopFromLiveSample(owner, live);
          return;
        }
        if (!sameLayoutGeometry(live.sample, expectedGeometry)) {
          finishPulse(owner, "cancelled", "width_changed");
          return;
        }
        if (firstFrameTimestamp === null) {
          firstFrameTimestamp = timestamp;
        }
        const elapsed = Math.max(0, timestamp - firstFrameTimestamp);
        owner.trace.dwellMs = Math.min(SIDEBAR_PULSE_REQUEST_TIMEOUT_MS, elapsed);
        if (elapsed >= SIDEBAR_PULSE_DWELL_MS) {
          complete("matched");
          return;
        }
        wait.frame = requestAnimationFrame(poll);
      };

      wait = { frame: null, timer: null, resolve: complete };
      owner.wait = wait;
      wait.frame = requestAnimationFrame(poll);
    });

  const runPulse = async (owner: SidebarPulseOwner): Promise<void> => {
    if (owner.completed || pulseOwner !== owner || disposed) {
      return;
    }
    owner.started = true;

    try {
      const originalWidth = sidebarWidth();
      if (!Number.isFinite(originalWidth) || originalWidth < 0) {
        failPulseForInvalidNumbers(owner);
        return;
      }
      const originalLive = readLivePulseSample(owner, originalWidth);
      if (originalLive.kind === "stop") {
        stopFromLiveSample(owner, originalLive);
        return;
      }

      owner.originalWidth = originalWidth;
      owner.trace.original = pulsePhase(
        originalWidth,
        originalLive.sample,
        null,
        originalLive.sample.completedObserverAck,
      );

      const nudgedWidth = clampMainSidebarWidth(
        originalWidth - SIDEBAR_PULSE_DELTA_PX,
        window.innerWidth,
      );
      if (nudgedWidth !== originalWidth - SIDEBAR_PULSE_DELTA_PX) {
        finishPulse(owner, "skipped", "clamped");
        return;
      }
      owner.nudgedWidth = nudgedWidth;

      const expansionBoundary = readLivePulseSample(owner, originalWidth);
      if (expansionBoundary.kind === "stop") {
        stopFromLiveSample(owner, expansionBoundary);
        return;
      }
      const originalGeometry: MainTerminalLayoutGeometry = {
        hostWidth: expansionBoundary.sample.hostWidth,
        cols: expansionBoundary.sample.cols,
        rows: expansionBoundary.sample.rows,
      };
      owner.trace.original = pulsePhase(
        originalWidth,
        expansionBoundary.sample,
        null,
        expansionBoundary.sample.completedObserverAck,
      );
      const expansionBaselineObservedEpoch =
        expansionBoundary.sample.observedObserverEpoch;

      setSidebarWidth(nudgedWidth);
      owner.ownsTemporaryWidth = true;

      const expandedOutcome = await waitForPulseLeg(
        owner,
        nudgedWidth,
        (sample) => {
          const ack = sample.completedObserverAck;
          return Boolean(
            ack &&
              ack.epoch > expansionBaselineObservedEpoch &&
              sameLayoutGeometry(ack.first, sample) &&
              sameLayoutGeometry(ack.second, sample) &&
              sample.hostWidth > originalGeometry.hostWidth &&
              sample.cols > originalGeometry.cols &&
              sample.rows === originalGeometry.rows,
          );
        },
        (sample) => {
          owner.trace.expanded = pulsePhase(
            nudgedWidth,
            sample,
            expansionBaselineObservedEpoch,
            sample.completedObserverAck,
          );
        },
      );
      if (owner.completed || pulseOwner !== owner) {
        return;
      }
      if (expandedOutcome === "timeout") {
        finishPulse(owner, "failed", "expanded_timeout");
        return;
      }
      if (expandedOutcome !== "matched") {
        return;
      }

      const expandedGeometry: MainTerminalLayoutGeometry = {
        hostWidth: owner.trace.expanded.hostWidth!,
        cols: owner.trace.expanded.cols!,
        rows: owner.trace.expanded.rows!,
      };
      const dwellOutcome = await waitForPulseDwell(
        owner,
        nudgedWidth,
        expandedGeometry,
      );
      if (
        dwellOutcome !== "matched" ||
        owner.completed ||
        pulseOwner !== owner
      ) {
        return;
      }

      const restoreBoundary = readLivePulseSample(owner, nudgedWidth);
      if (restoreBoundary.kind === "stop") {
        stopFromLiveSample(owner, restoreBoundary);
        return;
      }
      if (!sameLayoutGeometry(restoreBoundary.sample, expandedGeometry)) {
        finishPulse(owner, "cancelled", "width_changed");
        return;
      }
      const restoreBaselineObservedEpoch = restoreBoundary.sample.observedObserverEpoch;

      setSidebarWidth(originalWidth);
      owner.ownsTemporaryWidth = false;

      const restoredOutcome = await waitForPulseLeg(
        owner,
        originalWidth,
        (sample) => {
          const ack = sample.completedObserverAck;
          return Boolean(
            ack &&
              ack.epoch > restoreBaselineObservedEpoch &&
              sameLayoutGeometry(ack.first, sample) &&
              sameLayoutGeometry(ack.second, sample) &&
              sameLayoutGeometry(sample, originalGeometry),
          );
        },
        (sample) => {
          owner.trace.restored = pulsePhase(
            originalWidth,
            sample,
            restoreBaselineObservedEpoch,
            sample.completedObserverAck,
          );
        },
      );
      if (owner.completed || pulseOwner !== owner) {
        return;
      }
      if (restoredOutcome === "timeout") {
        finishPulse(owner, "failed", "restore_timeout");
        return;
      }
      if (restoredOutcome !== "matched") {
        return;
      }

      const finalLive = readLivePulseSample(owner, originalWidth);
      if (finalLive.kind === "stop") {
        stopFromLiveSample(owner, finalLive);
        return;
      }
      if (
        !sameLayoutGeometry(finalLive.sample, originalGeometry) ||
        sidebarPaneRef.style.width !== `${originalWidth}px`
      ) {
        finishPulse(owner, "cancelled", "width_changed");
        return;
      }
      if (splitterSettingsUpdateCount !== owner.settingsWritesAtAcceptance) {
        finishPulse(owner, "failed", "exception");
        return;
      }

      finishPulse(owner, "completed", "completed");
    } catch {
      finishPulse(owner, "failed", "exception");
    }
  };

  const onMainTerminalLayoutPulseRequest = (event: Event): void => {
    const request = (
      event as CustomEvent<MainTerminalLayoutPulseRequest>
    ).detail;
    if (!request || request.accepted) {
      return;
    }

    const owner = createPulseOwner(request);
    request.accepted = true;
    if (
      !isNonNegativeSafeInteger(request.requestId) ||
      !isNonNegativeSafeInteger(request.attachGeneration) ||
      typeof request.sessionId !== "string"
    ) {
      failPulseForInvalidNumbers(owner);
      return;
    }

    if (pulseOwner && !pulseOwner.completed) {
      let previousIsStale = false;
      try {
        previousIsStale = pulseOwner.request.sample() === null;
      } catch {
        finishPulse(pulseOwner, "failed", "exception");
      }
      if (previousIsStale) {
        finishPulse(pulseOwner, "cancelled", "stale");
      } else {
        finishPulse(owner, "skipped", "busy");
        return;
      }
    }

    if (dragging()) {
      finishPulse(owner, "skipped", "dragging");
      return;
    }
    if (splitterSaveTimeout !== null || splitterPersistenceInFlightCount > 0) {
      finishPulse(owner, "skipped", "persistence_owned");
      return;
    }

    pulseOwner = owner;
    if (sidebarInitializationSettled) {
      void runPulse(owner);
    }
  };

  window.addEventListener(
    MAIN_TERMINAL_LAYOUT_PULSE_REQUEST_EVENT,
    onMainTerminalLayoutPulseRequest,
  );

  const persistWidth = (w: number) => {
    if (splitterSaveTimeout !== null) {
      clearTimeout(splitterSaveTimeout);
    }
    const ownedTimeout = setTimeout(async () => {
      if (splitterSaveTimeout === ownedTimeout) {
        splitterSaveTimeout = null;
      }
      splitterPersistenceInFlightCount += 1;
      try {
        const settings = await SettingsAPI.get();
        splitterSettingsUpdateCount += 1;
        await SettingsAPI.update({ ...settings, mainSidebarWidth: w });
      } catch (e) {
        console.error("Failed to persist splitter width:", e);
      } finally {
        splitterPersistenceInFlightCount = Math.max(
          0,
          splitterPersistenceInFlightCount - 1,
        );
      }
    }, 500);
    splitterSaveTimeout = ownedTimeout;
  };

  const cancelPulseForMutation = (
    reason: "dragging" | "width_changed",
  ): void => {
    const owner = pulseOwner;
    if (!owner || owner.completed) {
      return;
    }
    if (
      owner.ownsTemporaryWidth &&
      owner.nudgedWidth !== null &&
      sidebarWidth() !== owner.nudgedWidth
    ) {
      finishPulse(owner, "cancelled", "width_changed");
      return;
    }
    finishPulse(owner, "cancelled", reason);
  };

  const onPointerDown = (e: PointerEvent) => {
    cancelPulseForMutation("dragging");
    e.preventDefault();
    const divider = e.currentTarget as HTMLElement;
    const sideAtDragStart = sidebarSide();
    try { divider.setPointerCapture(e.pointerId); } catch { /* some targets refuse capture */ }
    document.body.style.cursor = "col-resize";
    setDragging(true);

    const onMove = (m: PointerEvent) => {
      const rawWidth = sideAtDragStart === "left"
        ? m.clientX
        : window.innerWidth - m.clientX;
      setSidebarWidth(clampMainSidebarWidth(rawWidth, window.innerWidth));
    };
    const onUp = (u: PointerEvent) => {
      try { divider.releasePointerCapture(u.pointerId); } catch { /* already released */ }
      document.body.style.cursor = "";
      setDragging(false);
      divider.removeEventListener("pointermove", onMove);
      divider.removeEventListener("pointerup", onUp);
      divider.removeEventListener("pointercancel", onUp);
      persistWidth(sidebarWidth());
    };
    divider.addEventListener("pointermove", onMove);
    divider.addEventListener("pointerup", onUp);
    divider.addEventListener("pointercancel", onUp);
  };

  const onDividerKeyDown = (e: KeyboardEvent) => {
    if (
      e.key !== "ArrowLeft" &&
      e.key !== "ArrowRight" &&
      e.key !== "Home" &&
      e.key !== "End"
    ) {
      return;
    }
    e.preventDefault();
    cancelPulseForMutation("width_changed");
    const step = e.shiftKey ? 40 : 10;
    let next: number | null = null;
    if (e.key === "ArrowLeft") next = sidebarWidth() + (sidebarSide() === "right" ? step : -step);
    else if (e.key === "ArrowRight") next = sidebarWidth() + (sidebarSide() === "right" ? -step : step);
    else if (e.key === "Home") next = MAIN_SIDEBAR_MIN_WIDTH;
    else if (e.key === "End") next = Math.min(MAIN_SIDEBAR_MAX_WIDTH, window.innerWidth - MAIN_TERMINAL_MIN_WIDTH);
    if (next === null) return;
    const clamped = clampMainSidebarWidth(next, window.innerWidth);
    setSidebarWidth(clamped);
    persistWidth(clamped);
  };

  async function countDetachedWindows(): Promise<number> {
    if (!isTauri) return 0;
    const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
    const all = await WebviewWindow.getAll();
    return all.filter((w) => w.label.startsWith("terminal-")).length;
  }

  const onModalCancel = () => setQuitModalCount(null);

  const onModalQuit = async () => {
    quitInProgress = true;
    try {
      const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      for (const w of await WebviewWindow.getAll()) {
        if (w.label.startsWith("terminal-")) {
          try { await w.destroy(); }
          catch (err) { console.warn("[quit] destroy of", w.label, "failed:", err); }
        }
      }
      try { await getCurrentWindow().destroy(); }
      catch (err) { console.warn("[quit] destroy of main failed:", err); }
    } finally {
      quitInProgress = false;
      setQuitModalCount(null);
    }
  };

  const onWindowResize = () => {
    cancelPulseForMutation("width_changed");
    setSidebarWidth((w) => clampMainSidebarWidth(w, window.innerWidth));
  };

  const onSidebarWidthChange = (event: Event) => {
    const width = (event as CustomEvent<{ width?: number }>).detail?.width;
    if (typeof width === "number") {
      cancelPulseForMutation("width_changed");
      setSidebarWidth(clampMainSidebarWidth(width, window.innerWidth));
    }
  };

  const onSidebarSideChange = (event: Event) => {
    const side = (event as CustomEvent<{ side?: MainSidebarSide }>).detail?.side;
    if (side === "left" || side === "right") {
      cancelPulseForMutation("width_changed");
      setSidebarSide(side);
    }
  };

  startNonStopWatchdogClient();

  onMount(async () => {
    try {
      cleanupZoom = await initZoom("main");
      cleanupGeometry = await initWindowGeometry("main");

      try {
        const settings = await SettingsAPI.get();
        document.documentElement.classList.toggle("light-theme", settings.themeLight);
        const saved = settings.mainSidebarWidth ?? DEFAULT_MAIN_SIDEBAR_WIDTH;
        setSidebarWidth(clampMainSidebarWidth(saved, window.innerWidth));
        setSidebarSide(settings.mainSidebarSide === "left" ? "left" : DEFAULT_SIDEBAR_SIDE);
        centralViewStore.setInitialView(
          settings.mainResourceMonitorAttached ? "resourceMonitor" : "terminal"
        );
        if (isTauri && settings.mainAlwaysOnTop) {
          const { getCurrentWindow } = await import("@tauri-apps/api/window");
          await getCurrentWindow().setAlwaysOnTop(true);
        }
      } catch (e) {
        console.error("Failed to load main-window settings:", e);
      }
    } catch (e) {
      console.error("Failed to initialize main window:", e);
    } finally {
      sidebarInitializationSettled = true;
      const pendingOwner = pulseOwner;
      if (pendingOwner && !pendingOwner.started && !pendingOwner.completed && !disposed) {
        void runPulse(pendingOwner);
      }
    }

    unlisteners.push(...(await wireHomeListeners()));

    unlisteners.push(...(await wireCentralViewListeners()));

    window.addEventListener("resize", onWindowResize);
    window.addEventListener("main-sidebar-width-change", onSidebarWidthChange);
    window.addEventListener("main-sidebar-side-change", onSidebarSideChange);

    if (isTauri) {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      const win = getCurrentWindow();
      const unlistenClose = await win.onCloseRequested(async (e) => {
        if (quitInProgress || quitModalCount() !== null) {
          e.preventDefault();
          return;
        }
        const count = await countDetachedWindows();
        if (count === 0) return; // silent quit path
        e.preventDefault();
        setQuitModalCount(count);
      });
      unlisteners.push(unlistenClose);
    }
  });

  onCleanup(() => {
    window.removeEventListener(
      MAIN_TERMINAL_LAYOUT_PULSE_REQUEST_EVENT,
      onMainTerminalLayoutPulseRequest,
    );
    disposed = true;
    if (pulseOwner && !pulseOwner.completed) {
      finishPulse(pulseOwner, "cancelled", "teardown");
    }
    unlisteners.forEach((u) => u());
    if (cleanupZoom) cleanupZoom();
    if (cleanupGeometry) cleanupGeometry();
    if (splitterSaveTimeout !== null) {
      clearTimeout(splitterSaveTimeout);
      splitterSaveTimeout = null;
    }
    window.removeEventListener("resize", onWindowResize);
    window.removeEventListener("main-sidebar-width-change", onSidebarWidthChange);
    window.removeEventListener("main-sidebar-side-change", onSidebarSideChange);
  });

  return (
    <div
      class="main-root"
      classList={{
        "main-dragging": dragging(),
        "main-sidebar-right": sidebarSide() === "right",
      }}
      data-ac-testid="main.root"
      data-ac-role="surface"
      data-ac-state={dragging() ? "dragging" : "idle"}
    >
      <Titlebar />
      <div class="main-body">
        <div
          class="main-sidebar-pane"
          ref={sidebarPaneRef!}
          style={{ width: `${sidebarWidth()}px` }}
        >
          <SidebarApp embedded railSide={sidebarSide()} />
        </div>
        <div
          class="main-divider"
          classList={{ dragging: dragging() }}
          onPointerDown={onPointerDown}
          onKeyDown={onDividerKeyDown}
          role="separator"
          aria-orientation="vertical"
          aria-label={`Resize ${sidebarSide()} sidebar`}
          aria-valuenow={Math.round(sidebarWidth())}
          aria-valuetext={`${Math.round(sidebarWidth())} pixels, sidebar on ${sidebarSide()}`}
          aria-valuemin={MAIN_SIDEBAR_MIN_WIDTH}
          aria-valuemax={MAIN_SIDEBAR_MAX_WIDTH}
          tabindex="0"
          data-ac-testid="main.splitter"
          data-ac-role="separator"
          data-ac-state={dragging() ? "dragging" : "idle"}
        />
        <div class="main-terminal-pane">
          <TerminalApp embedded />
          <Show when={centralViewStore.isResourceMonitor}>
            <div
              class="main-rm-pane"
              data-ac-testid="main.resourceMonitorPane"
              data-ac-role="surface"
            >
              <ResourceMonitorApp embedded />
            </div>
          </Show>
        </div>
      </div>
      <Show when={quitModalCount() !== null}>
        <QuitConfirmModal
          detachedCount={quitModalCount()!}
          onCancel={onModalCancel}
          onQuit={onModalQuit}
        />
      </Show>
      <ErrorModal />
      <ExternalLinkConfirm />
    </div>
  );
};

export default MainApp;
