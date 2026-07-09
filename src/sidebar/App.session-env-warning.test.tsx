// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import SidebarApp from "./App";
import { toastStore } from "../shared/stores/toasts";
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

const projectPath = "C:\\Project";
const agentPath = `${projectPath}\\.ac\\_agent_General`;

function setupTransport(fake: FakeTransport): void {
  fake.resolve("get_settings", baseSettings({ projectPaths: [projectPath], projectPath }));
  fake.resolve("open_project", {
    path: projectPath,
    registered: true,
    created: false,
  });
  fake.resolve(
    "discover_project",
    discovery({
      agents: [
        {
          name: "General",
          path: agentPath,
          roleExists: true,
        },
      ],
      teams: [],
      workgroups: [],
    })
  );
  fake.resolve("get_project_groups", { groups: [], showAll: true, showUngrouped: true });
  fake.resolve("search_repos", []);
  fake.resolve("list_sessions", [
    session({
      id: "session-1",
      name: "General",
      workingDirectory: agentPath,
      status: "active",
    }),
  ]);
  fake.resolve("get_active_session", "session-1");
  fake.resolve("list_detached_sessions", []);
  fake.resolve("telegram_list_bridges", []);
}

describe("SidebarApp session environment warnings", () => {
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

  it("surfaces session_env_warning messages as sticky error toasts", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);

    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() => expect(rendered.root.textContent).toContain("General"));

      const message =
        "container session: CLAUDE_CONFIG_DIR='/workspace/.claude' looks like a container path. Set CLAUDE_CONFIG_DIR=%AC_REPLICA_ROOT%\\.claude.";
      fake.emitFromBackend("session_env_warning", {
        sessionId: "session-1",
        key: "CLAUDE_CONFIG_DIR",
        kind: "container-path-in-host-field",
        message,
      });

      await waitFor(() => {
        expect(toastStore.items).toHaveLength(1);
        expect(toastStore.items[0]).toMatchObject({
          kind: "error",
          message,
          exiting: false,
        });
      });
    } finally {
      rendered.cleanup();
    }
  });
});
