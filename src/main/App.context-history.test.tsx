// @vitest-environment jsdom
//
// #3532 P07 - the context history is hosted in the central pane next to the
// always-mounted terminal and sidebar, desktop only, for the sidebar's
// selected project and without any fallback project.
import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const h = vi.hoisted(() => ({
  isTauri: true,
  activeKey: null as string | null,
  projects: [] as Array<{ path: string; workgroups: []; agents: []; teams: [] }>,
  mounts: [] as string[],
  unmounts: [] as string[],
  terminalMounts: 0,
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    label: "main",
    onCloseRequested: async () => () => {},
    onMoved: async () => () => {},
    onResized: async () => () => {},
    setAlwaysOnTop: async () => undefined,
    isMaximized: async () => false,
    isFullscreen: async () => false,
    isMinimized: async () => false,
    outerPosition: async () => ({ x: 0, y: 0 }),
    outerSize: async () => ({ width: 1200, height: 800 }),
  }),
}));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  WebviewWindow: { getAll: async () => [], getByLabel: async () => null },
  getCurrentWebviewWindow: () => ({ label: "main" }),
}));
vi.mock("../shared/platform", () => ({
  get isTauri() {
    return h.isTauri;
  },
  get isBrowser() {
    return !h.isTauri;
  },
  isWindows: false,
}));
vi.mock("../shared/zoom", () => ({ initZoom: async () => () => {} }));
vi.mock("../sidebar/watchdog/non-stop-watchdog-client", () => ({
  startNonStopWatchdogClient: () => undefined,
}));
vi.mock("./listeners-home", () => ({ wireHomeListeners: async () => [] }));
vi.mock("./listeners-central-view", () => ({ wireCentralViewListeners: async () => [] }));
vi.mock("../sidebar/App", () => ({ default: () => null }));
vi.mock("../terminal/App", async () => {
  const { onMount } = await import("solid-js");
  return {
    default: () => {
      onMount(() => {
        h.terminalMounts += 1;
      });
      return <div data-testid="terminal" />;
    },
  };
});
vi.mock("../resource-monitor/App", () => ({ default: () => null }));
vi.mock("../sidebar/components/Titlebar", () => ({ default: () => null }));
vi.mock("./components/ErrorModal", () => ({ default: () => null }));
vi.mock("../shared/components/ExternalLinkConfirm", () => ({ default: () => null }));
// Only the selection inputs are overridden; every other member stays real.
function override<T extends object>(target: T, key: string, value: () => unknown): T {
  return new Proxy(target, {
    get: (object, prop) => (prop === key ? value() : Reflect.get(object, prop, object)),
  });
}
vi.mock("../sidebar/stores/project", async (original) => {
  const real = await original<{ projectStore: object }>();
  return { ...real, projectStore: override(real.projectStore, "projects", () => h.projects) };
});
vi.mock("../sidebar/stores/workgroup-groups", async (original) => {
  const real = await original<{ workgroupGroupsStore: object }>();
  const activeProjectPath = () => () => h.activeKey;
  return {
    ...real,
    workgroupGroupsStore: override(real.workgroupGroupsStore, "activeProjectPath", activeProjectPath),
  };
});
vi.mock("../context-history/App", async () => {
  const { onCleanup } = await import("solid-js");
  return {
    default: (props: { projectPath: string; onClose: () => void }) => {
      h.mounts.push(props.projectPath);
      onCleanup(() => h.unmounts.push(props.projectPath));
      return (
        <button data-testid="history-close" onClick={() => props.onClose()}>
          {props.projectPath}
        </button>
      );
    },
  };
});

import MainApp, { selectedHistoryProject } from "./App";
import { __setTransportForTests } from "../shared/ipc";
import { FakeTransport } from "../shared/testing/fake-transport";
import { centralViewStore, __resetCentralViewStoreForTests } from "./stores/centralView";

const PROJECT = "C:\\Work\\Proj";
const proj = (path: string) => ({ path, workgroups: [] as [], agents: [] as [], teams: [] as [] });

async function flush(): Promise<void> {
  for (let pass = 0; pass < 64; pass += 1) await Promise.resolve();
  await new Promise((resolve) => setTimeout(resolve, 0));
}

let fake: FakeTransport;
let restoreTransport: () => void;
let cleanup: (() => void) | null = null;

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

const pane = () => document.querySelector('[data-ac-testid="main.contextHistoryPane"]');
const historyCalls = () => fake.calls.filter((call) => call.cmd.startsWith("get_context_history"));

describe("MainApp context history host (#3532 P07)", () => {
  beforeEach(() => {
    h.isTauri = true;
    h.activeKey = null;
    h.projects = [];
    h.mounts = [];
    h.unmounts = [];
    h.terminalMounts = 0;
    __resetCentralViewStoreForTests();
    fake = new FakeTransport();
    restoreTransport = __setTransportForTests(fake);
    fake.resolve("get_settings", {
      themeLight: false,
      mainSidebarWidth: 360,
      mainSidebarSide: "right",
      mainResourceMonitorAttached: false,
      mainAlwaysOnTop: false,
    });
    fake.resolve("set_main_window_placement", undefined);
    fake.resolve("set_main_resource_monitor_attached", undefined);
    vi.spyOn(console, "error").mockImplementation(() => {});
    vi.spyOn(console, "warn").mockImplementation(() => {});
  });

  afterEach(() => {
    cleanup?.();
    cleanup = null;
    restoreTransport();
    vi.restoreAllMocks();
  });

  it("resolves the registered spelling of the selected project, never a fallback", () => {
    expect(selectedHistoryProject()).toBeNull();
    h.activeKey = "c:/work/proj";
    expect(selectedHistoryProject()).toBeNull();
    h.projects = [proj("C:\\Other"), proj(PROJECT)];
    expect(selectedHistoryProject()).toBe(PROJECT);
  });

  it("mounts the history over the kept terminal and closes back to it", async () => {
    h.activeKey = "c:/work/proj";
    h.projects = [proj(PROJECT)];
    const root = await mountMain();
    expect(pane()).toBeNull();
    centralViewStore.showContextHistory();
    await flush();
    expect(pane()).not.toBeNull();
    expect(h.mounts).toEqual([PROJECT]);
    expect(root.querySelector('[data-testid="terminal"]')).not.toBeNull();
    (root.querySelector('[data-testid="history-close"]') as HTMLElement).click();
    await flush();
    expect(centralViewStore.view).toBe("terminal");
    expect(pane()).toBeNull();
    expect(h.unmounts).toEqual([PROJECT]);
    expect(h.terminalMounts).toBe(1);
    expect(fake.calls.some((call) => call.cmd === "switch_session")).toBe(false);
  });

  it("shows the choose-a-project message without a project", async () => {
    await mountMain();
    centralViewStore.showContextHistory();
    await flush();
    expect(pane()?.textContent).toBe("Elegí un proyecto para consultar su historial.");
    expect(h.mounts).toEqual([]);
  });

  it("never mounts the consumer nor invokes the family outside Tauri", async () => {
    h.isTauri = false;
    h.activeKey = "c:/work/proj";
    h.projects = [proj(PROJECT)];
    await mountMain();
    centralViewStore.showContextHistory();
    await flush();
    expect(pane()).toBeNull();
    expect(h.mounts).toEqual([]);
    expect(historyCalls()).toEqual([]);
  });
});
