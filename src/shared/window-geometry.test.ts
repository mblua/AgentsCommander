// @vitest-environment jsdom
//
// #2349 - deterministic unit tests for the renderer-side main-window placement
// observation and the bounded quit flush. The real `window-geometry` module, the
// real IPC funnel and the real `FakeTransport` are used; only the Tauri window
// API and the platform flag are stubbed.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const tauriWindow = vi.hoisted(() => ({
  movedHandler: null as (() => void) | null,
  resizedHandler: null as (() => void) | null,
  isMaximized: vi.fn(),
  isFullscreen: vi.fn(),
  isMinimized: vi.fn(),
  outerPosition: vi.fn(),
  outerSize: vi.fn(),
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    onMoved: async (handler: () => void) => {
      tauriWindow.movedHandler = handler;
      return () => {
        tauriWindow.movedHandler = null;
      };
    },
    onResized: async (handler: () => void) => {
      tauriWindow.resizedHandler = handler;
      return () => {
        tauriWindow.resizedHandler = null;
      };
    },
    isMaximized: tauriWindow.isMaximized,
    isFullscreen: tauriWindow.isFullscreen,
    isMinimized: tauriWindow.isMinimized,
    outerPosition: tauriWindow.outerPosition,
    outerSize: tauriWindow.outerSize,
  }),
}));

vi.mock("./platform", () => ({ isTauri: true }));

import { __setTransportForTests } from "./ipc";
import { FakeTransport } from "./testing/fake-transport";
import {
  flushMainWindowGeometry,
  initWindowGeometry,
  MAIN_WINDOW_QUIT_FLUSH_TIMEOUT_MS,
} from "./window-geometry";

const SEED = { x: 100, y: 50, width: 1200, height: 800 };
const NORMAL = { x: 40, y: 30, width: 1100, height: 700 };
const OTHER = { x: 300, y: 200, width: 900, height: 600 };
const MAXED = { x: 0, y: 0, width: 2560, height: 1440 };
const SCREEN = { x: 0, y: 0, width: 2560, height: 1440 };

let fake: FakeTransport;
let restoreTransport: () => void;
let disposeController: (() => void) | null = null;
let errorSpy: ReturnType<typeof vi.spyOn>;

function setWindowState(state: {
  isMaximized?: boolean;
  isFullscreen?: boolean;
  isMinimized?: boolean;
  geometry?: { x: number; y: number; width: number; height: number };
}): void {
  tauriWindow.isMaximized.mockResolvedValue(state.isMaximized ?? false);
  tauriWindow.isFullscreen.mockResolvedValue(state.isFullscreen ?? false);
  tauriWindow.isMinimized.mockResolvedValue(state.isMinimized ?? false);
  const geometry = state.geometry ?? NORMAL;
  tauriWindow.outerPosition.mockResolvedValue({ x: geometry.x, y: geometry.y });
  tauriWindow.outerSize.mockResolvedValue({
    width: geometry.width,
    height: geometry.height,
  });
}

async function flushMicrotasks(): Promise<void> {
  for (let pass = 0; pass < 40; pass += 1) {
    await Promise.resolve();
  }
}

async function mountMain(): Promise<void> {
  disposeController = await initWindowGeometry("main");
  await flushMicrotasks();
}

async function advance(ms: number): Promise<void> {
  await vi.advanceTimersByTimeAsync(ms);
  await flushMicrotasks();
}

/** A move/resize burst followed by the 500 ms debounce window. */
async function moveAndSettle(): Promise<void> {
  tauriWindow.movedHandler?.();
  tauriWindow.resizedHandler?.();
  await flushMicrotasks();
  await advance(500);
}

const placementCalls = () => fake.callsFor("set_main_window_placement");
const lastPlacementArgs = (): Record<string, unknown> => {
  const calls = placementCalls();
  const last = calls[calls.length - 1];
  if (!last) {
    throw new Error("no set_main_window_placement call recorded");
  }
  return last.args;
};

beforeEach(() => {
  vi.useFakeTimers();
  vi.clearAllMocks();
  fake = new FakeTransport();
  restoreTransport = __setTransportForTests(fake);
  fake.resolve("get_settings", { mainGeometry: SEED });
  fake.resolve("set_main_window_placement", undefined);
  tauriWindow.movedHandler = null;
  tauriWindow.resizedHandler = null;
  setWindowState({});
  errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
});

afterEach(async () => {
  disposeController?.();
  disposeController = null;
  // Settle any flush left in flight so its module slot cannot leak across tests.
  await vi.advanceTimersByTimeAsync(MAIN_WINDOW_QUIT_FLUSH_TIMEOUT_MS);
  await flushMicrotasks();
  restoreTransport();
  vi.useRealTimers();
  errorSpy.mockRestore();
});

describe("main window placement observation (#2349)", () => {
  it("persists the persisted seed with maximized state on a start-maximized first observation", async () => {
    setWindowState({ isMaximized: true, geometry: MAXED });
    await mountMain();

    await moveAndSettle();

    expect(placementCalls()).toHaveLength(1);
    expect(placementCalls()[0].args).toEqual({
      geometry: SEED,
      displayState: "maximized",
    });
  });

  it.each([
    ["maximized", { isMaximized: true }],
    ["fullscreen", { isFullscreen: true }],
    ["minimized", { isMinimized: true }],
  ])(
    "does not retain a screen-sized rectangle while the first observation is %s with no seed",
    async (_label, special) => {
      fake.resolve("get_settings", { mainGeometry: null });
      setWindowState({ ...special, geometry: SCREEN });
      await mountMain();

      await moveAndSettle();
      expect(placementCalls()).toHaveLength(0);

      // The first later normal observation enables saving.
      setWindowState({ geometry: NORMAL });
      await moveAndSettle();
      expect(placementCalls()).toHaveLength(1);
      expect(placementCalls()[0].args).toEqual({
        geometry: NORMAL,
        displayState: "normal",
      });
    },
  );

  it("treats an invalid persisted seed as absent", async () => {
    fake.resolve("get_settings", {
      mainGeometry: { x: 10, y: 20, width: 0, height: 0 },
    });
    setWindowState({ isMaximized: true, geometry: MAXED });
    await mountMain();

    await moveAndSettle();

    expect(placementCalls()).toHaveLength(0);
  });

  it("replaces the retained bounds on a normal observation", async () => {
    await mountMain();

    setWindowState({ geometry: OTHER });
    await moveAndSettle();

    expect(lastPlacementArgs()).toEqual({
      geometry: OTHER,
      displayState: "normal",
    });
  });

  it("keeps the normal bounds and persists maximized on a maximize", async () => {
    await mountMain();
    setWindowState({ geometry: NORMAL });
    await moveAndSettle();

    setWindowState({ isMaximized: true, geometry: MAXED });
    await moveAndSettle();

    expect(lastPlacementArgs()).toEqual({
      geometry: NORMAL,
      displayState: "maximized",
    });
  });

  it("keeps the pre-fullscreen bounds and state while fullscreen", async () => {
    setWindowState({ isMaximized: true, geometry: MAXED });
    await mountMain();
    await moveAndSettle();
    expect(lastPlacementArgs()).toEqual({
      geometry: SEED,
      displayState: "maximized",
    });

    setWindowState({ isFullscreen: true, geometry: SCREEN });
    await moveAndSettle();

    expect(placementCalls()).toHaveLength(2);
    expect(lastPlacementArgs()).toEqual({
      geometry: SEED,
      displayState: "maximized",
    });
  });

  it("keeps the last non-minimized state while minimized", async () => {
    setWindowState({ geometry: NORMAL });
    await mountMain();
    await moveAndSettle();
    expect(lastPlacementArgs()).toEqual({
      geometry: NORMAL,
      displayState: "normal",
    });

    setWindowState({ isMinimized: true, isMaximized: true, geometry: MAXED });
    await moveAndSettle();

    expect(placementCalls()).toHaveLength(2);
    expect(lastPlacementArgs()).toEqual({
      geometry: NORMAL,
      displayState: "normal",
    });
  });

  it("coalesces a move/resize burst into one save", async () => {
    await mountMain();

    tauriWindow.movedHandler?.();
    tauriWindow.resizedHandler?.();
    tauriWindow.movedHandler?.();
    await flushMicrotasks();
    await advance(499);
    expect(placementCalls()).toHaveLength(0);

    await advance(1);
    expect(placementCalls()).toHaveLength(1);
  });
});

describe("main window quit flush (#2349)", () => {
  it("resolves noop when no main controller exists", async () => {
    expect(await flushMainWindowGeometry()).toEqual({ kind: "noop" });
  });

  it("flushes a pending debounce immediately at the latest observation", async () => {
    setWindowState({ geometry: OTHER });
    await mountMain();

    tauriWindow.movedHandler?.();
    await flushMicrotasks();
    await advance(100);
    expect(placementCalls()).toHaveLength(0);

    expect(await flushMainWindowGeometry()).toEqual({ kind: "saved" });
    expect(placementCalls()).toHaveLength(1);
    expect(placementCalls()[0].args).toEqual({
      geometry: OTHER,
      displayState: "normal",
    });

    // The cancelled debounce must not fire a second save.
    await advance(500);
    expect(placementCalls()).toHaveLength(1);
  });

  it("shares one bounded promise across concurrent calls in one attempt", async () => {
    fake.onInvoke("set_main_window_placement", () => new Promise(() => {}));
    await mountMain();

    const first = flushMainWindowGeometry();
    const second = flushMainWindowGeometry();
    expect(second).toBe(first);
    await flushMicrotasks();
    expect(placementCalls()).toHaveLength(1);

    let settled = false;
    void first.then(() => {
      settled = true;
    });
    await advance(MAIN_WINDOW_QUIT_FLUSH_TIMEOUT_MS - 1);
    expect(settled).toBe(false);
    expect(placementCalls()).toHaveLength(1);

    await advance(1);
    expect(await first).toEqual({ kind: "failed" });
    expect(await second).toEqual({ kind: "failed" });
    expect(settled).toBe(true);
  });

  it("times out as failed and detaches a never-settling invoke", async () => {
    fake.onInvoke("set_main_window_placement", () => new Promise(() => {}));
    await mountMain();

    const pending = flushMainWindowGeometry();
    await flushMicrotasks();
    await advance(MAIN_WINDOW_QUIT_FLUSH_TIMEOUT_MS - 1);
    expect(placementCalls()).toHaveLength(1);

    await advance(1);
    expect(await pending).toEqual({ kind: "failed" });
    expect(errorSpy).toHaveBeenCalledTimes(1);
  });

  it("starts a fresh invoke with the newer bounds after a bounded attempt settles", async () => {
    let placementCount = 0;
    fake.onInvoke("set_main_window_placement", () => {
      placementCount += 1;
      return placementCount === 1 ? new Promise(() => {}) : undefined;
    });
    await mountMain();

    const first = flushMainWindowGeometry();
    await flushMicrotasks();
    expect(placementCalls()).toHaveLength(1);
    await advance(MAIN_WINDOW_QUIT_FLUSH_TIMEOUT_MS);
    expect(await first).toEqual({ kind: "failed" });

    setWindowState({ geometry: OTHER });
    const second = flushMainWindowGeometry();
    expect(second).not.toBe(first);
    await flushMicrotasks();

    expect(placementCalls()).toHaveLength(2);
    expect(placementCalls()[1].args).toEqual({
      geometry: OTHER,
      displayState: "normal",
    });
    expect(await second).toEqual({ kind: "saved" });
  });

  it("maps only the exact overlay rejection to overlay-pinned", async () => {
    fake.onInvoke("set_main_window_placement", () => {
      throw "main_window_placement_overlay_pinned";
    });
    await mountMain();

    expect(await flushMainWindowGeometry()).toEqual({ kind: "overlay-pinned" });
    expect(errorSpy).not.toHaveBeenCalled();
  });

  it("maps an ordinary failure to failed with a console error", async () => {
    fake.onInvoke("set_main_window_placement", () => {
      throw new Error("placement write failed");
    });
    await mountMain();

    expect(await flushMainWindowGeometry()).toEqual({ kind: "failed" });
    expect(errorSpy).toHaveBeenCalled();
  });

  it("keeps the callable cleanup contract and stops flushing after disposal", async () => {
    await mountMain();
    expect(tauriWindow.movedHandler).not.toBeNull();
    expect(tauriWindow.resizedHandler).not.toBeNull();

    disposeController?.();
    disposeController = null;

    expect(tauriWindow.movedHandler).toBeNull();
    expect(tauriWindow.resizedHandler).toBeNull();
    expect(await flushMainWindowGeometry()).toEqual({ kind: "noop" });
    expect(placementCalls()).toHaveLength(0);
  });
});
