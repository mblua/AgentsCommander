// @vitest-environment jsdom
import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  MAIN_TERMINAL_LAYOUT_PULSE_REQUEST_EVENT,
  type MainTerminalLayoutPulseRequest,
  type MainTerminalLayoutPulseResult,
  type MainTerminalLayoutPulseSample,
} from "../shared/types";
import {
  installBrowserDomStubs,
  resetSidebarCompactForTests,
  setPulseSeamOverridesForTests,
} from "../shared/testing/ui-harness";
import { clampMainSidebarWidth } from "../shared/sidebar-layout";
import {
  RAIL_WIDTH_PX,
  createPulseWidthSeams,
  railNudgePx,
  restoreWidthPx,
  setSidebarCompactMode,
  sidebarCompact,
  toggleSidebarCompact,
} from "../shared/sidebar-compact";

// The seams App.sidebar-width.test.tsx mocks (its helpers are file-local and
// preserved, so this file reproduces the pattern under its own names).
const viewlessStub = vi.hoisted(() => ({ default: () => null }));

const appDoubles = vi.hoisted(() => ({
  loadSettings: vi.fn(),
  saveSettings: vi.fn(),
  bootZoom: vi.fn(),
  bootGeometry: vi.fn(),
  wireHomeEvents: vi.fn(),
  wireCentralEvents: vi.fn(),
  startWatchdogLoop: vi.fn(),
  chooseInitialView: vi.fn(),
}));

vi.mock("../shared/ipc", () => ({
  SettingsAPI: {
    get: appDoubles.loadSettings,
    update: appDoubles.saveSettings,
  },
}));
vi.mock("../shared/platform", () => ({ isTauri: false }));
vi.mock("../shared/zoom", () => ({ initZoom: appDoubles.bootZoom }));
vi.mock("../shared/window-geometry", () => ({
  initWindowGeometry: appDoubles.bootGeometry,
}));
vi.mock("../sidebar/watchdog/non-stop-watchdog-client", () => ({
  startNonStopWatchdogClient: appDoubles.startWatchdogLoop,
}));
vi.mock("./listeners-home", () => ({
  wireHomeListeners: appDoubles.wireHomeEvents,
}));
vi.mock("./listeners-central-view", () => ({
  wireCentralViewListeners: appDoubles.wireCentralEvents,
}));
vi.mock("./stores/centralView", () => ({
  centralViewStore: {
    isResourceMonitor: false,
    setInitialView: appDoubles.chooseInitialView,
  },
}));
vi.mock("../sidebar/components/Titlebar", () => viewlessStub);
vi.mock("../resource-monitor/App", () => viewlessStub);
vi.mock("./components/ErrorModal", () => viewlessStub);
vi.mock("../sidebar/App", () => viewlessStub);
vi.mock("./components/QuitConfirmModal", () => viewlessStub);
vi.mock("../terminal/App", () => viewlessStub);
vi.mock("../shared/components/ExternalLinkConfirm", () => viewlessStub);

import MainApp from "./App";

const COMPACT_PANE = "calc(var(--ac-rail-width) + 0px)";
const PULSE_LEG_MS = 2000;

type FrameQueue = {
  flushFrame: () => Promise<boolean>;
  outstanding: () => number;
  restore: () => void;
};

/** Manual frames: a real requestAnimationFrame never resolves under the fake
 *  timers some tests install, and a drained queue is not viewport quiescence. */
function installFrameQueue(): FrameQueue {
  const priorRequest = globalThis.requestAnimationFrame;
  const priorCancel = globalThis.cancelAnimationFrame;
  let nextHandle = 1;
  let clock = 0;
  const pending = new Map<number, FrameRequestCallback>();

  Object.defineProperty(globalThis, "requestAnimationFrame", {
    configurable: true,
    writable: true,
    value: (callback: FrameRequestCallback) => {
      const handle = nextHandle;
      nextHandle += 1;
      pending.set(handle, callback);
      return handle;
    },
  });
  Object.defineProperty(globalThis, "cancelAnimationFrame", {
    configurable: true,
    writable: true,
    value: (handle: number) => {
      pending.delete(handle);
    },
  });

  return {
    flushFrame: async () => {
      const due = [...pending.values()];
      pending.clear();
      const timestamp = clock;
      clock += 16;
      for (const callback of due) {
        callback(timestamp);
      }
      await settle();
      return due.length > 0;
    },
    outstanding: () => pending.size,
    restore: () => {
      pending.clear();
      Object.defineProperty(globalThis, "requestAnimationFrame", {
        configurable: true,
        writable: true,
        value: priorRequest,
      });
      Object.defineProperty(globalThis, "cancelAnimationFrame", {
        configurable: true,
        writable: true,
        value: priorCancel,
      });
    },
  };
}

async function settle(): Promise<void> {
  for (let pass = 0; pass < 8; pass += 1) {
    await Promise.resolve();
  }
}

function appSettings(mainSidebarWidth = 440) {
  return {
    themeLight: false,
    mainSidebarWidth,
    mainSidebarSide: "right",
    mainResourceMonitorAttached: false,
    mainAlwaysOnTop: false,
  };
}

function renderMainApp() {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const dispose = render(() => <MainApp />, host);
  return {
    host,
    unmount: () => {
      dispose();
      host.remove();
    },
  };
}

function paneWidth(host: HTMLElement): string {
  return (host.querySelector(".main-sidebar-pane") as HTMLElement).style.width;
}

let pulseSequence = 1;

function sendPulse(sampleNow: () => MainTerminalLayoutPulseSample | null) {
  const complete = vi.fn<(result: MainTerminalLayoutPulseResult) => void>();
  const request: MainTerminalLayoutPulseRequest = {
    requestId: pulseSequence,
    sessionId: "pulse-compact",
    attachGeneration: 1,
    accepted: false,
    sample: sampleNow,
    complete,
  };
  pulseSequence += 1;
  window.dispatchEvent(
    new CustomEvent<MainTerminalLayoutPulseRequest>(
      MAIN_TERMINAL_LAYOUT_PULSE_REQUEST_EVENT,
      { detail: request },
    ),
  );
  return { request, complete };
}

/** Geometry that follows the rail nudge, so a compact pulse's host and column
 *  counts really move with the pane and the acknowledgement chain is honest. */
function followPaneSampler(): () => MainTerminalLayoutPulseSample {
  let epoch = 0;
  return () => {
    const hostWidth = 1400 - RAIL_WIDTH_PX - railNudgePx();
    const geometry = { hostWidth, cols: Math.floor(hostWidth / 10), rows: 24 };
    epoch += 1;
    return {
      ...geometry,
      observedObserverEpoch: epoch,
      completedObserverAck: { epoch, first: geometry, second: geometry },
    };
  };
}

const HEALTHY_GEOMETRY = { hostWidth: 800, cols: 80, rows: 24 };

function healthySampler(): MainTerminalLayoutPulseSample {
  return {
    ...HEALTHY_GEOMETRY,
    observedObserverEpoch: 9,
    completedObserverAck: {
      epoch: 9,
      first: HEALTHY_GEOMETRY,
      second: HEALTHY_GEOMETRY,
    },
  };
}

describe("MainApp compact pulse (#2236 / #2279)", () => {
  let restoreDom: (() => void) | null = null;
  let frameQueue: FrameQueue;
  let consoleError: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    resetSidebarCompactForTests();
    restoreDom = installBrowserDomStubs();
    frameQueue = installFrameQueue();
    pulseSequence = 1;
    Object.defineProperty(window, "innerWidth", {
      configurable: true,
      writable: true,
      value: 1400,
    });
    appDoubles.loadSettings.mockReset().mockResolvedValue(appSettings());
    appDoubles.saveSettings.mockReset().mockResolvedValue(undefined);
    appDoubles.bootZoom.mockReset().mockResolvedValue(() => {});
    appDoubles.bootGeometry.mockReset().mockResolvedValue(() => {});
    appDoubles.wireHomeEvents.mockReset().mockResolvedValue([]);
    appDoubles.wireCentralEvents.mockReset().mockResolvedValue([]);
    appDoubles.startWatchdogLoop.mockReset();
    appDoubles.chooseInitialView.mockReset();
    consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
  });

  afterEach(() => {
    resetSidebarCompactForTests();
    frameQueue.restore();
    vi.useRealTimers();
    restoreDom?.();
    restoreDom = null;
    consoleError.mockRestore();
    document.body.innerHTML = "";
    document.documentElement.classList.remove("light-theme");
  });

  it("is dependency-backed with no override, and overrides only the installed member", () => {
    const writeExpanded = vi.fn<(px: number) => void>();
    const seams = createPulseWidthSeams({
      readExpanded: () => 520,
      writeExpanded,
      clampExpanded: (px) => clampMainSidebarWidth(px, 1400),
      paneStyleWidth: () => "520px",
    });
    expect(sidebarCompact()).toBe(false);

    // No override: every member delegates to its host accessor.
    expect(seams.readPulseWidth()).toBe(520);
    expect(seams.clampPulseWidth(100)).toBe(clampMainSidebarWidth(100, 1400));
    expect(seams.pulseWidthIsApplied(520)).toBe(true);
    seams.writePulseWidth(536);
    expect(writeExpanded).toHaveBeenCalledWith(536);

    // One installed member replaces exactly itself.
    setPulseSeamOverridesForTests({ clampPulseWidth: () => 123 });
    expect(seams.clampPulseWidth(520)).toBe(123);
    expect(seams.readPulseWidth()).toBe(520);
    expect(seams.pulseWidthIsApplied(520)).toBe(true);
    seams.writePulseWidth(540);
    expect(writeExpanded).toHaveBeenCalledTimes(2);
    expect(writeExpanded).toHaveBeenLastCalledWith(540);
  });

  it("is the identity while expanded, for widths inside and outside the clamp", () => {
    const written: number[] = [];
    const seams = createPulseWidthSeams({
      readExpanded: () => 520,
      writeExpanded: (px) => written.push(px),
      clampExpanded: (px) => clampMainSidebarWidth(px, 1400),
      paneStyleWidth: () => "520px",
    });
    expect(sidebarCompact()).toBe(false);

    expect(seams.readPulseWidth()).toBe(520);
    seams.writePulseWidth(536);
    expect(written).toEqual([536]);

    // 520 is inside the clamp; 100 and 3000 are outside it.
    expect(seams.clampPulseWidth(520)).toBe(520);
    expect(seams.clampPulseWidth(100)).toBe(400);
    expect(seams.clampPulseWidth(3000)).toBe(600);

    // The expanded branch is the same "px" string comparison as before.
    expect(seams.pulseWidthIsApplied(520)).toBe(true);
    expect(seams.pulseWidthIsApplied(536)).toBe(false);
  });

  it("completes a compact pulse instead of cancelling or skipping it", async () => {
    const app = renderMainApp();
    try {
      await settle();
      setSidebarCompactMode(true);
      expect(sidebarCompact()).toBe(true);
      expect(paneWidth(app.host)).toBe(COMPACT_PANE);

      const pulse = sendPulse(followPaneSampler());
      expect(railNudgePx()).toBe(-16);
      expect(paneWidth(app.host)).toBe("calc(var(--ac-rail-width) + -16px)");

      for (let pass = 0; pass < 40 && pulse.complete.mock.calls.length === 0; pass += 1) {
        await frameQueue.flushFrame();
      }

      expect(pulse.complete).toHaveBeenCalledTimes(1);
      expect(pulse.complete.mock.calls[0][0]).toMatchObject({
        status: "completed",
        reason: "completed",
        trace: {
          original: { sidebarWidth: RAIL_WIDTH_PX },
          expanded: { sidebarWidth: RAIL_WIDTH_PX - 16 },
          restored: { sidebarWidth: RAIL_WIDTH_PX },
        },
      });
      expect(railNudgePx()).toBe(0);
      expect(paneWidth(app.host)).toBe(COMPACT_PANE);
      expect(frameQueue.outstanding()).toBe(0);
    } finally {
      app.unmount();
    }
  });

  it("negative control: a no-op write leaves the leg on 68 and cancels width_changed", async () => {
    const app = renderMainApp();
    try {
      await settle();
      setSidebarCompactMode(true);
      setPulseSeamOverridesForTests({ writePulseWidth: () => {} });

      const pulse = sendPulse(followPaneSampler());
      expect(paneWidth(app.host)).toBe(COMPACT_PANE);

      await frameQueue.flushFrame();
      expect(pulse.complete).toHaveBeenCalledTimes(1);
      expect(pulse.complete.mock.calls[0][0]).toMatchObject({
        status: "cancelled",
        reason: "width_changed",
      });
      expect(railNudgePx()).toBe(0);
    } finally {
      app.unmount();
    }
  });

  it("negative control: the expanded clamp reverts the leg to skipped/clamped", async () => {
    const app = renderMainApp();
    try {
      await settle();
      setSidebarCompactMode(true);
      setPulseSeamOverridesForTests({
        clampPulseWidth: (px) => clampMainSidebarWidth(px, window.innerWidth),
      });

      const pulse = sendPulse(followPaneSampler());
      expect(pulse.complete).toHaveBeenCalledTimes(1);
      expect(pulse.complete.mock.calls[0][0]).toMatchObject({
        status: "skipped",
        reason: "clamped",
      });
      expect(railNudgePx()).toBe(0);
      expect(paneWidth(app.host)).toBe(COMPACT_PANE);
    } finally {
      app.unmount();
    }
  });

  it.each([
    {
      label: "setSidebarCompactMode(true)",
      start: "expanded" as const,
      flip: () => setSidebarCompactMode(true),
    },
    {
      label: "setSidebarCompactMode(false)",
      start: "compact" as const,
      flip: () => setSidebarCompactMode(false),
    },
    {
      label: "toggleSidebarCompact() from expanded",
      start: "expanded" as const,
      flip: () => toggleSidebarCompact(),
    },
    {
      label: "toggleSidebarCompact() from compact",
      start: "compact" as const,
      flip: () => toggleSidebarCompact(),
    },
  ])(
    "cancels a mid-pulse mode flip through $label and leaves no nudge",
    async ({ start, flip }) => {
      appDoubles.loadSettings.mockResolvedValue({
        ...appSettings(),
        mainSidebarWidth: 520,
      });
      const app = renderMainApp();
      try {
        await settle();
        expect(paneWidth(app.host)).toBe("520px");

        if (start === "compact") {
          setSidebarCompactMode(true);
          expect(sidebarCompact()).toBe(true);
          expect(paneWidth(app.host)).toBe(COMPACT_PANE);
        }

        const pulse = sendPulse(healthySampler);
        if (start === "expanded") {
          expect(paneWidth(app.host)).toBe("504px");
        } else {
          expect(paneWidth(app.host)).toBe("calc(var(--ac-rail-width) + -16px)");
        }
        expect(pulse.complete).not.toHaveBeenCalled();

        flip();

        expect(pulse.complete).toHaveBeenCalledTimes(1);
        expect(pulse.complete.mock.calls[0][0]).toMatchObject({
          status: "cancelled",
          reason: "width_changed",
        });

        if (start === "expanded") {
          // The nudge must not survive into the outgoing expanded width: the
          // pre-nudge 520, not 504, and the rail nudge cleared at the flip.
          expect(sidebarCompact()).toBe(true);
          expect(railNudgePx()).toBe(0);
          expect(paneWidth(app.host)).toBe(COMPACT_PANE);
          expect(restoreWidthPx()).toBe(520);
        } else {
          // The compact nudge must not survive into the snapshot width.
          expect(sidebarCompact()).toBe(false);
          expect(railNudgePx()).toBe(0);
          expect(paneWidth(app.host)).toBe("520px");
        }
      } finally {
        app.unmount();
      }
    },
  );

  it("leaves no rail nudge when the compact pulse leg times out", async () => {
    frameQueue.restore();
    vi.useFakeTimers();
    frameQueue = installFrameQueue();
    appDoubles.loadSettings.mockResolvedValue({
      ...appSettings(),
      mainSidebarWidth: 520,
    });
    const app = renderMainApp();
    try {
      await settle();
      setSidebarCompactMode(true);
      expect(paneWidth(app.host)).toBe(COMPACT_PANE);

      const pulse = sendPulse(healthySampler);
      expect(railNudgePx()).toBe(-16);

      await vi.advanceTimersByTimeAsync(PULSE_LEG_MS);
      await settle();

      expect(pulse.complete).toHaveBeenCalledTimes(1);
      expect(pulse.complete.mock.calls[0][0]).toMatchObject({
        status: "failed",
        reason: "expanded_timeout",
      });
      expect(railNudgePx()).toBe(0);
      expect(paneWidth(app.host)).toBe(COMPACT_PANE);
    } finally {
      app.unmount();
    }
  });
});
