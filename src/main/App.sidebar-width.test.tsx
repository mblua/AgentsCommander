// @vitest-environment jsdom
import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  MAIN_TERMINAL_LAYOUT_PULSE_REQUEST_EVENT,
  type MainTerminalLayoutObserverAck,
  type MainTerminalLayoutPulseRequest,
  type MainTerminalLayoutPulseResult,
  type MainTerminalLayoutPulseSample,
} from "../shared/types";
import { installBrowserDomStubs } from "../shared/testing/ui-harness";

const dependencies = vi.hoisted(() => ({
  settingsGet: vi.fn(),
  settingsUpdate: vi.fn(),
  initZoom: vi.fn(),
  initWindowGeometry: vi.fn(),
  wireHomeListeners: vi.fn(),
  wireCentralViewListeners: vi.fn(),
  startWatchdog: vi.fn(),
  setInitialView: vi.fn(),
}));

const signalControl = vi.hoisted(() => ({
  setSidebarWidth: null as ((width: number) => void) | null,
}));

vi.mock("solid-js", async () => {
  const actual = await vi.importActual<typeof import("solid-js")>("solid-js");
  return {
    ...actual,
    createSignal: <T,>(value: T) => {
      const signal = actual.createSignal(value);
      if (typeof value === "number" && value === 440) {
        signalControl.setSidebarWidth = (width: number) => {
          signal[1](() => width as T);
        };
      }
      return signal;
    },
  };
});

vi.mock("../shared/ipc", () => ({
  SettingsAPI: {
    get: dependencies.settingsGet,
    update: dependencies.settingsUpdate,
  },
}));
vi.mock("../shared/platform", () => ({ isTauri: false }));
vi.mock("../shared/zoom", () => ({ initZoom: dependencies.initZoom }));
vi.mock("../shared/window-geometry", () => ({
  initWindowGeometry: dependencies.initWindowGeometry,
}));
vi.mock("../sidebar/watchdog/non-stop-watchdog-client", () => ({
  startNonStopWatchdogClient: dependencies.startWatchdog,
}));
vi.mock("./listeners-home", () => ({
  wireHomeListeners: dependencies.wireHomeListeners,
}));
vi.mock("./listeners-central-view", () => ({
  wireCentralViewListeners: dependencies.wireCentralViewListeners,
}));
vi.mock("./stores/centralView", () => ({
  centralViewStore: {
    isResourceMonitor: false,
    setInitialView: dependencies.setInitialView,
  },
}));
vi.mock("../sidebar/App", () => ({ default: () => null }));
vi.mock("../terminal/App", () => ({ default: () => null }));
vi.mock("../resource-monitor/App", () => ({ default: () => null }));
vi.mock("../sidebar/components/Titlebar", () => ({ default: () => null }));
vi.mock("./components/QuitConfirmModal", () => ({ default: () => null }));
vi.mock("./components/ErrorModal", () => ({ default: () => null }));
vi.mock("../shared/components/ExternalLinkConfirm", () => ({ default: () => null }));

import MainApp from "./App";

type ManualFrames = {
  flushFrame: () => Promise<boolean>;
  pending: () => number;
  restore: () => void;
};

function installManualFrames(): ManualFrames {
  const previousRequest = globalThis.requestAnimationFrame;
  const previousCancel = globalThis.cancelAnimationFrame;
  let nextHandle = 1;
  let timestamp = 0;
  const queued = new Map<number, FrameRequestCallback>();

  Object.defineProperty(globalThis, "requestAnimationFrame", {
    configurable: true,
    writable: true,
    value: (callback: FrameRequestCallback) => {
      const handle = nextHandle;
      nextHandle += 1;
      queued.set(handle, callback);
      return handle;
    },
  });
  Object.defineProperty(globalThis, "cancelAnimationFrame", {
    configurable: true,
    writable: true,
    value: (handle: number) => queued.delete(handle),
  });

  return {
    flushFrame: async () => {
      const callbacks = [...queued.values()];
      queued.clear();
      const frameTimestamp = timestamp;
      timestamp += 16;
      for (const callback of callbacks) {
        callback(frameTimestamp);
      }
      await flushPromises();
      return callbacks.length > 0;
    },
    pending: () => queued.size,
    restore: () => {
      queued.clear();
      Object.defineProperty(globalThis, "requestAnimationFrame", {
        configurable: true,
        writable: true,
        value: previousRequest,
      });
      Object.defineProperty(globalThis, "cancelAnimationFrame", {
        configurable: true,
        writable: true,
        value: previousCancel,
      });
    },
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((next, fail) => {
    resolve = next;
    reject = fail;
  });
  return { promise, resolve, reject };
}

async function flushPromises(): Promise<void> {
  for (let pass = 0; pass < 8; pass += 1) {
    await Promise.resolve();
  }
}

function settings() {
  return {
    themeLight: false,
    mainSidebarWidth: 440,
    mainSidebarSide: "right",
    mainResourceMonitorAttached: false,
    mainAlwaysOnTop: false,
  };
}

function geometry(hostWidth: number, cols: number, rows = 24) {
  return { hostWidth, cols, rows };
}

function ack(
  epoch: number,
  hostWidth: number,
  cols: number,
  rows = 24,
): MainTerminalLayoutObserverAck {
  return {
    epoch,
    first: geometry(hostWidth, cols, rows),
    second: geometry(hostWidth, cols, rows),
  };
}

function sample(
  hostWidth: number,
  cols: number,
  observedObserverEpoch: number,
  completedObserverAck: MainTerminalLayoutObserverAck | null,
  rows = 24,
): MainTerminalLayoutPulseSample {
  return {
    hostWidth,
    cols,
    rows,
    observedObserverEpoch,
    completedObserverAck,
  };
}

let nextRequestId = 1;

function dispatchPulse(
  sampleNow: () => MainTerminalLayoutPulseSample | null,
  sessionId = "session-a",
  attachGeneration = 1,
) {
  const complete = vi.fn<(result: MainTerminalLayoutPulseResult) => void>();
  const request: MainTerminalLayoutPulseRequest = {
    requestId: nextRequestId,
    sessionId,
    attachGeneration,
    accepted: false,
    sample: sampleNow,
    complete,
  };
  nextRequestId += 1;
  window.dispatchEvent(
    new CustomEvent<MainTerminalLayoutPulseRequest>(
      MAIN_TERMINAL_LAYOUT_PULSE_REQUEST_EVENT,
      { detail: request },
    ),
  );
  return { request, complete };
}

function renderMain() {
  const root = document.createElement("div");
  document.body.append(root);
  const dispose = render(() => <MainApp />, root);
  return {
    root,
    cleanup: () => {
      dispose();
      root.remove();
    },
  };
}

function sidebarWidth(root: HTMLElement): string {
  return (root.querySelector(".main-sidebar-pane") as HTMLElement).style.width;
}

describe("MainApp sidebar layout pulse (#1532)", () => {
  let cleanupDom: (() => void) | null = null;
  let frames: ManualFrames;
  let error: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    frames = installManualFrames();
    nextRequestId = 1;
    signalControl.setSidebarWidth = null;
    Object.defineProperty(window, "innerWidth", {
      configurable: true,
      writable: true,
      value: 1400,
    });
    dependencies.settingsGet.mockReset().mockResolvedValue(settings());
    dependencies.settingsUpdate.mockReset().mockResolvedValue(undefined);
    dependencies.initZoom.mockReset().mockResolvedValue(() => {});
    dependencies.initWindowGeometry.mockReset().mockResolvedValue(() => {});
    dependencies.wireHomeListeners.mockReset().mockResolvedValue([]);
    dependencies.wireCentralViewListeners.mockReset().mockResolvedValue([]);
    dependencies.startWatchdog.mockReset();
    dependencies.setInitialView.mockReset();
    error = vi.spyOn(console, "error").mockImplementation(() => {});
  });

  afterEach(() => {
    frames.restore();
    vi.useRealTimers();
    cleanupDom?.();
    cleanupDom = null;
    error.mockRestore();
    document.body.innerHTML = "";
    document.documentElement.classList.remove("light-theme");
  });

  it("requires post-boundary two-fit acknowledgements, dwells at least 200ms, and restores exactly", async () => {
    const rendered = renderMain();
    try {
      await flushPromises();
      let current = sample(800, 80, 2, ack(2, 800, 80));
      const pulse = dispatchPulse(() => current);

      expect(pulse.request.accepted).toBe(true);
      expect(sidebarWidth(rendered.root)).toBe("424px");
      expect(pulse.complete).not.toHaveBeenCalled();

      // A pre-boundary or mixed record cannot qualify the expanded leg.
      current = {
        ...sample(816, 82, 3, null),
        completedObserverAck: {
          epoch: 2,
          first: geometry(800, 80),
          second: geometry(816, 82),
        },
      };
      await frames.flushFrame();
      expect(pulse.complete).not.toHaveBeenCalled();

      current = sample(816, 82, 3, ack(3, 816, 82));
      await frames.flushFrame();
      for (let frame = 0; frame < 14; frame += 1) {
        await frames.flushFrame();
      }
      expect(sidebarWidth(rendered.root)).toBe("440px");

      // The expansion acknowledgement cannot acknowledge restoration.
      await frames.flushFrame();
      expect(pulse.complete).not.toHaveBeenCalled();
      current = sample(800, 80, 4, ack(4, 800, 80));
      await frames.flushFrame();

      expect(pulse.complete).toHaveBeenCalledTimes(1);
      const result = pulse.complete.mock.calls[0][0];
      expect(result).toMatchObject({
        status: "completed",
        reason: "completed",
        trace: {
          version: 1,
          requestId: pulse.request.requestId,
          sessionId: "session-a",
          attachGeneration: 1,
          status: "completed",
          reason: "completed",
          original: { sidebarWidth: 440, hostWidth: 800, cols: 80, rows: 24 },
          expanded: {
            sidebarWidth: 424,
            hostWidth: 816,
            cols: 82,
            rows: 24,
            baselineObservedEpoch: 2,
            completedObserverAck: ack(3, 816, 82),
          },
          restored: {
            sidebarWidth: 440,
            hostWidth: 800,
            cols: 80,
            rows: 24,
            baselineObservedEpoch: 3,
            completedObserverAck: ack(4, 800, 80),
          },
          settingsWritesDelta: 0,
        },
      });
      expect(result.trace.dwellMs).toBeGreaterThanOrEqual(200);
      expect(result.trace.dwellMs).toBeLessThanOrEqual(8000);
      expect(dependencies.settingsUpdate).not.toHaveBeenCalled();
      expect(frames.pending()).toBe(0);
    } finally {
      rendered.cleanup();
    }
  });

  it.each([
    ["non-finite host width", sample(Number.NaN, 80, 1, ack(1, 800, 80))],
    ["negative host width", sample(-1, 80, 1, ack(1, 800, 80))],
    [
      "unsafe column count",
      sample(800, Number.MAX_SAFE_INTEGER + 1, 1, ack(1, 800, 80)),
    ],
    ["negative column count", sample(800, -1, 1, ack(1, 800, 80))],
    [
      "unsafe row count",
      sample(800, 80, 1, ack(1, 800, 80), Number.MAX_SAFE_INTEGER + 1),
    ],
    ["negative row count", sample(800, 80, 1, ack(1, 800, 80), -1)],
  ] as const)(
    "fails closed with bounded null phases for current %s geometry",
    async (_label, invalidSample) => {
      const rendered = renderMain();
      try {
        await flushPromises();
        const invalid = dispatchPulse(() => invalidSample);
        expect(invalid.complete.mock.calls[0][0]).toMatchObject({
          status: "failed",
          reason: "exception",
          trace: {
            original: {
              sidebarWidth: null,
              hostWidth: null,
              cols: null,
              rows: null,
              baselineObservedEpoch: null,
              completedObserverAck: null,
            },
            expanded: { sidebarWidth: null, hostWidth: null },
            restored: { sidebarWidth: null, hostWidth: null },
          },
        });
      } finally {
        rendered.cleanup();
      }
    },
  );

  it("skips an exact clamped width without mutation", async () => {
    const rendered = renderMain();
    try {
      await flushPromises();
      window.dispatchEvent(
        new CustomEvent("main-sidebar-width-change", { detail: { width: 400 } }),
      );
      const clamped = dispatchPulse(() => sample(800, 80, 2, ack(2, 800, 80)));
      expect(clamped.complete.mock.calls[0][0]).toMatchObject({
        status: "skipped",
        reason: "clamped",
      });
      expect(sidebarWidth(rendered.root)).toBe("400px");
    } finally {
      rendered.cleanup();
    }
  });

  it("does not acknowledge an expanded leg from a same-width event-only observer delivery", async () => {
    const rendered = renderMain();
    try {
      await flushPromises();
      let current = sample(800, 80, 1, ack(1, 800, 80));
      const pulse = dispatchPulse(() => current);
      expect(sidebarWidth(rendered.root)).toBe("424px");

      current = sample(800, 80, 2, ack(2, 800, 80));
      await frames.flushFrame();

      expect(pulse.complete).not.toHaveBeenCalled();
      expect(sidebarWidth(rendered.root)).toBe("424px");
    } finally {
      rendered.cleanup();
    }
  });

  it.each([
    ["zoom success", "zoom", false],
    ["zoom rejection", "zoom", true],
    ["geometry rejection", "geometry", true],
    ["Settings rejection", "settings", true],
  ] as const)(
    "queues before initialization and drains once after %s",
    async (_label, stage, reject) => {
      const gate = deferred<unknown>();
      if (stage === "zoom") {
        dependencies.initZoom.mockReturnValueOnce(gate.promise);
      } else if (stage === "geometry") {
        dependencies.initWindowGeometry.mockReturnValueOnce(gate.promise);
      } else {
        dependencies.settingsGet.mockReturnValueOnce(gate.promise);
      }

      const rendered = renderMain();
      const current = sample(800, 80, 1, ack(1, 800, 80));
      const pulse = dispatchPulse(() => current);
      try {
        expect(pulse.request.accepted).toBe(true);
        expect(sidebarWidth(rendered.root)).toBe("440px");
        expect(pulse.complete).not.toHaveBeenCalled();

        if (reject) {
          gate.reject(new Error(`${stage} failed`));
        } else {
          gate.resolve(() => {});
        }
        await flushPromises();

        expect(sidebarWidth(rendered.root)).toBe("424px");
        expect(pulse.complete).not.toHaveBeenCalled();
      } finally {
        rendered.cleanup();
      }
      expect(pulse.complete).toHaveBeenCalledTimes(1);
      expect(pulse.complete.mock.calls[0][0]).toMatchObject({
        status: "cancelled",
        reason: "teardown",
      });
    },
  );

  it("bounds a never-settling initialization at exactly 8000ms and never completes twice", async () => {
    frames.restore();
    vi.useFakeTimers();
    frames = installManualFrames();
    const gate = deferred<() => void>();
    dependencies.initZoom.mockReturnValueOnce(gate.promise);
    const rendered = renderMain();
    const pulse = dispatchPulse(() => sample(800, 80, 1, ack(1, 800, 80)));
    try {
      await vi.advanceTimersByTimeAsync(7999);
      expect(pulse.complete).not.toHaveBeenCalled();
      expect(sidebarWidth(rendered.root)).toBe("440px");

      await vi.advanceTimersByTimeAsync(1);
      expect(pulse.complete).toHaveBeenCalledTimes(1);
      expect(pulse.complete.mock.calls[0][0]).toMatchObject({
        status: "failed",
        reason: "initialization_timeout",
      });
      expect(sidebarWidth(rendered.root)).toBe("440px");

      gate.resolve(() => {});
      await flushPromises();
      rendered.cleanup();
      expect(pulse.complete).toHaveBeenCalledTimes(1);
    } finally {
      if (rendered.root.isConnected) rendered.cleanup();
    }
  });

  it("uses the exact expanded leg timeout and clears its frame, watchdog, and ownership", async () => {
    frames.restore();
    vi.useFakeTimers();
    frames = installManualFrames();
    const rendered = renderMain();
    try {
      await flushPromises();
      const pulse = dispatchPulse(() => sample(800, 80, 1, ack(1, 800, 80)));
      expect(sidebarWidth(rendered.root)).toBe("424px");

      await vi.advanceTimersByTimeAsync(1999);
      expect(pulse.complete).not.toHaveBeenCalled();
      await vi.advanceTimersByTimeAsync(1);
      await flushPromises();

      expect(pulse.complete).toHaveBeenCalledTimes(1);
      expect(pulse.complete.mock.calls[0][0]).toMatchObject({
        status: "failed",
        reason: "expanded_timeout",
      });
      expect(sidebarWidth(rendered.root)).toBe("440px");
      expect(frames.pending()).toBe(0);
      expect(vi.getTimerCount()).toBe(0);
    } finally {
      rendered.cleanup();
    }
  });

  it("uses the request-wide timeout during dwell and restores only its owned temporary width", async () => {
    frames.restore();
    vi.useFakeTimers();
    frames = installManualFrames();
    const rendered = renderMain();
    try {
      await flushPromises();
      let current = sample(800, 80, 1, ack(1, 800, 80));
      const pulse = dispatchPulse(() => current);
      current = sample(816, 82, 2, ack(2, 816, 82));
      await frames.flushFrame();
      expect(sidebarWidth(rendered.root)).toBe("424px");

      await vi.advanceTimersByTimeAsync(8000);
      await flushPromises();
      expect(pulse.complete).toHaveBeenCalledTimes(1);
      expect(pulse.complete.mock.calls[0][0]).toMatchObject({
        status: "failed",
        reason: "request_timeout",
      });
      expect(sidebarWidth(rendered.root)).toBe("440px");
      expect(frames.pending()).toBe(0);
      expect(vi.getTimerCount()).toBe(0);
    } finally {
      rendered.cleanup();
    }
  });

  it("uses a fresh post-dwell restore baseline and the exact restore timeout", async () => {
    frames.restore();
    vi.useFakeTimers();
    frames = installManualFrames();
    const rendered = renderMain();
    try {
      await flushPromises();
      let current = sample(800, 80, 1, ack(1, 800, 80));
      const pulse = dispatchPulse(() => current);
      current = sample(816, 82, 2, ack(2, 816, 82));
      await frames.flushFrame();
      for (let frame = 0; frame < 14; frame += 1) {
        await frames.flushFrame();
      }
      expect(sidebarWidth(rendered.root)).toBe("440px");

      await vi.advanceTimersByTimeAsync(1999);
      expect(pulse.complete).not.toHaveBeenCalled();
      await vi.advanceTimersByTimeAsync(1);
      await flushPromises();
      expect(pulse.complete.mock.calls[0][0]).toMatchObject({
        status: "failed",
        reason: "restore_timeout",
      });
      expect(frames.pending()).toBe(0);
      expect(vi.getTimerCount()).toBe(0);
    } finally {
      rendered.cleanup();
    }
  });

  it("rejects scheduled and in-flight persistence, then becomes eligible after update rejection", async () => {
    frames.restore();
    vi.useFakeTimers();
    frames = installManualFrames();
    const rendered = renderMain();
    try {
      await flushPromises();
      const getGate = deferred<ReturnType<typeof settings>>();
      const updateGate = deferred<void>();
      dependencies.settingsGet.mockReturnValueOnce(getGate.promise);
      dependencies.settingsUpdate.mockReturnValueOnce(updateGate.promise);

      const splitter = rendered.root.querySelector(".main-divider") as HTMLElement;
      splitter.dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowLeft", bubbles: true, cancelable: true }),
      );
      expect(sidebarWidth(rendered.root)).toBe("450px");

      const scheduled = dispatchPulse(() => sample(800, 80, 1, ack(1, 800, 80)));
      expect(scheduled.complete.mock.calls[0][0]).toMatchObject({
        status: "skipped",
        reason: "persistence_owned",
      });

      await vi.advanceTimersByTimeAsync(500);
      const fetching = dispatchPulse(() => sample(800, 80, 2, ack(2, 800, 80)), "b", 2);
      expect(fetching.complete.mock.calls[0][0]).toMatchObject({
        reason: "persistence_owned",
      });

      getGate.resolve(settings());
      await flushPromises();
      expect(dependencies.settingsUpdate).toHaveBeenCalledTimes(1);
      const updating = dispatchPulse(() => sample(800, 80, 3, ack(3, 800, 80)), "c", 3);
      expect(updating.complete.mock.calls[0][0]).toMatchObject({
        reason: "persistence_owned",
      });

      updateGate.reject(new Error("update failed"));
      await flushPromises();
      const eligible = dispatchPulse(() => sample(800, 80, 4, ack(4, 800, 80)), "d", 4);
      expect(eligible.request.accepted).toBe(true);
      expect(eligible.complete).not.toHaveBeenCalled();
      expect(sidebarWidth(rendered.root)).toBe("434px");
      rendered.cleanup();
      expect(eligible.complete).toHaveBeenCalledTimes(1);
      expect(eligible.complete.mock.calls[0][0]).toMatchObject({
        status: "cancelled",
        reason: "teardown",
        trace: { settingsWritesDelta: 0 },
      });
    } finally {
      if (rendered.root.isConnected) rendered.cleanup();
    }
  });

  it("releases persistence ownership after a Settings get rejection", async () => {
    frames.restore();
    vi.useFakeTimers();
    frames = installManualFrames();
    const rendered = renderMain();
    try {
      await flushPromises();
      dependencies.settingsGet.mockRejectedValueOnce(new Error("get failed"));
      const splitter = rendered.root.querySelector(".main-divider") as HTMLElement;
      splitter.dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowLeft", bubbles: true, cancelable: true }),
      );
      await vi.advanceTimersByTimeAsync(500);
      await flushPromises();

      const eligible = dispatchPulse(() => sample(800, 80, 1, ack(1, 800, 80)));
      expect(eligible.request.accepted).toBe(true);
      expect(eligible.complete).not.toHaveBeenCalled();
      expect(sidebarWidth(rendered.root)).toBe("434px");
    } finally {
      rendered.cleanup();
    }
  });

  it("releases persistence ownership after a successful Settings update", async () => {
    frames.restore();
    vi.useFakeTimers();
    frames = installManualFrames();
    const updateGate = deferred<void>();
    dependencies.settingsUpdate.mockReturnValueOnce(updateGate.promise);
    const rendered = renderMain();
    try {
      await flushPromises();
      const splitter = rendered.root.querySelector(".main-divider") as HTMLElement;
      splitter.dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowLeft", bubbles: true, cancelable: true }),
      );
      expect(sidebarWidth(rendered.root)).toBe("450px");

      await vi.advanceTimersByTimeAsync(500);
      await flushPromises();
      expect(dependencies.settingsUpdate).toHaveBeenCalledTimes(1);
      const updating = dispatchPulse(() => sample(800, 80, 1, ack(1, 800, 80)));
      expect(updating.complete.mock.calls[0][0]).toMatchObject({
        status: "skipped",
        reason: "persistence_owned",
      });

      updateGate.resolve();
      await flushPromises();
      const eligible = dispatchPulse(
        () => sample(800, 80, 2, ack(2, 800, 80)),
        "after-success",
        2,
      );
      expect(eligible.request.accepted).toBe(true);
      expect(eligible.complete).not.toHaveBeenCalled();
      expect(sidebarWidth(rendered.root)).toBe("434px");
    } finally {
      rendered.cleanup();
    }
  });

  it("skips overlap, replaces only a proven-stale owner, and completes every request once", async () => {
    const rendered = renderMain();
    try {
      await flushPromises();
      let firstCurrent = true;
      const first = dispatchPulse(() =>
        firstCurrent ? sample(800, 80, 1, ack(1, 800, 80)) : null,
      );
      const busy = dispatchPulse(() => sample(800, 80, 1, ack(1, 800, 80)), "busy", 1);
      expect(busy.complete.mock.calls[0][0]).toMatchObject({
        status: "skipped",
        reason: "busy",
      });

      firstCurrent = false;
      const replacement = dispatchPulse(
        () => sample(800, 80, 2, ack(2, 800, 80)),
        "replacement",
        2,
      );
      expect(first.complete).toHaveBeenCalledTimes(1);
      expect(first.complete.mock.calls[0][0]).toMatchObject({
        status: "cancelled",
        reason: "stale",
      });
      expect(replacement.complete).not.toHaveBeenCalled();
      expect(sidebarWidth(rendered.root)).toBe("424px");

      rendered.cleanup();
      expect(busy.complete).toHaveBeenCalledTimes(1);
      expect(replacement.complete).toHaveBeenCalledTimes(1);
      expect(replacement.complete.mock.calls[0][0]).toMatchObject({
        status: "cancelled",
        reason: "teardown",
      });
      await frames.flushFrame();
      expect(replacement.complete).toHaveBeenCalledTimes(1);
    } finally {
      if (rendered.root.isConnected) rendered.cleanup();
    }
  });

  it("does not treat corrupt live geometry as stale overlap evidence", async () => {
    const rendered = renderMain();
    try {
      await flushPromises();
      let current = sample(800, 80, 1, ack(1, 800, 80));
      const first = dispatchPulse(() => current);
      current = sample(Number.NaN, 80, 2, ack(2, 800, 80));

      const overlap = dispatchPulse(
        () => sample(800, 80, 3, ack(3, 800, 80)),
        "overlap",
        3,
      );
      expect(overlap.complete.mock.calls[0][0]).toMatchObject({
        status: "skipped",
        reason: "busy",
      });
      expect(first.complete).not.toHaveBeenCalled();

      await frames.flushFrame();
      expect(first.complete).toHaveBeenCalledTimes(1);
      expect(first.complete.mock.calls[0][0]).toMatchObject({
        status: "failed",
        reason: "exception",
        trace: {
          original: { sidebarWidth: null, hostWidth: null },
          expanded: { sidebarWidth: null, hostWidth: null },
          restored: { sidebarWidth: null, hostWidth: null },
        },
      });
      expect(sidebarWidth(rendered.root)).toBe("440px");
    } finally {
      rendered.cleanup();
    }
  });

  it("cancels on direct width theft without overwriting the stolen width", async () => {
    const rendered = renderMain();
    try {
      await flushPromises();
      const pulse = dispatchPulse(() => sample(800, 80, 1, ack(1, 800, 80)));
      expect(sidebarWidth(rendered.root)).toBe("424px");
      expect(signalControl.setSidebarWidth).not.toBeNull();

      signalControl.setSidebarWidth!(410);
      expect(sidebarWidth(rendered.root)).toBe("410px");
      await frames.flushFrame();

      expect(pulse.complete).toHaveBeenCalledTimes(1);
      expect(pulse.complete.mock.calls[0][0]).toMatchObject({
        status: "cancelled",
        reason: "width_changed",
      });
      expect(sidebarWidth(rendered.root)).toBe("410px");
    } finally {
      rendered.cleanup();
    }
  });

  it.each([
    ["pointer drag", "pointer"],
    ["divider keyboard", "keyboard"],
    ["window clamp", "window"],
    ["programmatic width", "programmatic"],
    ["programmatic side", "side"],
  ] as const)("cancels and restores before %s mutation", async (_label, action) => {
    const rendered = renderMain();
    try {
      await flushPromises();
      const pulse = dispatchPulse(() => sample(800, 80, 1, ack(1, 800, 80)));
      expect(sidebarWidth(rendered.root)).toBe("424px");
      const splitter = rendered.root.querySelector(".main-divider") as HTMLElement;

      if (action === "pointer") {
        splitter.dispatchEvent(new Event("pointerdown", { bubbles: true, cancelable: true }));
      } else if (action === "keyboard") {
        splitter.dispatchEvent(
          new KeyboardEvent("keydown", {
            key: "ArrowRight",
            bubbles: true,
            cancelable: true,
          }),
        );
      } else if (action === "window") {
        window.dispatchEvent(new Event("resize"));
      } else if (action === "programmatic") {
        window.dispatchEvent(
          new CustomEvent("main-sidebar-width-change", { detail: { width: 420 } }),
        );
      } else {
        window.dispatchEvent(
          new CustomEvent("main-sidebar-side-change", { detail: { side: "left" } }),
        );
      }

      expect(pulse.complete).toHaveBeenCalledTimes(1);
      expect(pulse.complete.mock.calls[0][0]).toMatchObject({
        status: "cancelled",
        reason: action === "pointer" ? "dragging" : "width_changed",
      });
      if (action === "keyboard") {
        expect(sidebarWidth(rendered.root)).toBe("430px");
      } else if (action === "programmatic") {
        expect(sidebarWidth(rendered.root)).toBe("420px");
      } else {
        expect(sidebarWidth(rendered.root)).toBe("440px");
      }

      if (action === "pointer") {
        const whileDragging = dispatchPulse(
          () => sample(800, 80, 2, ack(2, 800, 80)),
          "dragging",
          2,
        );
        expect(whileDragging.complete.mock.calls[0][0]).toMatchObject({
          status: "skipped",
          reason: "dragging",
        });
      }
    } finally {
      rendered.cleanup();
    }
  });
});
