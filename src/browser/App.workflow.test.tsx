// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import BrowserApp from "./App";
import { FakeTransport } from "../shared/testing/fake-transport";
import {
  baseSettings,
  discovery,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../shared/testing/ui-harness";

vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    cols = 80;
    rows = 24;
    element: HTMLElement | null = null;
    loadAddon(): void {}
    open(element: HTMLElement): void {
      this.element = element;
    }
    focus(): void {}
    dispose(): void {}
    write(): void {}
    scrollToBottom(): void {}
    paste(): void {}
    hasSelection(): boolean {
      return false;
    }
    getSelection(): string {
      return "";
    }
    attachCustomKeyEventHandler(): void {}
    onData(): { dispose: () => void } {
      return { dispose: () => {} };
    }
    onResize(): { dispose: () => void } {
      return { dispose: () => {} };
    }
  },
}));

vi.mock("@xterm/addon-fit", () => ({
  FitAddon: class {
    fit = vi.fn();
  },
}));

vi.mock("@xterm/addon-webgl", () => ({
  WebglAddon: class {
    onContextLoss = vi.fn();
    dispose = vi.fn();
  },
}));

vi.mock("@xterm/xterm/css/xterm.css", () => ({}));

const projectPath = "C:\\Project";
const architectSession = session({
  id: "session-architect",
  name: "wg-1-dev-team/architect",
  workingDirectory: `${projectPath}\\.ac\\wg-1-dev-team\\__agent_architect`,
  isCoordinator: true,
});

function browserDiscovery() {
  return discovery({
    teams: [
      {
        name: "dev-team",
        agents: ["architect", "dev-webpage-ui"],
        coordinator: "architect",
      },
    ],
    workgroups: [
      {
        name: "wg-1-dev-team",
        path: `${projectPath}\\.ac\\wg-1-dev-team`,
        task: null,
        taskTitle: "Browser UI tests",
        teamName: "dev-team",
        agents: [
          {
            name: "architect",
            path: architectSession.workingDirectory,
            repoPaths: [],
            isCoordinator: true,
          },
          {
            name: "dev-webpage-ui",
            path: `${projectPath}\\.ac\\wg-1-dev-team\\__agent_dev-webpage-ui`,
            repoPaths: [],
            isCoordinator: false,
          },
        ],
      },
    ],
  });
}

function setupBrowserTransport(fake: FakeTransport): void {
  fake.resolve(
    "get_settings",
    baseSettings({
      projectPaths: [projectPath],
      projectPath,
      telegramBots: [],
    })
  );
  fake.resolve("open_project", {
    path: projectPath,
    registered: true,
    created: false,
  });
  fake.resolve("discover_project", browserDiscovery());
  fake.resolve("search_repos", []);
  fake.resolve("list_sessions", [architectSession]);
  fake.resolve("get_active_session", architectSession.id);
  fake.resolve("list_detached_sessions", []);
  fake.resolve("telegram_list_bridges", []);
  fake.resolve("pty_resize", undefined);
}

describe("BrowserApp workflow", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
  });

  it("loads sidebar and terminal panes from fake transport", async () => {
    const fake = new FakeTransport();
    setupBrowserTransport(fake);

    const rendered = renderWithFakeTransport(() => <BrowserApp />, fake);
    try {
      await waitFor(() => {
        expect(rendered.root.querySelector(".browser-layout")).not.toBeNull();
        expect(rendered.root.querySelector(".browser-sidebar")).not.toBeNull();
        expect(rendered.root.querySelector(".browser-terminal")).not.toBeNull();
        expect(rendered.root.textContent).toContain("wg-1-dev-team");
        expect(rendered.root.textContent).toContain("architect");
      });

      expect(fake.callsFor("get_settings").length).toBeGreaterThan(0);
      expect(fake.lastCall("open_project")?.args).toEqual({ path: projectPath });
      expect(fake.lastCall("discover_project")?.args).toEqual({ path: projectPath });
      expect(fake.callsFor("list_sessions").length).toBeGreaterThan(0);
      expect(fake.callsFor("get_active_session").length).toBeGreaterThan(0);
      expect(fake.callsFor("list_detached_sessions").length).toBeGreaterThan(0);
    } finally {
      rendered.cleanup();
    }
  });

  it("updates sidebar width when the browser divider is dragged", async () => {
    const fake = new FakeTransport();
    setupBrowserTransport(fake);

    const rendered = renderWithFakeTransport(() => <BrowserApp />, fake);
    try {
      await waitFor(() =>
        expect(rendered.root.querySelector(".browser-divider")).not.toBeNull()
      );
      await waitFor(() => {
        expect(fake.callsFor("telegram_list_bridges").length).toBeGreaterThan(0);
        expect(fake.callsFor("get_active_session").length).toBeGreaterThan(0);
      });

      const sidebar = rendered.root.querySelector(".browser-sidebar") as HTMLElement;
      const divider = rendered.root.querySelector(".browser-divider") as HTMLElement;
      expect(sidebar.style.width).toBe("300px");

      divider.dispatchEvent(
        new MouseEvent("mousedown", { bubbles: true, cancelable: true })
      );
      document.dispatchEvent(
        new MouseEvent("mousemove", {
          bubbles: true,
          cancelable: true,
          clientX: 480,
        })
      );
      document.dispatchEvent(
        new MouseEvent("mouseup", { bubbles: true, cancelable: true })
      );

      expect(sidebar.style.width).toBe("480px");
    } finally {
      rendered.cleanup();
    }
  });
});
