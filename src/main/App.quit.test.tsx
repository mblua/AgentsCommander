// @vitest-environment jsdom
//
// #2297 phase 3 - main window close handshake.
//
// The real `QuitConfirmModal`, the real IPC funnel and the real `FakeTransport`
// are used; only the Tauri window APIs, the heavy child surfaces and the
// settings bootstrap seams are stubbed. FakeTransport only records calls, so
// blackbox pending/overdue accounting is covered by the phase-2 fake-time seam
// in `src/shared/ipc-blackbox.test.ts` instead of being double-instrumented
// here.
import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const tauri = vi.hoisted(() => ({
  closeRequested: null as
    | ((event: { preventDefault: () => void }) => unknown)
    | null,
  webviews: [] as Array<{ label: string; destroy: () => Promise<void> }>,
  destroyCalls: [] as string[],
  geometry: { x: 10, y: 20, width: 1200, height: 800 },
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    label: "main",
    onCloseRequested: async (
      handler: (event: { preventDefault: () => void }) => unknown,
    ) => {
      tauri.closeRequested = handler;
      return () => {
        tauri.closeRequested = null;
      };
    },
    onMoved: async () => () => {},
    onResized: async () => () => {},
    setAlwaysOnTop: async () => undefined,
    minimize: () => undefined,
    maximize: () => undefined,
    unmaximize: () => undefined,
    isMaximized: async () => false,
    isFullscreen: async () => false,
    isMinimized: async () => false,
    outerPosition: async () => ({ x: tauri.geometry.x, y: tauri.geometry.y }),
    outerSize: async () => ({
      width: tauri.geometry.width,
      height: tauri.geometry.height,
    }),
    close: () => undefined,
    destroy: async () => {
      tauri.destroyCalls.push("main");
    },
  }),
}));

vi.mock("@tauri-apps/api/webviewWindow", () => ({
  WebviewWindow: {
    getAll: async () => tauri.webviews,
  },
  getCurrentWebviewWindow: () => ({ label: "main" }),
}));

vi.mock("../shared/platform", () => ({ isTauri: true, isBrowser: false }));
vi.mock("../shared/zoom", () => ({ initZoom: async () => () => {} }));
vi.mock("../sidebar/watchdog/non-stop-watchdog-client", () => ({
  startNonStopWatchdogClient: () => undefined,
}));
vi.mock("./listeners-home", () => ({ wireHomeListeners: async () => [] }));
vi.mock("./listeners-central-view", () => ({
  wireCentralViewListeners: async () => [],
}));
vi.mock("../sidebar/App", () => ({ default: () => null }));
vi.mock("../terminal/App", () => ({ default: () => null }));
vi.mock("../resource-monitor/App", () => ({ default: () => null }));
vi.mock("../sidebar/components/Titlebar", () => ({ default: () => null }));
vi.mock("./components/ErrorModal", () => ({ default: () => null }));
vi.mock("../shared/components/ExternalLinkConfirm", () => ({
  default: () => null,
}));

import MainApp from "./App";
import { __setTransportForTests } from "../shared/ipc";
import { FakeTransport } from "../shared/testing/fake-transport";

async function flush(): Promise<void> {
  await vi.advanceTimersByTimeAsync(0);
  for (let pass = 0; pass < 64; pass += 1) {
    await Promise.resolve();
  }
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

function settings() {
  return {
    themeLight: false,
    mainSidebarWidth: 360,
    mainSidebarSide: "right",
    mainResourceMonitorAttached: false,
    mainAlwaysOnTop: false,
  };
}

let fake: FakeTransport;
let restoreTransport: () => void;
let cleanup: (() => void) | null = null;
let errorSpy: ReturnType<typeof vi.spyOn>;
let warnSpy: ReturnType<typeof vi.spyOn>;
let alertSpy: ReturnType<typeof vi.spyOn>;

async function mountMain(): Promise<HTMLDivElement> {
  const root = document.createElement("div");
  document.body.append(root);
  const dispose = render(() => <MainApp />, root);
  cleanup = () => {
    dispose();
    root.remove();
  };
  await flush();
  return root;
}

async function triggerClose(): Promise<{ preventDefault: ReturnType<typeof vi.fn> }> {
  const handler = tauri.closeRequested;
  if (!handler) {
    throw new Error("main close handler was not installed");
  }
  const event = { preventDefault: vi.fn() };
  await handler(event);
  await flush();
  return event;
}

function attemptIdOf(callIndex = 0): string {
  const call = fake.callsFor("quit_application")[callIndex];
  if (!call) {
    throw new Error(`no quit_application call at index ${callIndex}`);
  }
  return String(call.args.attemptId);
}

async function emitStarted(epoch: number, callIndex = 0): Promise<void> {
  fake.emitFromBackend("app_quit_started", {
    epoch,
    attemptId: attemptIdOf(callIndex),
  });
  await flush();
}

async function emitOutcome(outcome: Record<string, unknown>): Promise<void> {
  fake.emitFromBackend("app_quit_outcome", outcome);
  await flush();
}

const statusText = (): string =>
  document.querySelector(".quit-status")?.textContent ?? "";
const forceOffer = (): HTMLButtonElement | null =>
  document.querySelector('[data-ac-testid="quit.forceOffer"]');
const keepWaiting = (): HTMLButtonElement | null =>
  document.querySelector('[data-ac-testid="quit.keepWaiting"]');
const forceConfirm = (): HTMLButtonElement | null =>
  document.querySelector('[data-ac-testid="quit.forceConfirm"]');
const detachedButtons = (): HTMLButtonElement[] =>
  Array.from(
    document.querySelectorAll<HTMLButtonElement>(
      '[data-ac-testid="quit.cancel"], [data-ac-testid="quit.confirm"]',
    ),
  );

function pressKey(key: string, shiftKey = false): KeyboardEvent {
  const event = new KeyboardEvent("keydown", {
    key,
    shiftKey,
    bubbles: true,
    cancelable: true,
  });
  document.dispatchEvent(event);
  return event;
}

describe("MainApp quit handshake (#2297)", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    fake = new FakeTransport();
    restoreTransport = __setTransportForTests(fake);
    fake.resolve("get_settings", settings());
    fake.resolve("set_main_window_placement", undefined);
    tauri.closeRequested = null;
    tauri.webviews = [];
    tauri.destroyCalls = [];
    tauri.geometry = { x: 10, y: 20, width: 1200, height: 800 };
    errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
    alertSpy = vi.spyOn(window, "alert").mockImplementation(() => {});
  });

  afterEach(async () => {
    cleanup?.();
    cleanup = null;
    // Settle a placement flush a test may have left in flight.
    await vi.advanceTimersByTimeAsync(2000);
    await flush();
    restoreTransport();
    vi.useRealTimers();
    errorSpy.mockRestore();
    warnSpy.mockRestore();
    alertSpy.mockRestore();
    document.body.replaceChildren();
  });

  it("intercepts a zero-detached close, shows waiting status and issues one normal invoke", async () => {
    fake.onInvoke("quit_application", () => new Promise(() => {}));
    await mountMain();
    const event = await triggerClose();

    expect(event.preventDefault).toHaveBeenCalledTimes(1);
    expect(statusText()).toContain("Waiting to quit...");
    expect(fake.callsFor("quit_application")).toHaveLength(1);
    const call = fake.lastCall("quit_application")!;
    expect(call.args.force).toBe(false);
    expect(typeof call.args.attemptId).toBe("string");
    expect((call.args.attemptId as string).length).toBeGreaterThan(0);
    // The scoped start listener exists before the invoke; the outcome
    // listener is scoped to main as well.
    expect(fake.listensFor("app_quit_started")).toHaveLength(1);
    expect(fake.listensFor("app_quit_started")[0].options).toEqual({
      scopeToCurrentWindow: true,
    });
    expect(fake.listensFor("app_quit_outcome")[0].options).toEqual({
      scopeToCurrentWindow: true,
    });
  });

  it("keeps the detached confirmation and Cancel never invokes a quit", async () => {
    tauri.webviews = [
      { label: "terminal-1", destroy: async () => undefined },
      { label: "main", destroy: async () => undefined },
    ];
    await mountMain();
    const event = await triggerClose();

    expect(event.preventDefault).toHaveBeenCalledTimes(1);
    const buttons = detachedButtons();
    expect(buttons.map((button) => button.dataset.acTestid)).toEqual([
      "quit.cancel",
      "quit.confirm",
    ]);
    expect(forceOffer()).toBeNull();

    buttons[0].click();
    await flush();
    expect(fake.callsFor("quit_application")).toHaveLength(0);
    expect(fake.callsFor("set_main_window_placement")).toHaveLength(0);
    expect(alertSpy).not.toHaveBeenCalled();
    expect(statusText()).toBe("");
    expect(detachedButtons()).toHaveLength(0);
  });

  it("runs the full 2-second grace after a rejection and ignores unbound outcomes", async () => {
    let attempts = 0;
    fake.onInvoke("quit_application", () => {
      attempts += 1;
      if (attempts === 1) {
        throw new Error("first transport failure");
      }
      return { outcome: "InFlight", epoch: 3 };
    });
    await mountMain();
    await triggerClose();
    expect(fake.callsFor("quit_application")).toHaveLength(1);

    // An outcome while no epoch is bound cannot be matched safely and is
    // dropped; the grace is unconditional.
    await emitOutcome({
      outcome: "Aborted",
      epoch: 99,
      reason: "refused",
      refusingLabels: ["spec-board"],
    });
    expect(statusText()).toContain("Waiting to quit...");

    await vi.advanceTimersByTimeAsync(1999);
    await flush();
    expect(fake.callsFor("quit_application")).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    await flush();
    expect(fake.callsFor("quit_application")).toHaveLength(2);
    expect(fake.callsFor("quit_application")[1].args.force).toBe(false);
    expect(attemptIdOf(1)).not.toBe(attemptIdOf(0));

    // The 10-second clock started at the first accepted close action, so the
    // grace and the retry consume it: Force appears at 10s, not later.
    await vi.advanceTimersByTimeAsync(7999);
    expect(forceOffer()).toBeNull();
    await vi.advanceTimersByTimeAsync(1);
    expect(forceOffer()).not.toBeNull();

    await emitOutcome({
      outcome: "Aborted",
      epoch: 3,
      reason: "destroyed",
      unansweredLabels: ["spec-board"],
    });
    expect(forceOffer()).toBeNull();
    expect(statusText()).toContain("destroyed");
    expect(statusText()).toContain("spec-board");
  });

  it("binds a start event during grace and skips the retry", async () => {
    fake.onInvoke("quit_application", () => {
      throw new Error("first");
    });
    await mountMain();
    await triggerClose();
    expect(fake.callsFor("quit_application")).toHaveLength(1);

    // The first attempt's start event arrives inside the grace window, so the
    // round is live and the retry timer must not fire a second invoke.
    await emitStarted(91);
    await vi.advanceTimersByTimeAsync(2000);
    await flush();
    expect(fake.callsFor("quit_application")).toHaveLength(1);

    await emitOutcome({ outcome: "Aborted", epoch: 91, reason: "cancelled" });
    expect(statusText()).toContain("cancelled");
  });

  it("ends in a visible failed state after both rejects and allows a fresh close", async () => {
    let attempts = 0;
    fake.onInvoke("quit_application", () => {
      attempts += 1;
      throw new Error(`reject-${attempts}`);
    });
    await mountMain();
    await triggerClose();
    await vi.advanceTimersByTimeAsync(2000);
    await flush();

    expect(fake.callsFor("quit_application")).toHaveLength(2);
    expect(statusText()).toContain("Quit failed");
    expect(statusText()).toContain("reject-1");
    expect(statusText()).toContain("reject-2");
    expect(forceOffer()).toBeNull();

    await triggerClose();
    expect(fake.callsFor("quit_application")).toHaveLength(3);
  });

  it("ignores a late start event for a superseded attempt", async () => {
    let attempts = 0;
    fake.onInvoke("quit_application", () => {
      attempts += 1;
      if (attempts === 1) {
        throw new Error("first");
      }
      return { outcome: "InFlight", epoch: 71 };
    });
    await mountMain();
    await triggerClose();
    await vi.advanceTimersByTimeAsync(2000);
    await flush();

    // The retry bound epoch 71; the first attempt's late start must not bind.
    await emitStarted(70, 0);
    await emitOutcome({
      outcome: "Aborted",
      epoch: 70,
      reason: "refused",
      refusingLabels: ["spec-board"],
    });
    expect(statusText()).toContain("Waiting to quit...");

    await emitOutcome({ outcome: "Aborted", epoch: 71, reason: "cancelled" });
    expect(statusText()).toContain("cancelled");
  });

  it("binds an InFlight result and ignores mismatched outcome epochs", async () => {
    fake.resolve("quit_application", { outcome: "InFlight", epoch: 12 });
    await mountMain();
    await triggerClose();
    expect(statusText()).toContain("Waiting to quit...");

    await emitOutcome({
      outcome: "Aborted",
      epoch: 13,
      reason: "refused",
      refusingLabels: ["spec-board"],
    });
    expect(statusText()).toContain("Waiting to quit...");

    await emitOutcome({
      outcome: "Aborted",
      epoch: 12,
      reason: "timeout",
      unansweredLabels: ["spec-board"],
    });
    expect(statusText()).toContain("timeout");
  });

  it("binds a matching start event during a slow invoke and dedupes its terminal result by epoch", async () => {
    const pending = deferred<unknown>();
    fake.onInvoke("quit_application", () => pending.promise);
    await mountMain();
    await triggerClose();
    await emitStarted(7);

    await vi.advanceTimersByTimeAsync(9999);
    expect(forceOffer()).toBeNull();
    await vi.advanceTimersByTimeAsync(1);
    expect(forceOffer()).not.toBeNull();

    // The invoke result and the duplicate event report the same Aborted; the
    // round is cleaned up once and the offer is gone.
    await emitOutcome({ outcome: "Aborted", epoch: 7, reason: "cancelled" });
    pending.resolve({ outcome: "Aborted", epoch: 7, reason: "cancelled" });
    await flush();
    expect(forceOffer()).toBeNull();
    expect(statusText()).toContain("cancelled");
    expect(fake.callsFor("quit_application")).toHaveLength(1);
  });

  it("offers Force at ten seconds, names the possible loss, declines safely and confirms once", async () => {
    tauri.webviews = [{ label: "terminal-1", destroy: async () => undefined }];
    fake.onInvoke("quit_application", (args) =>
      args.force
        ? { outcome: "Exiting", epoch: 21 }
        : { outcome: "InFlight", epoch: 21 },
    );
    await mountMain();
    await triggerClose();
    const buttons = detachedButtons();
    expect(buttons).toHaveLength(2);
    buttons[1].click();
    await flush();
    expect(detachedButtons()).toHaveLength(0);
    expect(statusText()).toContain("Waiting to quit...");

    await vi.advanceTimersByTimeAsync(9999);
    expect(forceOffer()).toBeNull();
    await vi.advanceTimersByTimeAsync(1);
    expect(forceOffer()).not.toBeNull();
    // Native button: the browser/OS routes click, Enter and Space to it.
    expect(forceOffer()!.tagName).toBe("BUTTON");
    expect(forceOffer()!.type).toBe("button");

    // Offer -> separate alertdialog; Keep waiting gets focus.
    forceOffer()!.click();
    await flush();
    expect(forceOffer()).toBeNull();
    const dialog = document.querySelector('[role="alertdialog"]')!;
    expect(dialog.textContent).toContain("Unsaved Spec Board changes will be lost");
    expect(dialog.textContent).toContain("save");
    expect(
      Array.from(dialog.querySelectorAll("button")).map(
        (button) => (button as HTMLElement).dataset.acTestid,
      ),
    ).toEqual(["quit.keepWaiting", "quit.forceConfirm"]);
    expect(document.activeElement).toBe(keepWaiting());

    // Escape declines only the Force dialog: no force call, offer restored and
    // focused, the same round still pending.
    const escape = pressKey("Escape");
    expect(escape.defaultPrevented).toBe(true);
    await flush();
    expect(keepWaiting()).toBeNull();
    expect(forceOffer()).not.toBeNull();
    expect(document.activeElement).toBe(forceOffer());
    expect(fake.callsFor("quit_application")).toHaveLength(1);

    // Tab confinement: last wraps to first, first Shift+Tab wraps to last.
    forceOffer()!.click();
    await flush();
    forceConfirm()!.focus();
    pressKey("Tab");
    expect(document.activeElement).toBe(keepWaiting());
    pressKey("Tab", true);
    expect(document.activeElement).toBe(forceConfirm());

    // Enter on Keep waiting declines again without calling onCancel behavior.
    keepWaiting()!.focus();
    pressKey("Enter");
    await flush();
    expect(forceOffer()).not.toBeNull();
    expect(fake.callsFor("quit_application")).toHaveLength(1);

    // Enter on Force quit confirms once for the bound epoch, then Exiting.
    forceOffer()!.click();
    await flush();
    forceConfirm()!.focus();
    pressKey("Enter");
    await flush();
    const forceCall = fake.lastCall("quit_application")!;
    expect(forceCall.args).toEqual({ force: true, epoch: 21 });
    expect(statusText()).toBe("");
    expect(keepWaiting()).toBeNull();
    expect(forceOffer()).toBeNull();
  });

  it("clears the Force controls on a refusal and never offers Force for that round", async () => {
    fake.resolve("quit_application", { outcome: "InFlight", epoch: 31 });
    await mountMain();
    await triggerClose();
    await vi.advanceTimersByTimeAsync(10000);
    expect(forceOffer()).not.toBeNull();
    forceOffer()!.click();
    await flush();
    expect(keepWaiting()).not.toBeNull();

    await emitOutcome({
      outcome: "Aborted",
      epoch: 31,
      reason: "refused",
      refusingLabels: ["spec-board"],
    });
    expect(keepWaiting()).toBeNull();
    expect(forceOffer()).toBeNull();
    await vi.advanceTimersByTimeAsync(30000);
    expect(forceOffer()).toBeNull();
    expect(statusText()).toContain("refused");
  });

  it("clears the Force clock on a matching cancellation and never offers Force again", async () => {
    fake.resolve("quit_application", { outcome: "InFlight", epoch: 41 });
    await mountMain();
    await triggerClose();
    await vi.advanceTimersByTimeAsync(10000);
    expect(forceOffer()).not.toBeNull();
    forceOffer()!.click();
    await flush();

    fake.emitFromBackend("app_quit_cancelled", { epoch: 41, label: "spec-board" });
    await flush();
    expect(keepWaiting()).toBeNull();
    expect(forceOffer()).toBeNull();
    await vi.advanceTimersByTimeAsync(30000);
    expect(forceOffer()).toBeNull();
    // The round itself is still waiting for its terminal outcome.
    expect(statusText()).toContain("Waiting to quit...");
  });

  it("clears the old Force UI on a stale force call without exiting", async () => {
    fake.onInvoke("quit_application", (args) =>
      args.force
        ? { outcome: "Stale", epoch: args.epoch }
        : { outcome: "InFlight", epoch: 51 },
    );
    await mountMain();
    await triggerClose();
    await vi.advanceTimersByTimeAsync(10000);
    forceOffer()!.click();
    await flush();
    forceConfirm()!.click();
    await flush();

    expect(fake.lastCall("quit_application")!.args).toEqual({ force: true, epoch: 51 });
    expect(keepWaiting()).toBeNull();
    expect(forceOffer()).toBeNull();
    expect(tauri.destroyCalls).toHaveLength(0);

    await triggerClose();
    expect(fake.callsFor("quit_application")).toHaveLength(3);
  });

  it("ignores a stale Force result from a replaced epoch without disturbing the new round", async () => {
    const forceCall = deferred<unknown>();
    let normalAttempts = 0;
    fake.onInvoke("quit_application", (args) => {
      if (args.force) {
        return forceCall.promise;
      }
      normalAttempts += 1;
      return { outcome: "InFlight", epoch: normalAttempts === 1 ? 81 : 82 };
    });
    await mountMain();
    await triggerClose();
    await vi.advanceTimersByTimeAsync(10000);
    forceOffer()!.click();
    await flush();
    forceConfirm()!.click();
    await flush();

    // Round 81 aborts while the Force call is in flight; a fresh close starts
    // round 82.
    await emitOutcome({ outcome: "Aborted", epoch: 81, reason: "refused" });
    expect(statusText()).toContain("refused");
    await triggerClose();
    expect(fake.callsFor("quit_application")).toHaveLength(3);
    expect(statusText()).toContain("Waiting to quit...");

    // The stale Force result for the replaced epoch must not touch round 82.
    forceCall.resolve({ outcome: "Stale", epoch: 81 });
    await flush();
    expect(statusText()).toContain("Waiting to quit...");
    expect(forceOffer()).toBeNull();
    await emitOutcome({ outcome: "Aborted", epoch: 82, reason: "unregistered" });
    expect(statusText()).toContain("unregistered");
  });

  it("focuses the connected titlebar close after a terminal abort", async () => {
    const closeButton = document.createElement("button");
    closeButton.dataset.acTestid = "titlebar.close";
    document.body.append(closeButton);
    fake.resolve("quit_application", { outcome: "InFlight", epoch: 61 });
    await mountMain();
    await triggerClose();

    await emitOutcome({
      outcome: "Aborted",
      epoch: 61,
      reason: "unregistered",
      unansweredLabels: ["spec-board"],
    });
    expect(document.activeElement).toBe(closeButton);
  });

  // #2349 - the bounded placement flush precedes the first quit command.
  it("awaits the placement save before the zero-detached quit command", async () => {
    const placement = deferred<void>();
    fake.onInvoke("set_main_window_placement", () => placement.promise);
    fake.onInvoke("quit_application", () => new Promise(() => {}));
    await mountMain();

    await triggerClose();
    expect(fake.callsFor("set_main_window_placement")).toHaveLength(1);
    expect(fake.callsFor("quit_application")).toHaveLength(0);

    placement.resolve();
    await flush();
    expect(fake.callsFor("quit_application")).toHaveLength(1);
  });

  it("awaits the placement save before the confirmed detached quit command", async () => {
    tauri.webviews = [{ label: "terminal-1", destroy: async () => undefined }];
    const placement = deferred<void>();
    fake.onInvoke("set_main_window_placement", () => placement.promise);
    fake.onInvoke("quit_application", () => new Promise(() => {}));
    await mountMain();

    await triggerClose();
    expect(fake.callsFor("set_main_window_placement")).toHaveLength(0);
    detachedButtons()[1].click();
    await flush();
    expect(fake.callsFor("set_main_window_placement")).toHaveLength(1);
    expect(fake.callsFor("quit_application")).toHaveLength(0);

    placement.resolve();
    await flush();
    expect(fake.callsFor("quit_application")).toHaveLength(1);
  });

  it("bounds a pending-forever placement save at two seconds and still issues one first quit command", async () => {
    fake.onInvoke("set_main_window_placement", () => new Promise(() => {}));
    fake.onInvoke("quit_application", () => ({ outcome: "InFlight", epoch: 44 }));
    await mountMain();

    await triggerClose();
    expect(fake.callsFor("set_main_window_placement")).toHaveLength(1);

    await vi.advanceTimersByTimeAsync(1999);
    await flush();
    expect(fake.callsFor("quit_application")).toHaveLength(0);

    await vi.advanceTimersByTimeAsync(1);
    await flush();
    expect(fake.callsFor("quit_application")).toHaveLength(1);
    expect(statusText()).toContain("Waiting to quit...");
    expect(alertSpy).not.toHaveBeenCalled();
    expect(errorSpy).toHaveBeenCalled();

    // The 10-second Force clock stays live across the bounded wait.
    await vi.advanceTimersByTimeAsync(7999);
    expect(forceOffer()).toBeNull();
    await vi.advanceTimersByTimeAsync(1);
    expect(forceOffer()).not.toBeNull();
  });

  it("starts a fresh placement save with the newer bounds after the bounded attempt settles", async () => {
    let placementInvokes = 0;
    fake.onInvoke("set_main_window_placement", () => {
      placementInvokes += 1;
      return placementInvokes === 1 ? new Promise(() => {}) : undefined;
    });
    let quitAttempts = 0;
    fake.onInvoke("quit_application", () => {
      quitAttempts += 1;
      if (quitAttempts <= 2) {
        throw new Error(`reject-${quitAttempts}`);
      }
      return { outcome: "InFlight", epoch: 902 };
    });
    await mountMain();

    await triggerClose();
    await vi.advanceTimersByTimeAsync(2000);
    await flush();
    expect(fake.callsFor("set_main_window_placement")).toHaveLength(1);
    expect(fake.callsFor("quit_application")).toHaveLength(1);

    // Both normal attempts reject: the round fails and the close latch releases.
    await vi.advanceTimersByTimeAsync(2000);
    await flush();
    expect(statusText()).toContain("Quit failed");

    tauri.geometry = { x: 300, y: 200, width: 900, height: 600 };
    await triggerClose();
    await flush();
    expect(fake.callsFor("set_main_window_placement")).toHaveLength(2);
    expect(fake.lastCall("set_main_window_placement")!.args).toEqual({
      geometry: { x: 300, y: 200, width: 900, height: 600 },
      displayState: "normal",
    });
  });

  it("alerts once per accepted close round for an overlay-pinned placement", async () => {
    fake.onInvoke("set_main_window_placement", () => {
      throw "main_window_placement_overlay_pinned";
    });
    let quitAttempts = 0;
    fake.onInvoke("quit_application", (args) => {
      if (args.force) {
        return { outcome: "Aborted", epoch: args.epoch, reason: "refused" };
      }
      quitAttempts += 1;
      if (quitAttempts === 1) {
        throw new Error("first quit attempt failed");
      }
      return { outcome: "InFlight", epoch: 701 };
    });
    await mountMain();

    await triggerClose();
    await flush();
    expect(alertSpy).toHaveBeenCalledTimes(1);
    expect(alertSpy).toHaveBeenCalledWith(
      "Window placement is pinned by the local settings overlay and was not saved.",
    );
    expect(errorSpy).toHaveBeenCalled();

    // The retry inside the same round reuses the settled flush: no second alert.
    await vi.advanceTimersByTimeAsync(2000);
    await flush();
    expect(fake.callsFor("quit_application")).toHaveLength(2);
    expect(alertSpy).toHaveBeenCalledTimes(1);

    // Force confirmation in the same round also reuses it: no second alert.
    await vi.advanceTimersByTimeAsync(7999);
    expect(forceOffer()).toBeNull();
    await vi.advanceTimersByTimeAsync(1);
    expect(forceOffer()).not.toBeNull();
    forceOffer()!.click();
    await flush();
    forceConfirm()!.click();
    await flush();
    expect(fake.lastCall("quit_application")!.args.force).toBe(true);
    expect(alertSpy).toHaveBeenCalledTimes(1);

    // A newly accepted close round gets a fresh guard and may alert once again.
    await triggerClose();
    await flush();
    expect(alertSpy).toHaveBeenCalledTimes(2);
  });
});
