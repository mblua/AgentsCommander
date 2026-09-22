// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import SidebarApp from "./App";
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
import { liveSelection, SESSION_A, SESSION_B } from "../shared/testing/session-selection";
import { sessionsStore } from "./stores/sessions";
import type { Session } from "../shared/types";

// #2271 phase 8 - the idle edge is the only place the Co-managed decision can
// reach the dot without painting `waiting` first: lib.rs emits `session_idle`
// synchronously and App.tsx turns it straight into setSessionWaiting. This suite
// mounts the real SidebarApp and drives the real handlers over FakeTransport; it
// never calls the store into the state it asserts on. The ordering assertion
// inspects the captured calls synchronously after the emit: under the harness,
// waitFor is not a wait.

const projectPath = "C:\\Project";
const agentAPath = `${projectPath}\\.ac\\_agent_General`;
const agentBPath = `${projectPath}\\.ac\\_agent_Worker`;

function backendRows(): Session[] {
  return [
    session({
      id: SESSION_A,
      name: "General",
      workingDirectory: agentAPath,
      status: "running",
      waitingForInput: false,
    }),
    session({
      id: SESSION_B,
      name: "Worker",
      workingDirectory: agentBPath,
      status: "running",
      waitingForInput: false,
    }),
  ];
}

function setupTransport(fake: FakeTransport): void {
  fake.resolve(
    "get_settings",
    baseSettings({ projectPaths: [projectPath], projectPath }),
  );
  fake.resolve("open_project", { path: projectPath, registered: true, created: false });
  fake.resolve(
    "discover_project",
    discovery({
      agents: [
        { name: "General", path: agentAPath, roleExists: true },
        { name: "Worker", path: agentBPath, roleExists: true },
      ],
      teams: [],
      workgroups: [],
    }),
  );
  fake.resolve("get_project_groups", { groups: [], showAll: true, showUngrouped: true });
  fake.resolve("search_repos", []);
  fake.onInvoke("list_sessions", () => backendRows());
  fake.resolve("get_active_session", liveSelection(SESSION_A));
  fake.resolve("list_detached_sessions", []);
  fake.resolve("telegram_list_bridges", []);
}

function dot(root: HTMLElement, id: string): HTMLElement {
  const row = root.querySelector(`[data-ac-testid="session.${id}"]`);
  if (!row) throw new Error(`row ${id} not rendered`);
  const el = row.querySelector(".session-item-status");
  if (!el) throw new Error(`status dot missing for ${id}`);
  return el as HTMLElement;
}

function currentRow(id: string): Session | undefined {
  return sessionsStore.sessions.find((s) => s.id === id);
}

describe("SidebarApp Co-managed idle edge (#2271)", () => {
  let cleanupDom: (() => void) | null = null;
  let clearReconcileIntervals: () => void;
  let restoreIntervalSpy: () => void;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    sessionsStore.resetComanagedForTests();

    // Keep the real setInterval so the app's timers still run, then sweep the
    // 5000 ms reconcile handles in afterEach. Mocking the implementation would
    // hide a leak instead of exposing it.
    const intervalSpy = vi.spyOn(globalThis, "setInterval");
    clearReconcileIntervals = () => {
      intervalSpy.mock.calls.forEach((call, i) => {
        if (call[1] !== 5000) return;
        const handle = intervalSpy.mock.results[i]?.value as
          | ReturnType<typeof setInterval>
          | undefined;
        if (handle !== undefined) clearInterval(handle);
      });
    };
    restoreIntervalSpy = () => intervalSpy.mockRestore();
  });

  afterEach(() => {
    // The sweep MUST precede the restore: mockRestore() discards mock.calls and
    // mock.results and the handles become unrecoverable.
    clearReconcileIntervals();
    restoreIntervalSpy();
    cleanupDom?.();
    cleanupDom = null;
    sessionsStore.resetComanagedForTests();
    resetUiStoresForTests();
  });

  it("the idle edge arms comanaged and never paints waiting (test 8)", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const waitingSpy = vi.spyOn(sessionsStore, "setSessionWaiting");
    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() =>
        expect(fake.listensFor("session_idle").length).toBeGreaterThan(0),
      );

      const observedClasses: string[] = [dot(rendered.root, SESSION_B).className];

      fake.emitFromBackend("session_idle", { id: SESSION_B, comanaged: true });

      // Ordering assertion, not a wait: the handler has already run.
      observedClasses.push(dot(rendered.root, SESSION_B).className);

      expect(sessionsStore.comanagedBySessionId[SESSION_B]).toBe(true);
      expect(currentRow(SESSION_B)?.waitingForInput).toBe(false);
      expect(currentRow(SESSION_B)?.pendingReview).toBe(false);
      expect(
        waitingSpy.mock.calls.filter(([id, waiting]) => id === SESSION_B && waiting === true),
      ).toEqual([]);
      expect(observedClasses.some((c) => c.includes("waiting") || c.includes("pending"))).toBe(false);

      const renderedDot = dot(rendered.root, SESSION_B);
      expect(renderedDot.classList.contains("comanaged")).toBe(true);
      expect(renderedDot.getAttribute("data-ac-comanaged")).toBe("true");
      expect(renderedDot.getAttribute("title")).toMatch(/co-managed/i);
    } finally {
      waitingSpy.mockRestore();
      rendered.cleanup();
    }
  });

  it("treats an older { id } payload as not comanaged (test 16)", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() =>
        expect(fake.listensFor("session_idle").length).toBeGreaterThan(0),
      );

      // No `comanaged` key: the additive contract must not throw and must mean false.
      fake.emitFromBackend("session_idle", { id: SESSION_B });

      expect(sessionsStore.comanagedBySessionId[SESSION_B] ?? false).toBe(false);
      expect(currentRow(SESSION_B)?.waitingForInput).toBe(true);
      // The ordinary path still ran, so the old behaviour is untouched: B is not
      // the active row, so the idle edge also raises pendingReview.
      expect(dot(rendered.root, SESSION_B).classList.contains("pending")).toBe(true);

      const renderedDot = dot(rendered.root, SESSION_B);
      expect(renderedDot.classList.contains("comanaged")).toBe(false);
      expect(renderedDot.getAttribute("data-ac-comanaged")).toBe("false");
    } finally {
      rendered.cleanup();
    }
  });

  it("drives the sidecar from session_comanaged_state through the real handler (test 14 app leg)", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() =>
        expect(fake.listensFor("session_comanaged_state").length).toBeGreaterThan(0),
      );

      fake.emitFromBackend("session_comanaged_state", {
        id: SESSION_B,
        active: true,
        reason: null,
      });
      expect(sessionsStore.comanagedBySessionId[SESSION_B]).toBe(true);
      expect(dot(rendered.root, SESSION_B).classList.contains("comanaged")).toBe(true);

      fake.emitFromBackend("session_comanaged_state", {
        id: SESSION_B,
        active: false,
        reason: "abstained",
      });
      expect(SESSION_B in sessionsStore.comanagedBySessionId).toBe(false);
      expect(dot(rendered.root, SESSION_B).classList.contains("comanaged")).toBe(false);
      expect(dot(rendered.root, SESSION_B).classList.contains("running")).toBe(true);
    } finally {
      rendered.cleanup();
    }
  });
});
