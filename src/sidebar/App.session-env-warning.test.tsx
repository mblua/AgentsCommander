// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import SidebarApp from "./App";
import { TOAST_EXIT_MS, toastStore } from "../shared/stores/toasts";
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
import { liveSelection, SESSION_A } from "../shared/testing/session-selection";

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
      id: SESSION_A,
      name: "General",
      workingDirectory: agentPath,
      status: "active",
    }),
  ]);
  fake.resolve("get_active_session", liveSelection(SESSION_A));
  fake.resolve("list_detached_sessions", []);
  fake.resolve("telegram_list_bridges", []);
  fake.resolve("drain_session_warnings", []);
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
        sessionId: SESSION_A,
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

  it("drains buffered session warnings on mount", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const message =
      "container session: no CLAUDE_CONFIG_DIR is set. Set CLAUDE_CONFIG_DIR=%AC_REPLICA_ROOT%\\.claude.";
    fake.resolve("drain_session_warnings", [
      {
        sessionId: SESSION_A,
        key: "CLAUDE_CONFIG_DIR",
        kind: "no-value",
        message,
      },
    ]);

    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() => {
        expect(fake.lastCall("drain_session_warnings")?.args).toEqual({
          sessionId: null,
        });
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

  it("dedupes a live warning that is also returned by the initial drain", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const warning = {
      sessionId: SESSION_A,
      key: "CONTAINER_TRANSPORT_PROTOCOL",
      kind: "protocol-mismatch" as const,
      message: "Container image protocol is old. Rebuild the image.",
    };
    fake.onInvoke("drain_session_warnings", () => {
      fake.emitFromBackend("session_env_warning", warning);
      return [warning];
    });

    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() => {
        expect(toastStore.items).toHaveLength(1);
        expect(toastStore.items[0]).toMatchObject({
          kind: "error",
          message: warning.message,
          exiting: false,
        });
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("keeps a self-handoff failure visible until dismissed in the same session", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const warning = {
      sessionId: SESSION_A,
      key: "self-handoff",
      kind: "self-handoff-failed" as const,
      message:
        `self-switch attempt attempt-live for session ${SESSION_A} failed: session locked. ` +
        "Unlock the session or use reviewed --force, then retry.",
    };
    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() => expect(rendered.root.textContent).toContain("General"));
      vi.useFakeTimers();
      fake.emitFromBackend("session_env_warning", warning);
      await Promise.resolve();
      const alert = document.body.querySelector('[role="alert"]');
      expect(alert).not.toBeNull();
      expect(alert?.textContent).toContain(warning.message);

      await vi.advanceTimersByTimeAsync(5000);
      await Promise.resolve();
      expect(document.body.contains(alert)).toBe(true);
      expect(alert?.textContent).toContain(warning.message);
      expect(alert?.classList.contains("toast-item--exiting")).toBe(false);
      const dismiss = alert?.querySelector<HTMLButtonElement>(
        '[data-ac-testid="toast.item.dismiss"]'
      );
      expect(dismiss).not.toBeNull();
      dismiss!.click();
      await vi.advanceTimersByTimeAsync(TOAST_EXIT_MS);
      await Promise.resolve();
      expect(document.body.contains(alert)).toBe(false);
      expect(rendered.root.textContent).toContain("General");
      expect(fake.callsFor("set_active_session")).toHaveLength(0);
    } finally {
      rendered.cleanup();
      toastStore.clear();
      vi.useRealTimers();
    }
  });

  it("renders a buffered self-handoff failure on mount", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const warning = {
      sessionId: SESSION_A,
      key: "self-handoff",
      kind: "self-handoff-failed" as const,
      message:
        `self-restart attempt attempt-buffered for session ${SESSION_A} failed: session locked. ` +
        "Unlock the session or use reviewed --force, then retry.",
    };
    fake.resolve("drain_session_warnings", [warning]);
    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() => {
        const alert = document.body.querySelector('[role="alert"]');
        expect(alert).not.toBeNull();
        expect(alert?.textContent).toContain(warning.message);
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("renders one self-handoff failure for live and deferred initial drain overlap", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const warning = {
      sessionId: SESSION_A,
      key: "self-handoff",
      kind: "self-handoff-failed" as const,
      message:
        `self-switch attempt attempt-overlap for session ${SESSION_A} failed: session locked. ` +
        "Unlock the session or use reviewed --force, then retry.",
    };
    let finishDrain!: (warnings: typeof warning[]) => void;
    fake.onInvoke("drain_session_warnings", () => new Promise<typeof warning[]>((resolve) => {
      finishDrain = resolve;
    }));
    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() => expect(finishDrain).toBeTypeOf("function"));
      fake.emitFromBackend("session_env_warning", warning);
      finishDrain([warning]);
      await waitFor(() => {
        const alerts = [...document.body.querySelectorAll('[role="alert"]')];
        expect(alerts).toHaveLength(1);
        expect(alerts[0].textContent).toContain(warning.message);
      });
    } finally {
      rendered.cleanup();
    }
  });
});
