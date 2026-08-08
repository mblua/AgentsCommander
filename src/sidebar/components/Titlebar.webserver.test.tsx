// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Component, JSX } from "solid-js";
import type {
  AppSettings,
  IsolatedPackageTitlebarIdentityResponse,
  Session,
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
interface SnakeCaseIsolatedPackageTitlebarIdentityResponse {
  readonly mode: "isolated";
  readonly workgroup: string;
  readonly agent: string;
  readonly workspace: string;
  readonly header_identity: string;
}

type TitlebarIdentityPayload =
  | IsolatedPackageTitlebarIdentityResponse
  | SnakeCaseIsolatedPackageTitlebarIdentityResponse;
type TitlebarIdentitySource =
  | TitlebarIdentityPayload
  | Error
  | Promise<TitlebarIdentityPayload>;

interface TransportControl {
  getSettings: () => AppSettings;
  setStatus: (next: WebServerOwnedStatus) => void;
  queueStatus: (...next: StatusSource[]) => void;
}

interface MountOptions {
  settings?: Partial<AppSettings>;
  status?: StatusSource;
  titlebarIdentity?: TitlebarIdentitySource;
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

const titlebarSession = (overrides: Partial<Session> = {}): Session => ({
  id: "titlebar-session",
  name: "Terminal",
  shell: "powershell.exe",
  shellArgs: [],
  effectiveShellArgs: null,
  createdAt: "2026-08-08T00:00:00Z",
  workingDirectory: "C:\\work\\AgentsCommander\\.ac\\wg-1271-normal\\__agent_normal-agent",
  status: "idle",
  waitingForInput: false,
  communication: null,
  pendingReview: false,
  lastPrompt: null,
  agentId: null,
  agentLabel: null,
  gitRepos: [],
  workgroupTask: null,
  isCoordinator: false,
  isRootAgent: false,
  token: "",
  agentKind: null,
  requestedProfile: null,
  effectiveProfile: null,
  profileFallbackChain: [],
  profileFallbackApplied: false,
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
  const titlebarIdentity: TitlebarIdentitySource = options.titlebarIdentity ?? { mode: "normal" };
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
  fake.onInvoke("get_isolated_package_titlebar_identity", () => {
    if (titlebarIdentity instanceof Error) throw titlebarIdentity;
    return titlebarIdentity;
  });

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
    }, 2500);
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
});

describe("Titlebar isolated package identity", () => {
  const isolatedIdentity: IsolatedPackageTitlebarIdentityResponse = {
    mode: "isolated",
    workgroup: "WG-1271-ISOLATED-GATES",
    agent: "gate-tester",
    workspace: "AgentsCommander_1271_isolated",
    headerIdentity: "WG-1271-ISOLATED-GATES gate-tester@AgentsCommander_1271_isolated",
  };

  it("renders no terminal-derived identity while the bridge is pending", async () => {
    const response = deferred<IsolatedPackageTitlebarIdentityResponse>();
    const mounted = await mountTitlebar({ titlebarIdentity: response.promise });

    expect(mounted.root.querySelector("[data-isolation-titlebar-state]")?.getAttribute(
      "data-isolation-titlebar-state",
    )).toBe("pending");
    expect(mounted.root.textContent).not.toContain("Terminal");

    response.resolve(isolatedIdentity);
    await mounted.modules.waitFor(() => {
      expect(mounted.root.querySelector("[data-isolation-titlebar-state]")?.getAttribute(
        "data-isolation-titlebar-state",
      )).toBe("isolated");
    });
  });

  it("renders the fixed isolated identity before any terminal session exists", async () => {
    const mounted = await mountTitlebar({ titlebarIdentity: isolatedIdentity });

    await mounted.modules.waitFor(() => {
      expect(mounted.root.querySelector("[data-isolation-titlebar-state]")?.getAttribute(
        "data-isolation-titlebar-state",
      )).toBe("isolated");
    });

    expect(mounted.root.textContent).toContain("WG-1271-ISOLATED-GATES");
    expect(mounted.root.textContent).toContain("gate-tester@AgentsCommander_1271_isolated");
    expect(mounted.fake.callsFor("get_isolated_package_titlebar_identity")).toHaveLength(1);
  });

  it("retains normal terminal-derived identity rendering", async () => {
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const { terminalStore } = await import("../../terminal/stores/terminal");
    const session = titlebarSession();
    terminalStore.bindLockedSession(session);

    try {
      const mounted = await mountTitlebar({ titlebarIdentity: { mode: "normal" } });

      await mounted.modules.waitFor(() => {
        expect(mounted.root.querySelector("[data-isolation-titlebar-state]")?.getAttribute(
          "data-isolation-titlebar-state",
        )).toBe("normal");
      });

      expect(mounted.root.textContent).toContain("WG-1271-NORMAL");
      expect(mounted.root.textContent).toContain("normal-agent@AgentsCommander");
      expect(errorSpy).not.toHaveBeenCalled();
    } finally {
      terminalStore.resetForTests();
      errorSpy.mockRestore();
    }
  });

  it("renders the deterministic error instead of a Terminal fallback", async () => {
    const secret = "token=very-secret-value root=C:\\sensitive-root";
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const mounted = await mountTitlebar({
      titlebarIdentity: new Error(secret),
    });

    try {
      await mounted.modules.waitFor(() => {
        expect(mounted.root.querySelector("[data-isolation-titlebar-state]")?.getAttribute(
          "data-isolation-titlebar-state",
        )).toBe("error");
      });

      expect(mounted.root.textContent).toContain("ISOLATION IDENTITY UNAVAILABLE");
      expect(mounted.root.textContent).not.toContain("Terminal");
      expect(errorSpy).toHaveBeenCalledWith(
        "isolated titlebar identity bridge failed: ipc-failure",
      );
      expect(errorSpy.mock.calls.flat().map(String).join(" ")).not.toContain(secret);
    } finally {
      errorSpy.mockRestore();
    }
  });

  it("rejects a snake_case bridge field as unavailable", async () => {
    const secret = "secret-header-identity";
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const mounted = await mountTitlebar({
      titlebarIdentity: {
        mode: "isolated",
        workgroup: "WG-1271-ISOLATED-GATES",
        agent: "gate-tester",
        workspace: "AgentsCommander_1271_isolated",
        header_identity: secret,
      },
    });

    try {
      await mounted.modules.waitFor(() => {
        expect(mounted.root.querySelector("[data-isolation-titlebar-state]")?.getAttribute(
          "data-isolation-titlebar-state",
        )).toBe("error");
      });

      expect(mounted.root.textContent).toContain("ISOLATION IDENTITY UNAVAILABLE");
      expect(errorSpy).toHaveBeenCalledWith(
        "isolated titlebar identity bridge failed: invalid-response",
      );
      expect(errorSpy.mock.calls.flat().map(String).join(" ")).not.toContain(secret);
    } finally {
      errorSpy.mockRestore();
    }
  });

  it("does not let hostile terminal root, marker, or session values influence isolated text", async () => {
    const { terminalStore } = await import("../../terminal/stores/terminal");
    terminalStore.bindLockedSession(titlebarSession({
      name: "hostile-session-name",
      workingDirectory: "C:\\hostile-root\\hostile-marker\\.ac\\wg-999\\__agent_untrusted",
    }));

    try {
      const mounted = await mountTitlebar({ titlebarIdentity: isolatedIdentity });

      await mounted.modules.waitFor(() => {
        expect(mounted.root.querySelector("[data-isolation-titlebar-state]")?.getAttribute(
          "data-isolation-titlebar-state",
        )).toBe("isolated");
      });

      expect(mounted.root.textContent).toContain("WG-1271-ISOLATED-GATES");
      expect(mounted.root.textContent).toContain("gate-tester@AgentsCommander_1271_isolated");
      expect(mounted.root.textContent).not.toContain("hostile-root");
      expect(mounted.root.textContent).not.toContain("hostile-marker");
      expect(mounted.root.textContent).not.toContain("hostile-session-name");
      expect(mounted.root.textContent).not.toContain("WG-999");
      expect(mounted.root.textContent).not.toContain("untrusted");
    } finally {
      terminalStore.resetForTests();
    }
  });
});
