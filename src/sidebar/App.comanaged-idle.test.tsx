// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import SidebarApp from "./App";
import { FakeTransport } from "../shared/testing/fake-transport";
import {
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../shared/testing/ui-harness";
import { SESSION_A, SESSION_B } from "../shared/testing/session-selection";
import {
  installReconcileIntervalSpy,
  setupAppTransport,
  type ReconcileIntervalSpy,
} from "./testing/app-harness";
import { sessionsStore } from "./stores/sessions";
import { sessionActivity } from "../shared/session-activity";
import type { Session, SessionStatus } from "../shared/types";

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

// #2453 test 5 - the status the backend snapshot reports for SESSION_B.
let backendStatusB: SessionStatus = "running";

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
      status: backendStatusB,
      waitingForInput: false,
    }),
  ];
}

function setupTransport(fake: FakeTransport): void {
  setupAppTransport(fake, {
    projectPath,
    agents: [
      { name: "General", path: agentAPath },
      { name: "Worker", path: agentBPath },
    ],
    rows: backendRows,
  });
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

function activity(id: string) {
  return sessionActivity(currentRow(id), {
    comanaged: sessionsStore.comanagedBySessionId[id] ?? false,
  });
}

describe("SidebarApp Co-managed idle edge (#2271)", () => {
  let cleanupDom: (() => void) | null = null;
  let reconcileIntervals: ReconcileIntervalSpy;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    sessionsStore.resetComanagedForTests();
    backendStatusB = "running";

    // Keep the real setInterval so the app's timers still run, then sweep the
    // 5000 ms reconcile handles in afterEach. Mocking the implementation would
    // hide a leak instead of exposing it.
    reconcileIntervals = installReconcileIntervalSpy({ periodMs: 5000 });
  });

  afterEach(() => {
    // The sweep MUST precede the restore: mockRestore() discards mock.calls and
    // mock.results and the handles become unrecoverable.
    reconcileIntervals.clear();
    reconcileIntervals.restore();
    cleanupDom?.();
    cleanupDom = null;
    sessionsStore.resetComanagedForTests();
    resetUiStoresForTests();
  });

  it("the idle edge arms Co-managed, still paints Co-managed, and records waiting underneath (test 8, #2453)", async () => {
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
      expect(currentRow(SESSION_B)?.waitingForInput).toBe(true);
      // SESSION_A is activeId in this fixture, so B's idle edge raises pendingReview.
      expect(currentRow(SESSION_B)?.pendingReview).toBe(true);
      expect(
        waitingSpy.mock.calls.filter(([id, waiting]) => id === SESSION_B && waiting === true),
      ).toHaveLength(1);
      // #2442 D4-g - the dot keeps its real activity colour; the Co-managed
      // signal is the additive ring, which the armed idle edge must add.
      expect(observedClasses.map((c) => c.split(/\s+/).includes("comanaged"))).toEqual([
        false,
        true,
      ]);

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
  // #2453 - a Co-managed cycle must leave the row as a plain idle edge would.
  async function mountApp(): Promise<{ fake: FakeTransport; cleanup: () => void }> {
    const fake = new FakeTransport();
    setupTransport(fake);
    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    await waitFor(() => {
      expect(fake.listensFor("session_idle").length).toBeGreaterThan(0);
      expect(fake.listensFor("session_comanaged_state").length).toBeGreaterThan(0);
      expect(sessionsStore.activeId).toBe(SESSION_A);
    });
    return { fake, cleanup: rendered.cleanup };
  }

  // Handlers run synchronously on emit, so `during` is read inside the window.
  function runCycle(fake: FakeTransport, id: string, during: string[]): void {
    fake.emitFromBackend("session_busy", { id });
    fake.emitFromBackend("session_idle", { id, comanaged: true });
    during.push(activity(id));
    fake.emitFromBackend("session_comanaged_state", { id, active: false, reason: null });
  }

  it("the Co-managed cycle ends in waitingForInput on the active session (#2453 test 1)", async () => {
    const { fake, cleanup } = await mountApp();
    try {
      runCycle(fake, SESSION_A, []);
      expect(activity(SESSION_A)).toBe("waitingForInput");
    } finally {
      cleanup();
    }
  });

  it("the Co-managed cycle ends in pendingReview on a non-active session (#2453 test 2)", async () => {
    const { fake, cleanup } = await mountApp();
    try {
      runCycle(fake, SESSION_B, []);
      expect(activity(SESSION_B)).toBe("pendingReview");
    } finally {
      cleanup();
    }
  });

  it("paints exactly comanaged during the armed window on both legs (#2453 test 3)", async () => {
    const { fake, cleanup } = await mountApp();
    try {
      const during: string[] = [];
      runCycle(fake, SESSION_A, during);
      runCycle(fake, SESSION_B, during);
      expect(during).toEqual(["comanaged", "comanaged"]);
    } finally {
      cleanup();
    }
  });

  it("a plain idle edge never paints comanaged (#2453 test 4)", async () => {
    const { fake, cleanup } = await mountApp();
    try {
      fake.emitFromBackend("session_busy", { id: SESSION_A });
      fake.emitFromBackend("session_idle", { id: SESSION_A, comanaged: false });
      fake.emitFromBackend("session_busy", { id: SESSION_B });
      fake.emitFromBackend("session_idle", { id: SESSION_B });
      expect(activity(SESSION_A)).toBe("waitingForInput");
      expect(activity(SESSION_B)).toBe("pendingReview");
    } finally {
      cleanup();
    }
  });

  it("the 250 ms reconciler keeps the waiting recorded by the armed idle edge (#2453 test 5)", async () => {
    const { fake, cleanup } = await mountApp();
    try {
      fake.emitFromBackend("session_busy", { id: SESSION_B });
      fake.emitFromBackend("session_idle", { id: SESSION_B, comanaged: true });
      backendStatusB = "idle";
      const listsBefore = fake.callsFor("list_sessions").length;
      window.dispatchEvent(new Event("focus"));
      // Real timers: the debounced refresh fires after 250 ms and lists once.
      await new Promise((resolve) => setTimeout(resolve, 400));
      expect(fake.callsFor("list_sessions").length).toBe(listsBefore + 1);
      expect(currentRow(SESSION_B)?.waitingForInput).toBe(true);
      fake.emitFromBackend("session_comanaged_state", { id: SESSION_B, active: false, reason: null });
      expect(activity(SESSION_B)).toBe("pendingReview");
    } finally {
      cleanup();
    }
  });
});
