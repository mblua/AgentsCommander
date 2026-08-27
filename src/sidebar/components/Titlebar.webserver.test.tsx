// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Component, JSX } from "solid-js";
import type {
  AppSettings,
  WebServerInterfaceInfo,
  WebServerOwnedStatus,
} from "../../shared/types";
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
  getSavedDrafts: () => readonly AppSettings[];
  setStatus: (next: WebServerOwnedStatus) => void;
  queueStatus: (...next: StatusSource[]) => void;
}

interface MountOptions {
  settings?: Partial<AppSettings>;
  status?: StatusSource;
  interfaces?: WebServerInterfaceInfo[];
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
  const state =
    overrides.state ??
    (overrides.owned
      ? "ownedRunning"
      : overrides.externalListening
        ? "externalListening"
        : "stopped");
  const owned = overrides.owned ?? ["starting", "ownedRunning", "stopping"].includes(state);
  const externalListening = overrides.externalListening ?? state === "externalListening";
  const listening =
    overrides.listening ?? ["ownedRunning", "externalListening", "stopping"].includes(state);
  return {
    listening,
    owned,
    externalListening,
    openAllowed: overrides.openAllowed ?? state === "ownedRunning",
    bind: overrides.bind ?? "127.0.0.1",
    port: overrides.port ?? 8765,
    state,
    bindFailure: overrides.bindFailure ?? null,
  };
};

// #1453 - the machine of the issue, trimmed to one physical and one virtual
// adapter: enough to exercise both chooser groups.
const DEFAULT_INTERFACES: WebServerInterfaceInfo[] = [
  { address: "192.168.1.9", interfaceName: "Ethernet", isVirtual: false },
  { address: "100.121.138.61", interfaceName: "Tailscale", isVirtual: true },
];

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

const startingStatus = (overrides: Partial<WebServerOwnedStatus> = {}) =>
  status({
    listening: false,
    owned: true,
    externalListening: false,
    openAllowed: false,
    state: "starting",
    ...overrides,
  });

const stoppingStatus = (overrides: Partial<WebServerOwnedStatus> = {}) =>
  status({
    listening: true,
    owned: true,
    externalListening: false,
    openAllowed: false,
    state: "stopping",
    ...overrides,
  });

const deferred = <T,>() => {
  let resolve: (value: T) => void = () => {};
  let reject: (reason?: unknown) => void = () => {};
  const promise = new Promise<T>((nextResolve, nextReject) => {
    resolve = nextResolve;
    reject = nextReject;
  });
  return { promise, resolve, reject };
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
  const savedDrafts: AppSettings[] = [];

  const readStatus = () => {
    const next = statusQueue.shift() ?? currentStatus;
    if (next instanceof Error) throw next;
    currentStatus = next;
    return next;
  };

  const control: TransportControl = {
    getSettings: () => currentSettings,
    getSavedDrafts: () => savedDrafts,
    setStatus: (next) => {
      currentStatus = next;
    },
    queueStatus: (...next) => {
      statusQueue.push(...next);
    },
  };

  fake.onInvoke("get_settings", () => currentSettings);
  fake.onInvoke("save_settings_draft", ({ draft }) => {
    const next = { ...(draft as AppSettings) };
    savedDrafts.push(next);
    currentSettings = next;
  });
  fake.onInvoke("get_web_server_owned_status", readStatus);
  fake.onInvoke("start_web_server", () => options.startWebServer?.(control) ?? true);
  fake.onInvoke("stop_web_server", () => options.stopWebServer?.(control) ?? true);
  fake.onInvoke("open_web_remote", () => options.openWebRemote?.());
  fake.onInvoke("list_web_server_interfaces", () => options.interfaces ?? DEFAULT_INTERFACES);

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

// #1453 - chooser rows all share one testid and are told apart by their address.
const ADDR_SELECTOR = '[data-ac-testid="titlebar.webserver.addrOption"]';

function maybeByAddr(address: string): HTMLButtonElement | null {
  return document.querySelector<HTMLButtonElement>(`${ADDR_SELECTOR}[data-addr="${address}"]`);
}

function byAddr(address: string): HTMLButtonElement {
  const element = maybeByAddr(address);
  if (!element) throw new Error(`missing bind option ${address}`);
  return element;
}

// #1453 - the popover binds Escape with `on:keydown` (a real element listener)
// rather than the delegated `onKeyDown`, so this has to bubble from the element
// the user is actually focused on.
function pressEscape(el: Element): void {
  el.dispatchEvent(
    new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }),
  );
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

  it("displays local browser destinations without changing the configured bind", async () => {
    const cases = [
      ["0.0.0.0", "http://127.0.0.1:8888"],
      ["::", "http://[::1]:8888"],
      ["127.0.0.1", "http://127.0.0.1:8888"],
      ["192.168.1.50", "http://192.168.1.50:8888"],
      ["::1", "http://[::1]:8888"],
      ["2001:db8::25", "http://[2001:db8::25]:8888"],
      ["0:0:0:0:0:0:0:0", "http://[::1]:8888"],
      ["0::0", "http://[::1]:8888"],
      ["2001:0DB8::0025", "http://[2001:0DB8::0025]:8888"],
      ["::\n", "http://[::\n]:8888"],
    ] as const;

    for (const [bind, browserUrl] of cases) {
      const mounted = await mountTitlebar({
        settings: { webServerBind: bind, webServerPort: 8888 },
      });

      await openWebServerMenu(mounted);

      expect(document.querySelector(".webserver-url")?.textContent).toBe(browserUrl);
      expect(document.querySelector(".webserver-bind-value[title]")?.textContent).toBe(bind);

      cleanups.pop()?.();
      document.body.innerHTML = "";
    }
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

  it("treats external listening after owner release as terminal and persists disabled", async () => {
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
      expect(mounted.getSavedDrafts()).toHaveLength(1);
      expect(mounted.getSavedDrafts()[0]).toMatchObject({ webServerEnabled: false });
      expect(mounted.getSettings().webServerEnabled).toBe(false);
      expect(byTestId("titlebar.webserver.menu").textContent).toContain("Port in use");
      expect(byTestId<HTMLButtonElement>("titlebar.webserver.open").disabled).toBe(true);
    });
    expect(maybeByTestId("titlebar.webserver.error")).toBeNull();
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

  it("persists disabled after a successful Stop exceeds the old polling budget", async () => {
    const stop = deferred<boolean>();
    const mounted = await mountTitlebar({
      settings: { webServerEnabled: true },
      status: ownedStatus(),
      stopWebServer: (control) => {
        control.setStatus(stoppingStatus());
        return stop.promise;
      },
    });

    await openWebServerMenu(mounted);
    await mounted.modules.waitFor(() => {
      expect(mounted.fake.callsFor("get_web_server_owned_status").length).toBeGreaterThanOrEqual(2);
    });
    mounted.fake.clearCalls();
    vi.useFakeTimers();
    mounted.modules.click(byTestId("titlebar.webserver.toggle"));

    await vi.advanceTimersByTimeAsync(1_600);
    expect(mounted.fake.callsFor("get_web_server_owned_status").length).toBeGreaterThan(15);
    expect(mounted.getSavedDrafts()).toHaveLength(0);

    mounted.setStatus(stoppedStatus());
    stop.resolve(true);
    await vi.advanceTimersByTimeAsync(200);
    for (let i = 0; i < 8; i += 1) await Promise.resolve();

    expect(mounted.getSavedDrafts()).toHaveLength(1);
    expect(mounted.getSavedDrafts()[0]).toMatchObject({ webServerEnabled: false });
    expect(mounted.getSettings().webServerEnabled).toBe(false);
    expect(byTestId("titlebar.webserver.menu").textContent).toContain("Stopped");
    expect(maybeByTestId("titlebar.webserver.error")).toBeNull();
  });

  it("persists enabled after a successful Start exceeds the old polling budget", async () => {
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
    await mounted.modules.waitFor(() => {
      expect(mounted.fake.callsFor("get_web_server_owned_status").length).toBeGreaterThanOrEqual(2);
    });
    mounted.fake.clearCalls();
    vi.useFakeTimers();
    mounted.modules.click(byTestId("titlebar.webserver.toggle"));

    await vi.advanceTimersByTimeAsync(1_600);
    expect(mounted.fake.callsFor("get_web_server_owned_status").length).toBeGreaterThan(15);
    expect(mounted.getSavedDrafts()).toHaveLength(0);

    start.resolve(true);
    await vi.advanceTimersByTimeAsync(200);
    for (let i = 0; i < 8; i += 1) await Promise.resolve();

    expect(mounted.getSavedDrafts()).toHaveLength(1);
    expect(mounted.getSavedDrafts()[0]).toMatchObject({ webServerEnabled: true });
    expect(mounted.getSettings().webServerEnabled).toBe(true);
    expect(byTestId("titlebar.webserver.menu").textContent).toContain("Running");
    expect(maybeByTestId("titlebar.webserver.error")).toBeNull();
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
      expect(mounted.fake.callsFor("start_web_server")).toHaveLength(1);
    });
    // #1453 - the intent flag is now written only once the start converged, so
    // while the first start is still pending nothing has been persisted yet.
    expect(mounted.fake.callsFor("save_settings_draft")).toHaveLength(0);

    start.resolve(true);
    await mounted.modules.waitFor(() => {
      expect(mounted.fake.callsFor("save_settings_draft")).toHaveLength(1);
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
      expect(byTestId("titlebar.webserver.menu").textContent).toContain("Running");
    });
  });

  it("waits through transient external-listening status before restarting", async () => {
    const mounted = await mountTitlebar({
      settings: { webServerEnabled: true },
      status: ownedStatus(),
      stopWebServer: (control) => {
        control.queueStatus(externalStatus(), stoppedStatus());
        return true;
      },
      startWebServer: (control) => {
        control.setStatus(ownedStatus());
        return true;
      },
    });

    await openWebServerMenu(mounted);
    mounted.modules.click(byTestId("titlebar.webserver.restart"));

    await mounted.modules.waitFor(() => {
      expect(mounted.fake.callsFor("start_web_server")).toHaveLength(1);
      expect(byTestId("titlebar.webserver.menu").textContent).toContain("Running");
    });
    expect(maybeByTestId("titlebar.webserver.error")).toBeNull();
  });

  it("start_invoke_pending_exposes_starting_and_allows_stop", async () => {
    const start = deferred<boolean>();
    const stop = deferred<boolean>();
    const mounted = await mountTitlebar({
      settings: { webServerEnabled: false },
      status: stoppedStatus(),
      startWebServer: (control) => {
        control.queueStatus(startingStatus());
        return start.promise;
      },
      stopWebServer: (control) => {
        control.queueStatus(stoppingStatus(), stoppedStatus());
        return stop.promise;
      },
    });

    await openWebServerMenu(mounted);
    mounted.modules.click(byTestId("titlebar.webserver.toggle"));

    await mounted.modules.waitFor(() => {
      expect(byTestId("titlebar.webserver.menu").textContent).toContain("Starting");
      expect(byTestId("titlebar.webserver.button").getAttribute("data-ac-state")).toBe(
        "ambiguous",
      );
      const toggle = byTestId<HTMLButtonElement>("titlebar.webserver.toggle");
      expect(toggle.textContent).toContain("Stop Server");
      expect(toggle.disabled).toBe(false);
      expect(byTestId<HTMLButtonElement>("titlebar.webserver.editAddr").disabled).toBe(true);
      expect(byTestId<HTMLButtonElement>("titlebar.webserver.editPort").disabled).toBe(true);
      expect(byTestId<HTMLButtonElement>("titlebar.webserver.restart").disabled).toBe(true);
      expect(byTestId<HTMLButtonElement>("titlebar.webserver.open").disabled).toBe(true);
    });
    expect(mounted.getSavedDrafts()).toHaveLength(0);

    mounted.modules.click(byTestId("titlebar.webserver.toggle"));
    await mounted.modules.waitFor(() => {
      expect(mounted.fake.callsFor("stop_web_server")).toHaveLength(1);
    });
    expect(mounted.fake.callsFor("start_web_server")).toHaveLength(1);
    expect(mounted.getSavedDrafts()).toHaveLength(0);

    stop.resolve(true);
    start.reject(new Error("Web server start cancelled by stop"));
    await mounted.modules.waitFor(() => {
      expect(mounted.getSavedDrafts()).toHaveLength(1);
      expect(mounted.getSavedDrafts()[0]).toMatchObject({ webServerEnabled: false });
      expect(mounted.getSavedDrafts().filter((draft) => draft.webServerEnabled)).toHaveLength(0);
      expect(mounted.getSettings().webServerEnabled).toBe(false);
      expect(byTestId("titlebar.webserver.menu").textContent).toContain("Stopped");
    });
  });

  it("start_wins_but_late_start_continuation_cannot_override_completed_stop", async () => {
    const start = deferred<boolean>();
    const mounted = await mountTitlebar({
      settings: { webServerEnabled: false },
      status: stoppedStatus(),
      startWebServer: (control) => {
        control.queueStatus(startingStatus(), ownedStatus());
        return start.promise;
      },
      stopWebServer: (control) => {
        control.queueStatus(stoppingStatus(), stoppedStatus());
        return true;
      },
    });

    await openWebServerMenu(mounted);
    mounted.modules.click(byTestId("titlebar.webserver.toggle"));
    await mounted.modules.waitFor(() => {
      expect(byTestId("titlebar.webserver.menu").textContent).toContain("Running");
      const toggle = byTestId<HTMLButtonElement>("titlebar.webserver.toggle");
      expect(toggle.textContent).toContain("Stop Server");
      expect(toggle.disabled).toBe(false);
    });
    expect(mounted.getSavedDrafts()).toHaveLength(0);

    mounted.modules.click(byTestId("titlebar.webserver.toggle"));
    await mounted.modules.waitFor(() => {
      expect(mounted.fake.callsFor("stop_web_server")).toHaveLength(1);
      expect(mounted.getSavedDrafts()).toHaveLength(1);
      expect(mounted.getSavedDrafts()[0]).toMatchObject({ webServerEnabled: false });
      expect(mounted.getSettings().webServerEnabled).toBe(false);
      expect(byTestId("titlebar.webserver.menu").textContent).toContain("Stopped");
    });

    start.resolve(true);
    await Promise.resolve();
    await Promise.resolve();
    await mounted.modules.waitFor(() => {
      expect(mounted.getSavedDrafts().filter((draft) => draft.webServerEnabled)).toHaveLength(0);
      expect(mounted.getSettings().webServerEnabled).toBe(false);
      expect(byTestId("titlebar.webserver.menu").textContent).toContain("Stopped");
      expect(byTestId("titlebar.webserver.menu").textContent).not.toContain("Running");
      expect(maybeByTestId("titlebar.webserver.error")).toBeNull();
    });
  });

  it("stop_invoke_pending_exposes_stopping_before_persistence", async () => {
    const stop = deferred<boolean>();
    const mounted = await mountTitlebar({
      settings: { webServerEnabled: true },
      status: ownedStatus(),
      stopWebServer: (control) => {
        control.queueStatus(stoppingStatus());
        return stop.promise;
      },
    });

    await openWebServerMenu(mounted);
    mounted.modules.click(byTestId("titlebar.webserver.toggle"));
    await mounted.modules.waitFor(() => {
      expect(byTestId("titlebar.webserver.menu").textContent).toContain("Stopping");
      expect(byTestId("titlebar.webserver.button").getAttribute("data-ac-state")).toBe(
        "ambiguous",
      );
      const toggle = byTestId<HTMLButtonElement>("titlebar.webserver.toggle");
      expect(toggle.textContent).toContain("Stop Server");
      expect(toggle.disabled).toBe(false);
      expect(byTestId<HTMLButtonElement>("titlebar.webserver.editAddr").disabled).toBe(true);
      expect(byTestId<HTMLButtonElement>("titlebar.webserver.editPort").disabled).toBe(true);
      expect(byTestId<HTMLButtonElement>("titlebar.webserver.restart").disabled).toBe(true);
      expect(byTestId<HTMLButtonElement>("titlebar.webserver.open").disabled).toBe(true);
    });
    expect(mounted.getSavedDrafts()).toHaveLength(0);

    mounted.queueStatus(stoppedStatus());
    stop.resolve(true);
    await mounted.modules.waitFor(() => {
      expect(mounted.getSavedDrafts()).toHaveLength(1);
      expect(mounted.getSavedDrafts()[0]).toMatchObject({ webServerEnabled: false });
      expect(byTestId("titlebar.webserver.menu").textContent).toContain("Stopped");
    });
  });

  it("keeps stopping fail-closed after timeout and lets a later Stop join", async () => {
    const mounted = await mountTitlebar({
      settings: { webServerEnabled: true },
      status: stoppingStatus(),
      stopWebServer: () => Promise.reject(new Error("Timed out waiting for web server stop")),
    });

    await openWebServerMenu(mounted);
    const toggle = byTestId<HTMLButtonElement>("titlebar.webserver.toggle");
    expect(toggle.textContent).toContain("Stop Server");
    expect(toggle.disabled).toBe(false);
    expect(byTestId<HTMLButtonElement>("titlebar.webserver.restart").disabled).toBe(true);
    expect(byTestId<HTMLButtonElement>("titlebar.webserver.open").disabled).toBe(true);

    mounted.modules.click(toggle);
    await mounted.modules.waitFor(() => {
      expect(byTestId("titlebar.webserver.error").textContent).toContain(
        "Timed out waiting for web server stop",
      );
    }, 2500);
    expect(mounted.getSavedDrafts()).toHaveLength(0);
    expect(mounted.fake.callsFor("start_web_server")).toHaveLength(0);

    mounted.modules.click(toggle);
    await mounted.modules.waitFor(() => {
      expect(mounted.fake.callsFor("stop_web_server")).toHaveLength(2);
    });
    await mounted.modules.waitFor(() => {
      expect(byTestId("titlebar.webserver.error").textContent).toContain(
        "Timed out waiting for web server stop",
      );
    }, 2500);
    expect(mounted.getSavedDrafts()).toHaveLength(0);
    expect(mounted.fake.callsFor("start_web_server")).toHaveLength(0);
  });

  it("an active port edit saves, waits through Stopping, then starts", async () => {
    const stop = deferred<boolean>();
    const mounted = await mountTitlebar({
      settings: { webServerEnabled: true, webServerPort: 8765 },
      status: ownedStatus({ port: 8765 }),
      stopWebServer: (control) => {
        control.queueStatus(stoppingStatus({ port: 8765 }));
        return stop.promise;
      },
      startWebServer: (control) => {
        const port = control.getSettings().webServerPort;
        control.queueStatus(startingStatus({ port }), ownedStatus({ port }));
        return true;
      },
    });

    await openWebServerMenu(mounted);
    const edit = byTestId<HTMLButtonElement>("titlebar.webserver.editPort");
    expect(edit.disabled).toBe(false);
    mounted.modules.click(edit);
    await mounted.modules.waitFor(() => {
      expect(byTestId<HTMLInputElement>("titlebar.webserver.portInput").hidden).toBe(false);
    });
    mounted.modules.input(byTestId("titlebar.webserver.portInput"), "9001");
    const save = byTestId<HTMLButtonElement>("titlebar.webserver.savePort");
    expect(save.disabled).toBe(false);
    mounted.modules.click(save);

    await mounted.modules.waitFor(() => {
      expect(mounted.getSavedDrafts()).toHaveLength(1);
      expect(mounted.getSavedDrafts()[0]).toMatchObject({ webServerPort: 9001 });
      expect(mounted.fake.callsFor("stop_web_server")).toHaveLength(1);
      expect(byTestId("titlebar.webserver.menu").textContent).toContain("Stopping");
    });
    expect(mounted.fake.callsFor("start_web_server")).toHaveLength(0);
    expect(byTestId<HTMLInputElement>("titlebar.webserver.portInput").hidden).toBe(false);

    mounted.queueStatus(stoppedStatus({ port: 9001 }));
    stop.resolve(true);
    await mounted.modules.waitFor(() => {
      expect(mounted.fake.callsFor("start_web_server")).toHaveLength(1);
      expect(byTestId("titlebar.webserver.menu").textContent).toContain("Running");
      expect(byTestId<HTMLInputElement>("titlebar.webserver.portInput").hidden).toBe(true);
    });
  });

  it("an active bind edit never starts or collapses when Stop fails in Stopping", async () => {
    const mounted = await mountTitlebar({
      settings: { webServerEnabled: true, webServerBind: "127.0.0.1" },
      status: ownedStatus({ bind: "127.0.0.1" }),
      stopWebServer: (control) => {
        control.queueStatus(stoppedStatus({ bind: "192.168.1.9" }));
        return Promise.reject(new Error("Stop failed before terminal confirmation"));
      },
    });

    await openWebServerMenu(mounted);
    const edit = byTestId<HTMLButtonElement>("titlebar.webserver.editAddr");
    expect(edit.disabled).toBe(false);
    mounted.modules.click(edit);
    await mounted.modules.waitFor(() => {
      expect(maybeByTestId("titlebar.webserver.bindPanel")).toBeTruthy();
    });
    const option = byAddr("192.168.1.9");
    expect(option.disabled).toBe(false);
    mounted.modules.click(option);

    await mounted.modules.waitFor(() => {
      expect(mounted.getSavedDrafts()).toHaveLength(1);
      expect(mounted.getSavedDrafts()[0]).toMatchObject({ webServerBind: "192.168.1.9" });
      expect(mounted.fake.callsFor("stop_web_server")).toHaveLength(1);
      expect(byTestId("titlebar.webserver.error").textContent).toContain(
        "Stop failed before terminal confirmation",
      );
    });
    expect(mounted.fake.callsFor("start_web_server")).toHaveLength(0);
    expect(maybeByTestId("titlebar.webserver.bindPanel")).toBeTruthy();
  });

  // ---------------------------------------------------------------------------
  // #1453 - bind address chooser and bind-failure surfacing.
  // ---------------------------------------------------------------------------

  const BIND_FAILURE = {
    bind: "192.168.1.12",
    port: 8888,
    detail: "The requested address is not valid in its context. (os error 10049)",
  };

  it("bind failure renders failed status, alert and runtime toggle", async () => {
    const mounted = await mountTitlebar({
      settings: { webServerEnabled: true, webServerBind: "192.168.1.12", webServerPort: 8888 },
      status: stoppedStatus({ bind: "192.168.1.12", port: 8888, bindFailure: BIND_FAILURE }),
      startWebServer: (control) => {
        control.setStatus(ownedStatus({ bind: "192.168.1.12", port: 8888 }));
        return true;
      },
    });

    await openWebServerMenu(mounted);

    expect(byTestId("titlebar.webserver.menu").textContent).toContain("Stopped · bind failed");
    expect(byTestId("titlebar.webserver.button").getAttribute("data-ac-state")).toBe("ambiguous");

    const alert = byTestId("titlebar.webserver.bindAlert");
    expect(alert.textContent).toContain("Address no longer on this machine");
    expect(alert.textContent).toContain(BIND_FAILURE.detail);

    const toggle = byTestId("titlebar.webserver.toggle");
    expect(toggle.textContent).toContain("Start Server");

    // The behavioural half of the gate: the JSX label and the handleToggle
    // guard are two separate expressions, so only invoking the toggle proves a
    // half-applied fix did not ship.
    mounted.modules.click(toggle);
    await mounted.modules.waitFor(() => {
      expect(mounted.fake.callsFor("start_web_server")).toHaveLength(1);
      expect(byTestId("titlebar.webserver.menu").textContent).toContain("Running");
    });
    expect(mounted.fake.callsFor("stop_web_server")).toHaveLength(0);
  });

  it("picking an address while enabled and stopped saves bind and starts", async () => {
    const enabled = await mountTitlebar({
      settings: { webServerEnabled: true, webServerBind: "192.168.1.12", webServerPort: 8888 },
      status: stoppedStatus({ bind: "192.168.1.12", port: 8888, bindFailure: BIND_FAILURE }),
      startWebServer: (control) => {
        control.setStatus(ownedStatus({ bind: control.getSettings().webServerBind, port: 8888 }));
        return true;
      },
    });

    await openWebServerMenu(enabled);
    enabled.modules.click(byTestId("titlebar.webserver.editAddr"));
    await enabled.modules.waitFor(() => {
      expect(maybeByTestId("titlebar.webserver.bindPanel")).toBeTruthy();
    });
    enabled.modules.click(byAddr("0.0.0.0"));

    await enabled.modules.waitFor(() => {
      expect(enabled.fake.lastCall("save_settings_draft")?.args.draft).toMatchObject({
        webServerBind: "0.0.0.0",
      });
      expect(enabled.fake.callsFor("start_web_server")).toHaveLength(1);
      expect(byTestId("titlebar.webserver.menu").textContent).toContain("Running");
      expect(maybeByTestId("titlebar.webserver.bindPanel")).toBeNull();
    });
    cleanups.pop()?.();
    document.body.innerHTML = "";

    // Same click with the intent flag off only saves: changing the address of a
    // server that is meant to stay off must not turn it on.
    const disabled = await mountTitlebar({
      settings: { webServerEnabled: false, webServerBind: "192.168.1.12", webServerPort: 8888 },
      status: stoppedStatus({ bind: "192.168.1.12", port: 8888 }),
    });

    await openWebServerMenu(disabled);
    disabled.modules.click(byTestId("titlebar.webserver.editAddr"));
    await disabled.modules.waitFor(() => {
      expect(maybeByTestId("titlebar.webserver.bindPanel")).toBeTruthy();
    });
    disabled.modules.click(byAddr("0.0.0.0"));

    await disabled.modules.waitFor(() => {
      expect(disabled.fake.lastCall("save_settings_draft")?.args.draft).toMatchObject({
        webServerBind: "0.0.0.0",
      });
      expect(maybeByTestId("titlebar.webserver.bindPanel")).toBeNull();
    });
    expect(disabled.fake.callsFor("start_web_server")).toHaveLength(0);
  });

  it("manual entry validates IPv4 shape and warns on undetected", async () => {
    const mounted = await mountTitlebar({
      settings: { webServerEnabled: false, webServerBind: "127.0.0.1" },
      status: stoppedStatus(),
    });

    await openWebServerMenu(mounted);
    mounted.modules.click(byTestId("titlebar.webserver.editAddr"));
    await mounted.modules.waitFor(() => {
      expect(maybeByTestId("titlebar.webserver.bindPanel")).toBeTruthy();
    });

    mounted.modules.input(byTestId("titlebar.webserver.addrInput"), "192.168.1.300");
    await mounted.modules.waitFor(() => {
      expect(byTestId<HTMLButtonElement>("titlebar.webserver.addrUse").disabled).toBe(true);
      expect(byTestId("titlebar.webserver.bindPanel").textContent).toContain(
        "Not a valid IPv4 address.",
      );
    });

    mounted.modules.input(byTestId("titlebar.webserver.addrInput"), "192.168.1.50");
    await mounted.modules.waitFor(() => {
      expect(byTestId<HTMLButtonElement>("titlebar.webserver.addrUse").disabled).toBe(false);
      expect(byTestId("titlebar.webserver.bindPanel").textContent).toContain(
        "Not detected on this machine. The bind may fail.",
      );
    });
  });

  it("stored missing address renders disabled unavailable row", async () => {
    const mounted = await mountTitlebar({
      settings: { webServerEnabled: true, webServerBind: "192.168.1.12", webServerPort: 8888 },
      status: stoppedStatus({ bind: "192.168.1.12", port: 8888, bindFailure: BIND_FAILURE }),
    });

    await openWebServerMenu(mounted);
    mounted.modules.click(byTestId("titlebar.webserver.editAddr"));
    await mounted.modules.waitFor(() => {
      expect(maybeByTestId("titlebar.webserver.storedRow")).toBeTruthy();
    });

    const stored = byTestId<HTMLButtonElement>("titlebar.webserver.storedRow");
    expect(stored.disabled).toBe(true);
    expect(stored.textContent).toContain("Unavailable · not on this machine");

    // Virtual adapters start collapsed here, because the stored bind is not one
    // of them, and the count is the affordance that says what is hidden.
    const virtualToggle = byTestId("titlebar.webserver.virtualToggle");
    expect(virtualToggle.textContent).toContain("Virtual & tunnel");
    expect(virtualToggle.textContent).toContain("(1)");
    expect(maybeByAddr("100.121.138.61")).toBeNull();

    mounted.modules.click(virtualToggle);
    await mounted.modules.waitFor(() => {
      expect(maybeByAddr("100.121.138.61")).toBeTruthy();
    });
  });

  it("toggle label follows runtime state, not the persisted enable flag", async () => {
    const mounted = await mountTitlebar({
      settings: { webServerEnabled: true },
      status: stoppedStatus(),
    });

    await openWebServerMenu(mounted);

    expect(maybeByTestId("titlebar.webserver.bindAlert")).toBeNull();
    expect(byTestId("titlebar.webserver.toggle").textContent).toContain("Start Server");
  });

  it("empty interface list makes no availability claim", async () => {
    const mounted = await mountTitlebar({
      settings: { webServerEnabled: true, webServerBind: "192.168.1.12", webServerPort: 8888 },
      status: stoppedStatus({ bind: "192.168.1.12", port: 8888, bindFailure: BIND_FAILURE }),
      interfaces: [],
    });

    await openWebServerMenu(mounted);

    // Fetch succeeded with zero rows: that is absence of evidence, so the
    // headline falls back to the generic flavour and nothing claims the address
    // is missing.
    const alert = byTestId("titlebar.webserver.bindAlert");
    expect(alert.textContent).toContain("Could not start the web server");
    expect(alert.textContent).not.toContain("Address no longer on this machine");

    mounted.modules.click(byTestId("titlebar.webserver.editAddr"));
    await mounted.modules.waitFor(() => {
      expect(maybeByTestId("titlebar.webserver.bindPanel")).toBeTruthy();
    });
    expect(byTestId("titlebar.webserver.bindPanel").textContent).not.toContain(
      "Unavailable · not on this machine",
    );

    mounted.modules.input(byTestId("titlebar.webserver.addrInput"), "192.168.1.50");
    await mounted.modules.waitFor(() => {
      expect(byTestId("titlebar.webserver.bindPanel").textContent).toContain(
        "Valid IPv4 address.",
      );
    });
    expect(byTestId("titlebar.webserver.bindPanel").textContent).not.toContain(
      "Not detected on this machine.",
    );
  });

  it("escape collapses the chooser first and only then closes the popover", async () => {
    const mounted = await mountTitlebar({
      settings: { webServerEnabled: false, webServerBind: "127.0.0.1" },
      status: stoppedStatus(),
    });

    await openWebServerMenu(mounted);
    mounted.modules.click(byTestId("titlebar.webserver.editAddr"));
    await mounted.modules.waitFor(() => {
      expect(maybeByTestId("titlebar.webserver.bindPanel")).toBeTruthy();
    });

    // From inside the manual input, which is the furthest a Tab walk gets and
    // the reason D13 asked for this handler at all.
    pressEscape(byTestId("titlebar.webserver.addrInput"));
    await mounted.modules.waitFor(() => {
      expect(maybeByTestId("titlebar.webserver.bindPanel")).toBeNull();
      expect(maybeByTestId("titlebar.webserver.menu")).toBeTruthy();
    });

    pressEscape(byTestId("titlebar.webserver.menu"));
    await mounted.modules.waitFor(() => {
      expect(maybeByTestId("titlebar.webserver.menu")).toBeNull();
    });
  });
});
