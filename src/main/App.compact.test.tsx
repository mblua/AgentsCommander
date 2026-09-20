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
} from "../shared/testing/ui-harness";
import {
  railNudgePx,
  restoreWidthPx,
  setSidebarCompactMode,
  sidebarCompact,
} from "../shared/sidebar-compact";

// Same seams App.sidebar-width.test.tsx mocks (its helpers are file-local, so
// the pattern is reproduced here rather than imported), under this file's own
// names so the scaffolding is not a byte clone of that preserved file.
const testDoubles = vi.hoisted(() => ({
  readSettings: vi.fn(),
  writeSettings: vi.fn(),
  startZoom: vi.fn(),
  startGeometry: vi.fn(),
  wireHome: vi.fn(),
  wireCentral: vi.fn(),
  startWatchdogClient: vi.fn(),
  selectInitialView: vi.fn(),
}));

vi.mock("../shared/ipc", () => ({
  SettingsAPI: {
    get: testDoubles.readSettings,
    update: testDoubles.writeSettings,
  },
}));
vi.mock("../shared/platform", () => ({ isTauri: false }));
vi.mock("../shared/zoom", () => ({ initZoom: testDoubles.startZoom }));
vi.mock("../shared/window-geometry", () => ({
  initWindowGeometry: testDoubles.startGeometry,
}));
vi.mock("../sidebar/watchdog/non-stop-watchdog-client", () => ({
  startNonStopWatchdogClient: testDoubles.startWatchdogClient,
}));
vi.mock("./listeners-home", () => ({
  wireHomeListeners: testDoubles.wireHome,
}));
vi.mock("./listeners-central-view", () => ({
  wireCentralViewListeners: testDoubles.wireCentral,
}));
vi.mock("./stores/centralView", () => ({
  centralViewStore: {
    isResourceMonitor: false,
    setInitialView: testDoubles.selectInitialView,
  },
}));
vi.mock("../sidebar/components/Titlebar", () => ({ default: () => null }));
vi.mock("../resource-monitor/App", () => ({ default: () => null }));
vi.mock("./components/ErrorModal", () => ({ default: () => null }));
vi.mock("../sidebar/App", () => ({ default: () => null }));
vi.mock("./components/QuitConfirmModal", () => ({ default: () => null }));
vi.mock("../terminal/App", () => ({ default: () => null }));
vi.mock("../shared/components/ExternalLinkConfirm", () => ({ default: () => null }));

import MainApp from "./App";

function appSettings() {
  return {
    themeLight: false,
    mainSidebarWidth: 440,
    mainSidebarSide: "right",
    mainResourceMonitorAttached: false,
    mainAlwaysOnTop: false,
  };
}

async function settle(): Promise<void> {
  for (let pass = 0; pass < 8; pass += 1) {
    await Promise.resolve();
  }
}

function settingsGate() {
  let release!: (value: ReturnType<typeof appSettings>) => void;
  const pending = new Promise<ReturnType<typeof appSettings>>((resolve) => {
    release = resolve;
  });
  return { pending, release };
}

function mountMainApp() {
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

function pointerAt(
  element: Element,
  type: "pointerdown" | "pointermove" | "pointerup" | "pointercancel",
  clientX: number,
  pointerId = 2236,
): void {
  const event = new MouseEvent(type, {
    bubbles: true,
    cancelable: true,
    clientX,
    button: 0,
  });
  Object.defineProperty(event, "pointerId", { value: pointerId });
  element.dispatchEvent(event);
}

function pressKey(element: Element, key: string): void {
  element.dispatchEvent(
    new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }),
  );
}

function sendPulseRequest(sampleNow: () => MainTerminalLayoutPulseSample | null) {
  const complete = vi.fn<(result: MainTerminalLayoutPulseResult) => void>();
  const request: MainTerminalLayoutPulseRequest = {
    requestId: 1,
    sessionId: "compact-pulse",
    attachGeneration: 1,
    accepted: false,
    sample: sampleNow,
    complete,
  };
  window.dispatchEvent(
    new CustomEvent<MainTerminalLayoutPulseRequest>(
      MAIN_TERMINAL_LAYOUT_PULSE_REQUEST_EVENT,
      { detail: request },
    ),
  );
  return complete;
}

const COMPACT_WIDTH = "calc(var(--ac-rail-width) + 0px)";

describe("MainApp compact host (#2236)", () => {
  let cleanupDom: (() => void) | null = null;
  let error: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    Object.defineProperty(window, "innerWidth", {
      configurable: true,
      writable: true,
      value: 1400,
    });
    testDoubles.readSettings.mockReset().mockResolvedValue(appSettings());
    testDoubles.writeSettings.mockReset().mockResolvedValue(undefined);
    testDoubles.startZoom.mockReset().mockResolvedValue(() => {});
    testDoubles.startGeometry.mockReset().mockResolvedValue(() => {});
    testDoubles.wireHome.mockReset().mockResolvedValue([]);
    testDoubles.wireCentral.mockReset().mockResolvedValue([]);
    testDoubles.startWatchdogClient.mockReset();
    testDoubles.selectInitialView.mockReset();
    error = vi.spyOn(console, "error").mockImplementation(() => {});
    resetSidebarCompactForTests();
  });

  afterEach(() => {
    resetSidebarCompactForTests();
    vi.useRealTimers();
    cleanupDom?.();
    cleanupDom = null;
    error.mockRestore();
    document.body.style.cursor = "";
    document.body.innerHTML = "";
    document.documentElement.classList.remove("light-theme");
  });

  it("renders the pane at the live width while expanded", async () => {
    const app = mountMainApp();
    try {
      await settle();
      expect(sidebarCompact()).toBe(false);
      expect(paneWidth(app.host)).toBe("440px");
    } finally {
      app.unmount();
    }
  });

  it("renders the pane at the rail token while compact", async () => {
    const app = mountMainApp();
    try {
      await settle();
      setSidebarCompactMode(true);
      expect(sidebarCompact()).toBe(true);
      expect(paneWidth(app.host)).toBe(COMPACT_WIDTH);
    } finally {
      app.unmount();
    }
  });

  it("restores the collapse-time snapshot and ignores a preset received while compact", async () => {
    testDoubles.readSettings.mockResolvedValue({
      ...appSettings(),
      mainSidebarWidth: 520,
    });
    const app = mountMainApp();
    try {
      await settle();
      expect(paneWidth(app.host)).toBe("520px");

      setSidebarCompactMode(true);
      expect(restoreWidthPx()).toBe(520);

      window.dispatchEvent(
        new CustomEvent("main-sidebar-width-change", { detail: { width: 580 } }),
      );
      expect(restoreWidthPx()).toBe(520);

      setSidebarCompactMode(false);
      expect(paneWidth(app.host)).toBe("520px");
      expect(railNudgePx()).toBe(0);
    } finally {
      app.unmount();
    }
  });

  it("re-bases the snapshot when settings hydrate while compact", async () => {
    const gate = settingsGate();
    testDoubles.readSettings.mockReturnValueOnce(gate.pending);
    const app = mountMainApp();
    try {
      setSidebarCompactMode(true);
      expect(restoreWidthPx()).toBe(440);

      gate.release({ ...appSettings(), mainSidebarWidth: 520 });
      await settle();
      expect(restoreWidthPx()).toBe(520);

      setSidebarCompactMode(false);
      expect(paneWidth(app.host)).toBe("520px");
    } finally {
      app.unmount();
    }
  });

  it("ends a drag in flight before the snapshot and persists the dragged width", async () => {
    vi.useFakeTimers();
    const app = mountMainApp();
    try {
      await settle();
      const splitter = app.host.querySelector(".main-divider") as HTMLElement;
      const root = app.host.querySelector(".main-root") as HTMLElement;
      splitter.setPointerCapture = vi.fn();
      const released = vi.fn();
      splitter.releasePointerCapture = released;

      pointerAt(splitter, "pointerdown", 1000);
      expect(splitter.getAttribute("data-ac-state")).toBe("dragging");
      expect(root.getAttribute("data-ac-state")).toBe("dragging");

      pointerAt(splitter, "pointermove", 880);
      expect(paneWidth(app.host)).toBe("520px");

      setSidebarCompactMode(true);

      expect(splitter.getAttribute("data-ac-state")).toBe("idle");
      expect(root.getAttribute("data-ac-state")).toBe("idle");
      expect(restoreWidthPx()).toBe(520);
      expect(released).toHaveBeenCalledTimes(1);
      expect(released).toHaveBeenCalledWith(2236);

      pointerAt(splitter, "pointermove", 700);
      expect(paneWidth(app.host)).toBe(COMPACT_WIDTH);
      expect(restoreWidthPx()).toBe(520);

      await vi.advanceTimersByTimeAsync(500);
      expect(testDoubles.writeSettings).toHaveBeenCalledTimes(1);
      expect(testDoubles.writeSettings).toHaveBeenCalledWith(
        expect.objectContaining({ mainSidebarWidth: 520 }),
      );
    } finally {
      app.unmount();
    }
  });

  it("keeps the divider inert while compact and both handlers live while expanded", async () => {
    vi.useFakeTimers();
    const app = mountMainApp();
    try {
      await settle();
      const splitter = app.host.querySelector(".main-divider") as HTMLElement;
      const root = app.host.querySelector(".main-root") as HTMLElement;
      splitter.setPointerCapture = vi.fn();
      const released = vi.fn();
      splitter.releasePointerCapture = released;

      // Own 520px fixture, independent of the snapshot tests.
      window.dispatchEvent(
        new CustomEvent("main-sidebar-width-change", { detail: { width: 520 } }),
      );
      expect(paneWidth(app.host)).toBe("520px");

      setSidebarCompactMode(true);
      expect(splitter.getAttribute("aria-disabled")).toBe("true");
      expect(splitter.getAttribute("tabindex")).toBe("-1");

      pointerAt(splitter, "pointerdown", 1000);
      pointerAt(splitter, "pointermove", 880);
      pointerAt(splitter, "pointerup", 880);
      expect(root.getAttribute("data-ac-state")).toBe("idle");
      expect(splitter.getAttribute("data-ac-state")).toBe("idle");
      expect(paneWidth(app.host)).toBe(COMPACT_WIDTH);
      expect(restoreWidthPx()).toBe(520);
      expect(released).not.toHaveBeenCalled();
      await vi.advanceTimersByTimeAsync(500);
      expect(testDoubles.writeSettings).not.toHaveBeenCalled();

      pressKey(splitter, "ArrowLeft");
      pressKey(splitter, "End");
      expect(root.getAttribute("data-ac-state")).toBe("idle");
      expect(paneWidth(app.host)).toBe(COMPACT_WIDTH);
      expect(restoreWidthPx()).toBe(520);
      await vi.advanceTimersByTimeAsync(500);
      expect(testDoubles.writeSettings).not.toHaveBeenCalled();

      setSidebarCompactMode(false);
      expect(paneWidth(app.host)).toBe("520px");

      // Positive control 1: the same divider-targeted pointer sequence while
      // expanded must drag and persist, or the compact legs prove nothing.
      window.dispatchEvent(
        new CustomEvent("main-sidebar-width-change", { detail: { width: 520 } }),
      );
      released.mockClear();
      pointerAt(splitter, "pointerdown", 1000);
      expect(root.getAttribute("data-ac-state")).toBe("dragging");
      expect(splitter.getAttribute("data-ac-state")).toBe("dragging");
      pointerAt(splitter, "pointermove", 700);
      expect(paneWidth(app.host)).toBe("600px");
      pointerAt(splitter, "pointerup", 700);
      expect(splitter.getAttribute("data-ac-state")).toBe("idle");
      await vi.advanceTimersByTimeAsync(500);
      expect(testDoubles.writeSettings).toHaveBeenCalledTimes(1);
      expect(testDoubles.writeSettings).toHaveBeenLastCalledWith(
        expect.objectContaining({ mainSidebarWidth: 600 }),
      );

      // Positive control 2: ArrowLeft from a reset 520.
      window.dispatchEvent(
        new CustomEvent("main-sidebar-width-change", { detail: { width: 520 } }),
      );
      pressKey(splitter, "ArrowLeft");
      expect(paneWidth(app.host)).toBe("530px");
      await vi.advanceTimersByTimeAsync(500);
      expect(testDoubles.writeSettings).toHaveBeenCalledTimes(2);
      expect(testDoubles.writeSettings).toHaveBeenLastCalledWith(
        expect.objectContaining({ mainSidebarWidth: 530 }),
      );

      // Positive control 3: End from a reset 520.
      window.dispatchEvent(
        new CustomEvent("main-sidebar-width-change", { detail: { width: 520 } }),
      );
      pressKey(splitter, "End");
      expect(paneWidth(app.host)).toBe("600px");
      await vi.advanceTimersByTimeAsync(500);
      expect(testDoubles.writeSettings).toHaveBeenCalledTimes(3);
      expect(testDoubles.writeSettings).toHaveBeenLastCalledWith(
        expect.objectContaining({ mainSidebarWidth: 600 }),
      );
    } finally {
      app.unmount();
    }
  });

  it("reports the snapshot on the divider while compact, through a re-clamp", async () => {
    testDoubles.readSettings.mockResolvedValue({
      ...appSettings(),
      mainSidebarWidth: 520,
    });
    const app = mountMainApp();
    try {
      await settle();
      const splitter = app.host.querySelector(".main-divider") as HTMLElement;

      setSidebarCompactMode(true);
      expect(splitter.getAttribute("aria-valuenow")).toBe("520");
      expect(splitter.getAttribute("aria-valuetext")).toBe(
        "520 pixels, sidebar on right",
      );

      Object.defineProperty(window, "innerWidth", {
        configurable: true,
        writable: true,
        value: 700,
      });
      window.dispatchEvent(new Event("resize"));

      expect(splitter.getAttribute("aria-valuenow")).toBe("520");
      expect(splitter.getAttribute("aria-valuetext")).toBe(
        "520 pixels, sidebar on right",
      );
    } finally {
      app.unmount();
    }
  });

  it("animates a mode change symmetrically and disarms the 400 ms fallback", async () => {
    vi.useFakeTimers();
    const app = mountMainApp();
    try {
      await settle();
      const pane = app.host.querySelector(".main-sidebar-pane") as HTMLElement;

      setSidebarCompactMode(true);
      expect(pane.classList.contains("ac-sidebar-animating")).toBe(true);
      expect(vi.getTimerCount()).toBe(1);
      pane.dispatchEvent(new Event("transitionend", { bubbles: true }));
      expect(pane.classList.contains("ac-sidebar-animating")).toBe(false);
      expect(vi.getTimerCount()).toBe(0);

      setSidebarCompactMode(false);
      expect(pane.classList.contains("ac-sidebar-animating")).toBe(true);
      pane.dispatchEvent(new Event("transitionend", { bubbles: true }));
      expect(pane.classList.contains("ac-sidebar-animating")).toBe(false);
      expect(vi.getTimerCount()).toBe(0);

      setSidebarCompactMode(true);
      expect(pane.classList.contains("ac-sidebar-animating")).toBe(true);
      await vi.advanceTimersByTimeAsync(400);
      expect(pane.classList.contains("ac-sidebar-animating")).toBe(false);
      expect(vi.getTimerCount()).toBe(0);
      setSidebarCompactMode(false);
    } finally {
      app.unmount();
    }
  });

  it("does not animate a non-toggle width write", async () => {
    const app = mountMainApp();
    try {
      await settle();
      const pane = app.host.querySelector(".main-sidebar-pane") as HTMLElement;
      const splitter = app.host.querySelector(".main-divider") as HTMLElement;

      window.dispatchEvent(
        new CustomEvent("main-sidebar-width-change", { detail: { width: 520 } }),
      );
      expect(paneWidth(app.host)).toBe("520px");
      expect(pane.classList.contains("ac-sidebar-animating")).toBe(false);

      pressKey(splitter, "ArrowLeft");
      expect(paneWidth(app.host)).toBe("530px");
      expect(pane.classList.contains("ac-sidebar-animating")).toBe(false);
    } finally {
      app.unmount();
    }
  });

  it("cancels an in-flight pulse before the flip and snapshots the restored width", async () => {
    const app = mountMainApp();
    try {
      await settle();
      const frame = { hostWidth: 800, cols: 80, rows: 24 };
      const complete = sendPulseRequest(() => ({
        ...frame,
        observedObserverEpoch: 2,
        completedObserverAck: { epoch: 2, first: frame, second: frame },
      }));
      expect(paneWidth(app.host)).toBe("424px");

      setSidebarCompactMode(true);

      expect(complete).toHaveBeenCalledTimes(1);
      expect(complete.mock.calls[0][0]).toMatchObject({
        status: "cancelled",
        reason: "width_changed",
      });
      expect(paneWidth(app.host)).toBe(COMPACT_WIDTH);
      expect(restoreWidthPx()).toBe(440);
      expect(railNudgePx()).toBe(0);
    } finally {
      app.unmount();
    }
  });
});
