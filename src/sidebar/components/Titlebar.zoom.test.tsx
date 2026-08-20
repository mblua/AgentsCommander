// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Component, JSX } from "solid-js";
import type { AppSettings, WebServerOwnedStatus } from "../../shared/types";
import type { FakeTransport } from "../../shared/testing/fake-transport";

const tauriMocks = vi.hoisted(() => ({
  invoke: vi.fn(() => Promise.resolve("")),
  setZoom: vi.fn(() => Promise.resolve()),
  minimize: vi.fn(),
  isMaximized: vi.fn(() => Promise.resolve(false)),
  maximize: vi.fn(),
  unmaximize: vi.fn(),
  close: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: tauriMocks.invoke,
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    minimize: tauriMocks.minimize,
    isMaximized: tauriMocks.isMaximized,
    maximize: tauriMocks.maximize,
    unmaximize: tauriMocks.unmaximize,
    close: tauriMocks.close,
  }),
}));

vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({
    setZoom: tauriMocks.setZoom,
  }),
}));

type InitZoom = typeof import("../../shared/zoom").initZoom;

type HarnessModules = {
  Titlebar: Component;
  FakeTransport: typeof FakeTransport;
  baseSettings: (overrides?: Partial<AppSettings>) => AppSettings;
  renderWithFakeTransport: (
    component: () => JSX.Element,
    fake?: FakeTransport,
  ) => { fake: FakeTransport; root: HTMLDivElement; cleanup: () => void };
  click: (el: Element) => void;
  waitFor: (assertion: () => void | Promise<void>, timeoutMs?: number) => Promise<void>;
  initZoom: InitZoom;
};

interface MountedTitlebar {
  fake: FakeTransport;
  root: HTMLDivElement;
  modules: HarnessModules;
  getSettings: () => AppSettings;
}

const cleanups: Array<() => void> = [];

// --- #1093: always-on leaked zoom-listener tracking (plan §4) -----------------
// initZoom (src/shared/zoom.ts:136-137) attaches its wheel/keydown listeners on the
// shared jsdom `document` only after two awaits. A test truncated mid-initZoom can
// let that continuation resume later and attach an ORPHAN document listener whose
// cleanup was never captured into `cleanups`. jsdom builds one `document` per file,
// so that orphan would survive into a sibling test. To make the leak deterministically
// removable we wrap `document`'s add/removeEventListener once — file-wide, never
// restored (plan §4.1) — track every live wheel/keydown listener, and scrub survivors
// at each test boundary.
//
// Single source of truth: this tracked-type set is tied to initZoom's own registration
// (zoom.ts:136-137). If initZoom ever registers another document event type, extend
// this list so the scrub and the registry-empty checks keep covering it (plan §4.3/§10, G6).
const TRACKED_ZOOM_EVENT_TYPES = ["wheel", "keydown"] as const;
type TrackedZoomEventType = (typeof TRACKED_ZOOM_EVENT_TYPES)[number];

interface TrackedZoomListener {
  type: TrackedZoomEventType;
  handler: EventListenerOrEventListenerObject;
  capture: boolean;
}

const trackedZoomListeners = new Set<TrackedZoomListener>();
let zoomListenerTrackingInstalled = false;

function isTrackedZoomType(type: string): type is TrackedZoomEventType {
  return (TRACKED_ZOOM_EVENT_TYPES as readonly string[]).includes(type);
}

// removeEventListener matches on (type, handler, capture) only; passive/once are
// irrelevant to removal. initZoom attaches wheel with `{ passive: false }` and keydown
// with no options — both capture false — so the scrub detaches with `{ capture }` (plan §4.4).
function normalizeCapture(
  options?: boolean | AddEventListenerOptions | EventListenerOptions,
): boolean {
  return typeof options === "boolean" ? options : options?.capture ?? false;
}

// Plain save-native + assignment swap with an idempotent guard. NOT vi.spyOn: both
// hooks call vi.clearAllMocks(), and a future switch to mockReset/restoreAllMocks/
// { restore: true } could strip a spy-based wrapper mid-file and disable tracking,
// making the registry-empty check pass vacuously (plan §4.4, G7).
function installZoomListenerTracking(): void {
  if (zoomListenerTrackingInstalled) return;
  zoomListenerTrackingInstalled = true;

  const nativeAdd = document.addEventListener.bind(document) as (
    type: string,
    handler: EventListenerOrEventListenerObject | null,
    options?: boolean | AddEventListenerOptions,
  ) => void;
  const nativeRemove = document.removeEventListener.bind(document) as (
    type: string,
    handler: EventListenerOrEventListenerObject | null,
    options?: boolean | EventListenerOptions,
  ) => void;

  const wrappedAdd = (
    type: string,
    handler: EventListenerOrEventListenerObject | null,
    options?: boolean | AddEventListenerOptions,
  ): void => {
    if (handler && isTrackedZoomType(type)) {
      trackedZoomListeners.add({ type, handler, capture: normalizeCapture(options) });
    }
    nativeAdd(type, handler, options);
  };

  // Wrapping remove too lets the normal-path `cleanups` drain unrecord its owned
  // entries, so in non-timeout runs the registry is already empty when the scrub runs
  // and the scrub only ever removes a genuine orphan (plan §4.4).
  const wrappedRemove = (
    type: string,
    handler: EventListenerOrEventListenerObject | null,
    options?: boolean | EventListenerOptions,
  ): void => {
    if (handler && isTrackedZoomType(type)) {
      const capture = normalizeCapture(options);
      for (const entry of Array.from(trackedZoomListeners)) {
        if (entry.type === type && entry.handler === handler && entry.capture === capture) {
          trackedZoomListeners.delete(entry);
        }
      }
    }
    nativeRemove(type, handler, options);
  };

  document.addEventListener = wrappedAdd as unknown as typeof document.addEventListener;
  document.removeEventListener = wrappedRemove as unknown as typeof document.removeEventListener;
}

// SolidJS delegates keydown (solid-js/web DelegatedEvents), storing the delegated set on
// the stable `document["_$DX_DELEGATE"]` key that survives vi.resetModules(). The scrub
// removes only the initZoom handler identities it tracked, never SolidJS's delegated
// handler — collateral-free ONLY while no component in the mounted tree renders onKeyDown.
// Fail loudly if a future change ever violates that invariant (plan §4.3, G2).
function assertNoKeydownDelegation(): void {
  const delegated = (document as unknown as { _$DX_DELEGATE?: Set<string> })._$DX_DELEGATE;
  if (delegated?.has("keydown")) {
    throw new Error(
      'SolidJS keydown delegation is active on document (_$DX_DELEGATE contains "keydown"): a ' +
        "component in the mounted tree now renders onKeyDown, so the zoom-listener scrub can no " +
        "longer assume the only document keydown listener is initZoom's. Revisit plan §4.3 before " +
        "relying on this scrub.",
    );
  }
}

// Boundary scrub: remove every still-tracked wheel/keydown listener by its recorded
// identity (never a blanket "remove all keydown" — plan §4.3), then clear the registry.
function scrubZoomListeners(): void {
  assertNoKeydownDelegation();
  for (const entry of Array.from(trackedZoomListeners)) {
    document.removeEventListener(entry.type, entry.handler, { capture: entry.capture });
  }
  trackedZoomListeners.clear();
}
// -----------------------------------------------------------------------------

const stoppedStatus = (port = 8765): WebServerOwnedStatus => ({
  listening: false,
  owned: false,
  externalListening: false,
  openAllowed: false,
  bind: "127.0.0.1",
  port,
  state: "stopped",
  bindFailure: null,
});

async function loadModules(): Promise<HarnessModules> {
  const [{ default: Titlebar }, testing, fakeTransport, zoom] = await Promise.all([
    import("./Titlebar"),
    import("../../shared/testing/ui-harness"),
    import("../../shared/testing/fake-transport"),
    import("../../shared/zoom"),
  ]);
  return {
    Titlebar,
    FakeTransport: fakeTransport.FakeTransport,
    baseSettings: testing.baseSettings,
    renderWithFakeTransport: testing.renderWithFakeTransport,
    click: testing.click,
    waitFor: testing.waitFor,
    initZoom: zoom.initZoom,
  };
}

async function mountTitlebar(settings: Partial<AppSettings> = {}): Promise<MountedTitlebar> {
  const modules = await loadModules();
  const fake = new modules.FakeTransport();
  let currentSettings = modules.baseSettings(settings);

  fake.onInvoke("get_settings", () => currentSettings);
  fake.onInvoke("update_settings", ({ newSettings }) => {
    currentSettings = newSettings as AppSettings;
  });
  fake.onInvoke("save_settings_draft", ({ draft }) => {
    currentSettings = draft as AppSettings;
  });
  fake.onInvoke("get_web_server_owned_status", () => stoppedStatus(currentSettings.webServerPort));
  fake.onInvoke("start_web_server", () => true);
  fake.onInvoke("stop_web_server", () => true);
  fake.onInvoke("open_web_remote", () => undefined);

  const rendered = modules.renderWithFakeTransport(() => <modules.Titlebar />, fake);
  cleanups.push(rendered.cleanup);

  return {
    fake,
    root: rendered.root,
    modules,
    getSettings: () => currentSettings,
  };
}

function byTestId<T extends Element = Element>(testId: string): T {
  const element = document.querySelector<T>(`[data-ac-testid="${testId}"]`);
  if (!element) throw new Error(`missing selector ${testId}`);
  return element;
}

async function initMainZoom(mounted: MountedTitlebar): Promise<void> {
  const cleanupZoom = await mounted.modules.initZoom("main");
  cleanups.push(cleanupZoom);
}

// Model a test that timed out / was interrupted mid-initZoom: a live document listener
// on the SAME module instance whose returned cleanup is never captured into `cleanups`.
// Only the boundary scrub can remove it. Per plan §2.6 this is equivalent to a real
// 5000ms mid-initZoom timeout for the isolation question.
async function leakInitZoomOrphan(mounted: MountedTitlebar): Promise<void> {
  await mounted.modules.initZoom("main");
}

describe("Titlebar zoom stepper", () => {
  beforeEach(() => {
    // Install the always-on document listener tracking once, before any mount, so every
    // orphan attach is visible whenever its continuation runs (plan §4.1). Idempotent.
    installZoomListenerTracking();
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      configurable: true,
      value: {},
    });
    vi.resetModules();
    vi.clearAllMocks();
    tauriMocks.setZoom.mockResolvedValue(undefined);
    // Belt-and-suspenders re-scrub for anything a prior test's orphan continuation
    // attached in the inter-test gap (plan §4.5).
    scrubZoomListeners();
  });

  afterEach(() => {
    while (cleanups.length > 0) cleanups.pop()?.();
    document.body.innerHTML = "";
    // Deterministically remove any leaked wheel/keydown listener whose cleanup was never
    // captured, independent of whether initZoom's async cleanup ever reached `cleanups`
    // (plan §4.5). The native methods are intentionally NOT restored here (plan §4.1).
    scrubZoomListeners();
    delete (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
    vi.useRealTimers();
    vi.clearAllMocks();
  });

  it("renders between the webserver menu and layout button", async () => {
    await mountTitlebar();

    const controls = document.querySelector(".titlebar-controls");
    expect(controls).toBeTruthy();
    const children = Array.from(controls?.children ?? []);

    expect(children[0]?.querySelector('[data-ac-testid="titlebar.webserver.button"]')).toBeTruthy();
    expect(children[1]?.getAttribute("data-ac-testid")).toBe("titlebar.zoom");
    expect(children[2]?.querySelector('[data-ac-testid="titlebar.layout.button"]')).toBeTruthy();
  });

  it("clicking plus updates the live display and persists main zoom", async () => {
    const mounted = await mountTitlebar({ mainZoom: 1 });
    await initMainZoom(mounted);

    mounted.modules.click(byTestId("titlebar.zoom.in"));

    await mounted.modules.waitFor(() => {
      expect(tauriMocks.setZoom).toHaveBeenLastCalledWith(1.1);
      expect(byTestId("titlebar.zoom.value").textContent).toBe("110%");
    });

    await new Promise((resolve) => setTimeout(resolve, 550));

    await mounted.modules.waitFor(() => {
      expect(mounted.fake.callsFor("update_settings")).toHaveLength(1);
      expect(mounted.fake.lastCall("update_settings")?.args.newSettings).toMatchObject({
        mainZoom: 1.1,
      });
      expect(mounted.getSettings().mainZoom).toBe(1.1);
    });
  });

  it("ctrl plus wheel updates the display live", async () => {
    const mounted = await mountTitlebar({ mainZoom: 1 });
    await initMainZoom(mounted);

    document.dispatchEvent(
      new WheelEvent("wheel", {
        ctrlKey: true,
        deltaY: -1,
        bubbles: true,
        cancelable: true,
      }),
    );

    await mounted.modules.waitFor(() => {
      expect(byTestId("titlebar.zoom.value").textContent).toBe("110%");
    });
  });

  it("meta plus keydown updates the display live", async () => {
    const mounted = await mountTitlebar({ mainZoom: 1 });
    await initMainZoom(mounted);

    document.dispatchEvent(
      new KeyboardEvent("keydown", {
        metaKey: true,
        key: "=",
        bubbles: true,
        cancelable: true,
      }),
    );

    await mounted.modules.waitFor(() => {
      expect(byTestId("titlebar.zoom.value").textContent).toBe("110%");
    });
  });

  it("disables zoom out at the minimum", async () => {
    const mounted = await mountTitlebar({ mainZoom: 0.5 });
    await initMainZoom(mounted);

    await mounted.modules.waitFor(() => {
      expect(byTestId("titlebar.zoom.value").textContent).toBe("50%");
      expect(byTestId<HTMLButtonElement>("titlebar.zoom.out").disabled).toBe(true);
    });
  });

  it("disables zoom in at the maximum", async () => {
    const mounted = await mountTitlebar({ mainZoom: 3.0 });
    await initMainZoom(mounted);

    await mounted.modules.waitFor(() => {
      expect(byTestId("titlebar.zoom.value").textContent).toBe("300%");
      expect(byTestId<HTMLButtonElement>("titlebar.zoom.in").disabled).toBe(true);
    });
  });

  it("catches a rejected zoom apply from an initZoom listener instead of leaking an unhandled rejection (#1083)", async () => {
    const mounted = await mountTitlebar({ mainZoom: 1 });
    await initMainZoom(mounted);

    // Model applyZoom rejecting exactly as the real Tauri webview module does
    // during the flake: the next zoom apply from a listener rejects.
    tauriMocks.setZoom.mockRejectedValue(new Error("boom"));
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});

    document.dispatchEvent(
      new KeyboardEvent("keydown", {
        metaKey: true,
        key: "=",
        bubbles: true,
        cancelable: true,
      }),
    );

    // Let the rejected fire-and-forget apply settle on a macrotask tick.
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(errorSpy).toHaveBeenCalledWith("Failed to change zoom:", expect.any(Error));

    errorSpy.mockRestore();
  });

  // #1093 — within-test deterministic gate (plan §9.1). Instance-agnostic, so it does
  // not depend on cross-test timing. Reproduce the 120% compounding with two
  // same-instance listeners, prove the full teardown scrubs the tracked orphan to an
  // empty registry, then show a freshly re-established single listener reads 110%. With
  // the scrub reduced to a no-op, step 3 reads 120% and the test fails (before/after gate).
  it("scrubs a same-instance leaked zoom listener so a re-init reads 110%, not 120% (#1093)", async () => {
    // Step 1: two live listeners on the SAME module instance (one owned, one orphaned).
    // A single ctrl+wheel compounds 1.0 -> 1.1 -> 1.2 synchronously (plan §2.5).
    const mounted = await mountTitlebar({ mainZoom: 1 });
    await initMainZoom(mounted); // owned listener, cleanup captured
    await leakInitZoomOrphan(mounted); // second listener on the same instance, cleanup dropped

    document.dispatchEvent(
      new WheelEvent("wheel", { ctrlKey: true, deltaY: -1, bubbles: true, cancelable: true }),
    );

    await mounted.modules.waitFor(() => {
      expect(byTestId("titlebar.zoom.value").textContent).toBe("120%");
    });

    // Step 2: full teardown. The drain removes the owned cleanup; the scrub removes the
    // remaining tracked orphan. A full type-scoped scrub leaves ZERO listeners (plan §9.1, G4).
    while (cleanups.length > 0) cleanups.pop()?.();
    document.body.innerHTML = "";
    scrubZoomListeners();
    expect(trackedZoomListeners.size).toBe(0);

    // Step 3: re-establish exactly one owned listener. Its restore path resets currentZoom
    // to 1.0, so a single ctrl+wheel now reads 110% — not a leaked-orphan 120%.
    const remounted = await mountTitlebar({ mainZoom: 1 });
    await initMainZoom(remounted);

    document.dispatchEvent(
      new WheelEvent("wheel", { ctrlKey: true, deltaY: -1, bubbles: true, cancelable: true }),
    );

    await remounted.modules.waitFor(() => {
      expect(byTestId("titlebar.zoom.value").textContent).toBe("110%");
    });
  });

  // #1093 — cross-test boundary regression, part 1 of 2 (plan §9.2): this test leaks an
  // orphan whose cleanup is never captured, modelling a test that timed out mid-initZoom.
  // The boundary scrub must remove it before the sibling test below runs.
  it("leaves a leaked initZoom listener for the boundary scrub to remove (#1093)", async () => {
    const mounted = await mountTitlebar({ mainZoom: 1 });
    await leakInitZoomOrphan(mounted);

    // The orphan is live on the shared document and tracked; afterEach must scrub it.
    expect(trackedZoomListeners.size).toBeGreaterThan(0);
  });

  // #1093 — cross-test boundary regression, part 2 of 2 (plan §9.2). Runs immediately
  // after the leaking test above.
  it("does not carry a leaked initZoom listener into the next test (#1093)", async () => {
    // Deterministic discriminator: the prior test's orphan must have been scrubbed at the
    // boundary. Sound only because the tracking wrapper is always-on (plan §4.1, §9.2).
    expect(trackedZoomListeners.size).toBe(0);

    const mounted = await mountTitlebar({ mainZoom: 1 });
    await initMainZoom(mounted);

    // initMainZoom's restore path already produced a setZoom(1); clear AFTER it so the count
    // reflects only the discriminating dispatch (plan §9.2, G5).
    tauriMocks.setZoom.mockClear();

    document.dispatchEvent(
      new WheelEvent("wheel", { ctrlKey: true, deltaY: -1, bubbles: true, cancelable: true }),
    );

    // Exactly ONE dispatch-driven setZoom when clean; a surviving orphan would add a second
    // on the shared hoisted mock. The extra call lands async after its dynamic import, so
    // assert inside an awaited waitFor (plan §9.2, G5).
    await mounted.modules.waitFor(() => {
      expect(tauriMocks.setZoom).toHaveBeenCalledTimes(1);
    });

    // Sanity only, NOT the discriminator: under module isolation the sibling display reads
    // 110% whether or not a leak occurred, so it cannot distinguish leaked from clean
    // (plan §9.2, §12 E1/P2).
    await mounted.modules.waitFor(() => {
      expect(byTestId("titlebar.zoom.value").textContent).toBe("110%");
    });
  });
});
