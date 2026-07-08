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

const stoppedStatus = (port = 8765): WebServerOwnedStatus => ({
  listening: false,
  owned: false,
  externalListening: false,
  openAllowed: false,
  bind: "127.0.0.1",
  port,
  state: "stopped",
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

describe("Titlebar zoom stepper", () => {
  beforeEach(() => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      configurable: true,
      value: {},
    });
    vi.resetModules();
    vi.clearAllMocks();
    tauriMocks.setZoom.mockResolvedValue(undefined);
  });

  afterEach(() => {
    while (cleanups.length > 0) cleanups.pop()?.();
    document.body.innerHTML = "";
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
});
