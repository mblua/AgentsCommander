// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Component, JSX } from "solid-js";
import type { AppSettings, WebServerOwnedStatus } from "../../shared/types";
import type { FakeTransport } from "../../shared/testing/fake-transport";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve("")),
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    minimize: vi.fn(),
    isMaximized: vi.fn(() => Promise.resolve(false)),
    maximize: vi.fn(),
    unmaximize: vi.fn(),
    close: vi.fn(),
  }),
}));

type HarnessModules = {
  Titlebar: Component;
  FakeTransport: typeof FakeTransport;
  baseSettings: (overrides?: Partial<AppSettings>) => AppSettings;
  renderWithFakeTransport: (
    component: () => JSX.Element,
    fake?: FakeTransport,
  ) => { fake: FakeTransport; root: HTMLDivElement; cleanup: () => void };
  click: (el: Element) => void;
  input: (el: HTMLInputElement, value: string) => void;
  waitFor: (assertion: () => void | Promise<void>, timeoutMs?: number) => Promise<void>;
};

type StatusSource = WebServerOwnedStatus | Error;

interface TransportControl {
  getSettings: () => AppSettings;
  setStatus: (next: WebServerOwnedStatus) => void;
  queueStatus: (...next: StatusSource[]) => void;
}

interface MountOptions {
  settings?: Partial<AppSettings>;
  status?: StatusSource;
  startWebServer?: (control: TransportControl) => boolean | Promise<boolean>;
  stopWebServer?: (control: TransportControl) => boolean | Promise<boolean>;
  openWebRemote?: () => void | Promise<void>;
}

interface MountedTitlebar extends TransportControl {
  fake: FakeTransport;
  root: HTMLDivElement;
  modules: HarnessModules;
}

const cleanups: Array<() => void> = [];

const status = (overrides: Partial<WebServerOwnedStatus> = {}): WebServerOwnedStatus => {
  const owned = overrides.owned ?? false;
  const externalListening = overrides.externalListening ?? false;
  const listening = overrides.listening ?? (owned || externalListening);
  return {
    listening,
    owned,
    externalListening,
    openAllowed: overrides.openAllowed ?? owned,
    bind: overrides.bind ?? "127.0.0.1",
    port: overrides.port ?? 8765,
    state:
      overrides.state ??
      (owned ? "ownedRunning" : externalListening ? "externalListening" : "stopped"),
  };
};

const ownedStatus = (overrides: Partial<WebServerOwnedStatus> = {}) =>
  status({ listening: true, owned: true, openAllowed: true, state: "ownedRunning", ...overrides });

const externalStatus = (overrides: Partial<WebServerOwnedStatus> = {}) =>
  status({
    listening: true,
    owned: false,
    externalListening: true,
    openAllowed: false,
    state: "externalListening",
    ...overrides,
  });

const stoppedStatus = (overrides: Partial<WebServerOwnedStatus> = {}) =>
  status({
    listening: false,
    owned: false,
    externalListening: false,
    openAllowed: false,
    state: "stopped",
    ...overrides,
  });

const deferred = <T,>() => {
  let resolve: (value: T) => void = () => {};
  const promise = new Promise<T>((nextResolve) => {
    resolve = nextResolve;
  });
  return { promise, resolve };
};

async function loadModules(): Promise<HarnessModules> {
  const [{ default: Titlebar }, testing, fakeTransport] = await Promise.all([
    import("./Titlebar"),
    import("../../shared/testing/ui-harness"),
    import("../../shared/testing/fake-transport"),
  ]);
  return {
    Titlebar,
    FakeTransport: fakeTransport.FakeTransport,
    baseSettings: testing.baseSettings,
    renderWithFakeTransport: testing.renderWithFakeTransport,
    click: testing.click,
    input: testing.input,
    waitFor: testing.waitFor,
  };
}

async function mountTitlebar(options: MountOptions = {}): Promise<MountedTitlebar> {
  const modules = await loadModules();
  const fake = new modules.FakeTransport();
  let currentSettings = modules.baseSettings(options.settings);
  let currentStatus = options.status ?? stoppedStatus({ port: currentSettings.webServerPort });
  const statusQueue: StatusSource[] = [];

  const readStatus = () => {
    const next = statusQueue.shift() ?? currentStatus;
    if (next instanceof Error) throw next;
    currentStatus = next;
    return next;
  };

  const control: TransportControl = {
    getSettings: () => currentSettings,
    setStatus: (next) => {
      currentStatus = next;
    },
    queueStatus: (...next) => {
      statusQueue.push(...next);
    },
  };

  fake.onInvoke("get_settings", () => currentSettings);
  fake.onInvoke("save_settings_draft", ({ draft }) => {
    currentSettings = draft as AppSettings;
  });
  fake.onInvoke("get_web_server_owned_status", readStatus);
  fake.onInvoke("start_web_server", () => options.startWebServer?.(control) ?? true);
  fake.onInvoke("stop_web_server", () => options.stopWebServer?.(control) ?? true);
  fake.onInvoke("open_web_remote", () => options.openWebRemote?.());

  const rendered = modules.renderWithFakeTransport(() => <modules.Titlebar />, fake);
  cleanups.push(rendered.cleanup);

  return { fake, root: rendered.root, modules, ...control };
}

function byTestId<T extends Element = Element>(testId: string): T {
  const element = document.querySelector<T>(`[data-ac-testid="${testId}"]`);
  if (!element) throw new Error(`missing selector ${testId}`);
  return element;
}

function maybeByTestId<T extends Element = Element>(testId: string): T | null {
  return document.querySelector<T>(`[data-ac-testid="${testId}"]`);
}

async function openWebServerMenu(mounted: MountedTitlebar): Promise<void> {
  mounted.modules.click(byTestId("titlebar.webserver.button"));
  await mounted.modules.waitFor(() => {
    expect(maybeByTestId("titlebar.webserver.menu")).toBeTruthy();
  });
}

describe("Titlebar webserver menu", () => {
  beforeEach(() => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      configurable: true,
      value: {},
    });
    vi.resetModules();
    vi.clearAllMocks();
  });

  afterEach(() => {
    while (cleanups.length > 0) cleanups.pop()?.();
    document.body.innerHTML = "";
    delete (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
    vi.useRealTimers();
    vi.clearAllMocks();
  });

  it("renders the globe button and opens a menu with owned-running state and port", async () => {
    const mounted = await mountTitlebar({
      settings: { webServerEnabled: true, webServerPort: 9000 },
      status: ownedStatus({ port: 9000 }),
    });

    await openWebServerMenu(mounted);

    expect(byTestId("titlebar.webserver.button").getAttribute("data-ac-state")).toBe("running");
    expect(byTestId("titlebar.webserver.menu").textContent).toContain("Running");
    expect(byTestId("titlebar.webserver.menu").textContent).toContain("9000");
  });

  it("keeps the globe and layout dropdowns mutually exclusive", async () => {
    const mounted = await mountTitlebar();

    mounted.modules.click(byTestId("titlebar.layout.button"));
    await mounted.modules.waitFor(() => {
      expect(maybeByTestId("titlebar.layout.menu")).toBeTruthy();
    });

    mounted.modules.click(byTestId("titlebar.webserver.button"));
    await mounted.modules.waitFor(() => {
      expect(maybeByTestId("titlebar.layout.menu")).toBeNull();
      expect(maybeByTestId("titlebar.webserver.menu")).toBeTruthy();
    });

    mounted.modules.click(byTestId("titlebar.layout.button"));
    await mounted.modules.waitFor(() => {
      expect(maybeByTestId("titlebar.webserver.menu")).toBeNull();
      expect(maybeByTestId("titlebar.layout.menu")).toBeTruthy();
    });
  });

  it("enables Open only for enabled, Rust-owned running status", async () => {
    const mounted = await mountTitlebar({
      settings: { webServerEnabled: true },
      status: ownedStatus(),
    });

    await openWebServerMenu(mounted);
    mounted.modules.click(byTestId("titlebar.webserver.open"));

    await mounted.modules.waitFor(() => {
      expect(mounted.fake.callsFor("open_web_remote")).toHaveLength(1);
    });
  });

  it("disables Open for external-listening status and does not call open_web_remote", async () => {
    const mounted = await mountTitlebar({
      settings: { webServerEnabled: true },
      status: externalStatus(),
    });

    await openWebServerMenu(mounted);
    const open = byTestId<HTMLButtonElement>("titlebar.webserver.open");
    expect(open.disabled).toBe(true);
    expect(byTestId("titlebar.webserver.menu").textContent).toContain("Port is already in use");

    mounted.modules.click(open);
    expect(mounted.fake.callsFor("open_web_remote")).toHaveLength(0);
  });

  it("disables Open when owned-status is unavailable", async () => {
    const mounted = await mountTitlebar({
      settings: { webServerEnabled: true },
      status: new Error("status unavailable"),
    });

    await openWebServerMenu(mounted);
    const open = byTestId<HTMLButtonElement>("titlebar.webserver.open");
    expect(open.disabled).toBe(true);
    expect(byTestId("titlebar.webserver.menu").textContent).toContain(
      "Ownership status unavailable",
    );

    mounted.modules.click(open);
    expect(mounted.fake.callsFor("open_web_remote")).toHaveLength(0);
  });

  it("starts the server, saves enabled state, polls owned status, and shows running", async () => {
    const mounted = await mountTitlebar({
      settings: { webServerEnabled: false },
      status: stoppedStatus(),
      startWebServer: (control) => {
        control.setStatus(ownedStatus());
        return true;
      },
    });

    await openWebServerMenu(mounted);
    mounted.modules.click(byTestId("titlebar.webserver.toggle"));

    await mounted.modules.waitFor(() => {
      expect(mounted.fake.callsFor("save_settings_draft")[0]?.args.draft).toMatchObject({
        webServerEnabled: true,
      });
      expect(mounted.fake.callsFor("start_web_server")).toHaveLength(1);
      expect(byTestId("titlebar.webserver.menu").textContent).toContain("Running");
    });
  });

  it("shows a failed health-check error when start reports success but ownership polling never does", async () => {
    const mounted = await mountTitlebar({
      settings: { webServerEnabled: false },
      status: stoppedStatus(),
      startWebServer: () => true,
    });

    await openWebServerMenu(mounted);
    mounted.modules.click(byTestId("titlebar.webserver.toggle"));

    await mounted.modules.waitFor(() => {
      expect(byTestId("titlebar.webserver.error").textContent).toContain("Server did not start");
      expect(byTestId<HTMLButtonElement>("titlebar.webserver.open").disabled).toBe(true);
    }, 2500);
    expect(mounted.fake.callsFor("open_web_remote")).toHaveLength(0);
  });

  it("surfaces false-start external-listening conflicts and keeps Open disabled", async () => {
    const mounted = await mountTitlebar({
      settings: { webServerEnabled: false },
      status: stoppedStatus(),
      startWebServer: (control) => {
        control.setStatus(externalStatus());
        return false;
      },
    });

    await openWebServerMenu(mounted);
    mounted.modules.click(byTestId("titlebar.webserver.toggle"));

    await mounted.modules.waitFor(() => {
      expect(byTestId("titlebar.webserver.error").textContent).toContain("Port is already in use");
      expect(byTestId("titlebar.webserver.button").getAttribute("data-ac-state")).toBe("ambiguous");
      expect(byTestId<HTMLButtonElement>("titlebar.webserver.open").disabled).toBe(true);
    });
    mounted.modules.click(byTestId("titlebar.webserver.open"));
    expect(mounted.fake.callsFor("open_web_remote")).toHaveLength(0);
  });

  it("surfaces stop-still-listening conflicts and keeps Open disabled", async () => {
    const mounted = await mountTitlebar({
      settings: { webServerEnabled: true },
      status: ownedStatus(),
      stopWebServer: (control) => {
        control.setStatus(externalStatus());
        return true;
      },
    });

    await openWebServerMenu(mounted);
    mounted.modules.click(byTestId("titlebar.webserver.toggle"));

    await mounted.modules.waitFor(() => {
      expect(byTestId("titlebar.webserver.error").textContent).toContain("Port is still in use");
      expect(byTestId<HTMLButtonElement>("titlebar.webserver.open").disabled).toBe(true);
    });
    mounted.modules.click(byTestId("titlebar.webserver.open"));
    expect(mounted.fake.callsFor("open_web_remote")).toHaveLength(0);
  });

  it("stops successfully, saves disabled state, and shows stopped", async () => {
    const mounted = await mountTitlebar({
      settings: { webServerEnabled: true },
      status: ownedStatus(),
      stopWebServer: (control) => {
        control.setStatus(stoppedStatus());
        return true;
      },
    });

    await openWebServerMenu(mounted);
    mounted.modules.click(byTestId("titlebar.webserver.toggle"));

    await mounted.modules.waitFor(() => {
      expect(mounted.fake.callsFor("stop_web_server")).toHaveLength(1);
      expect(mounted.fake.lastCall("save_settings_draft")?.args.draft).toMatchObject({
        webServerEnabled: false,
      });
      expect(byTestId("titlebar.webserver.menu").textContent).toContain("Stopped");
    });
  });

  it("rejects invalid port drafts without saving", async () => {
    const mounted = await mountTitlebar();
    await openWebServerMenu(mounted);
    byTestId<HTMLButtonElement>("titlebar.webserver.editPort").click();
    await mounted.modules.waitFor(() => {
      expect(byTestId<HTMLInputElement>("titlebar.webserver.portInput").hidden).toBe(false);
    });

    for (const value of ["abc", "0", "65536"]) {
      mounted.fake.clearCalls();
      mounted.modules.input(byTestId("titlebar.webserver.portInput"), value);
      mounted.modules.click(byTestId("titlebar.webserver.savePort"));
      await mounted.modules.waitFor(() => {
        expect(byTestId("titlebar.webserver.error").textContent).toContain("Port must be 1 to 65535");
      });
      expect(mounted.fake.callsFor("save_settings_draft")).toHaveLength(0);
    }
  });

  it("saves a valid new port while owned-running and restarts through the shared helper", async () => {
    const mounted = await mountTitlebar({
      settings: { webServerEnabled: true, webServerPort: 8765 },
      status: ownedStatus({ port: 8765 }),
      stopWebServer: (control) => {
        control.setStatus(stoppedStatus({ port: control.getSettings().webServerPort }));
        return true;
      },
      startWebServer: (control) => {
        control.setStatus(ownedStatus({ port: control.getSettings().webServerPort }));
        return true;
      },
    });

    await openWebServerMenu(mounted);
    byTestId<HTMLButtonElement>("titlebar.webserver.editPort").click();
    await mounted.modules.waitFor(() => {
      expect(byTestId<HTMLInputElement>("titlebar.webserver.portInput").hidden).toBe(false);
    });
    mounted.modules.input(byTestId("titlebar.webserver.portInput"), "9000");
    mounted.modules.click(byTestId("titlebar.webserver.savePort"));

    await mounted.modules.waitFor(() => {
      expect(mounted.fake.callsFor("save_settings_draft")[0]?.args.draft).toMatchObject({
        webServerPort: 9000,
      });
      expect(mounted.fake.callsFor("stop_web_server")).toHaveLength(1);
      expect(mounted.fake.callsFor("start_web_server")).toHaveLength(1);
      expect(byTestId<HTMLInputElement>("titlebar.webserver.portInput").hidden).toBe(true);
    });
  });

  it("opens only when running, enabled, and Rust-owned", async () => {
    const stopped = await mountTitlebar({
      settings: { webServerEnabled: true },
      status: stoppedStatus(),
    });
    await openWebServerMenu(stopped);
    stopped.modules.click(byTestId("titlebar.webserver.open"));
    expect(stopped.fake.callsFor("open_web_remote")).toHaveLength(0);
    cleanups.pop()?.();
    document.body.innerHTML = "";

    const disabled = await mountTitlebar({
      settings: { webServerEnabled: false },
      status: ownedStatus(),
    });
    await openWebServerMenu(disabled);
    disabled.modules.click(byTestId("titlebar.webserver.open"));
    expect(disabled.fake.callsFor("open_web_remote")).toHaveLength(0);
    cleanups.pop()?.();
    document.body.innerHTML = "";

    const enabled = await mountTitlebar({
      settings: { webServerEnabled: true },
      status: ownedStatus(),
    });
    await openWebServerMenu(enabled);
    enabled.modules.click(byTestId("titlebar.webserver.open"));
    await enabled.modules.waitFor(() => {
      expect(enabled.fake.callsFor("open_web_remote")).toHaveLength(1);
    });
  });

  it("guards double-click start while the first start is pending", async () => {
    const start = deferred<boolean>();
    const mounted = await mountTitlebar({
      settings: { webServerEnabled: false },
      status: stoppedStatus(),
      startWebServer: async (control) => {
        const result = await start.promise;
        control.setStatus(ownedStatus());
        return result;
      },
    });

    await openWebServerMenu(mounted);
    const toggle = byTestId("titlebar.webserver.toggle");
    mounted.modules.click(toggle);
    mounted.modules.click(toggle);

    await mounted.modules.waitFor(() => {
      expect(mounted.fake.callsFor("save_settings_draft")).toHaveLength(1);
      expect(mounted.fake.callsFor("start_web_server")).toHaveLength(1);
    });

    start.resolve(true);
    await mounted.modules.waitFor(() => {
      expect(byTestId("titlebar.webserver.menu").textContent).toContain("Running");
    });
  });

  it("guards double-click restart while the first restart is pending", async () => {
    const stop = deferred<boolean>();
    const mounted = await mountTitlebar({
      settings: { webServerEnabled: true },
      status: ownedStatus(),
      stopWebServer: async (control) => {
        const result = await stop.promise;
        control.setStatus(stoppedStatus());
        return result;
      },
      startWebServer: (control) => {
        control.setStatus(ownedStatus());
        return true;
      },
    });

    await openWebServerMenu(mounted);
    const restart = byTestId("titlebar.webserver.restart");
    mounted.modules.click(restart);
    mounted.modules.click(restart);

    await mounted.modules.waitFor(() => {
      expect(mounted.fake.callsFor("stop_web_server")).toHaveLength(1);
      expect(mounted.fake.callsFor("start_web_server")).toHaveLength(0);
    });

    stop.resolve(true);
    await mounted.modules.waitFor(() => {
      expect(mounted.fake.callsFor("start_web_server")).toHaveLength(1);
    });
  });
});
