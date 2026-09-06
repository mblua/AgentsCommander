// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "solid-js/web";
import SidebarApp from "./App";
import { FakeTransport } from "../shared/testing/fake-transport";
import { __setTransportForTests } from "../shared/ipc";
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
import type { Session, SessionStatus } from "../shared/types";

// #1779: the sidebar's pendingReview/waitingForInput mirror is fed by exactly two
// events, session_idle and session_busy. A single missed session_busy latches the
// amber dot forever, because nothing re-reads the backend's own view of the
// session. This mounts the real SidebarApp and drives the real edge sequence over
// FakeTransport; it never pokes the store into the state it asserts on, which is
// precisely why the existing suite missed the defect.

const SESSION_C = "33333333-3333-4333-8333-333333333333";

const projectPath = "C:\Project";
const agentAPath = `${projectPath}\.ac\_agent_General`;
const agentBPath = `${projectPath}\.ac\_agent_Worker`;
const agentCPath = `${projectPath}\.ac\_agent_Worker2`;

type BackendShape = { status: SessionStatus; waitingForInput: boolean };
let backendA: BackendShape = { status: "running", waitingForInput: false };
let backendB: BackendShape = { status: "running", waitingForInput: false };
let backendC: BackendShape = { status: "running", waitingForInput: false };
let outdatedC = false;

function backendRows(): Session[] {
  return [
    session({
      id: SESSION_A,
      name: "General",
      workingDirectory: agentAPath,
      status: backendA.status,
      waitingForInput: backendA.waitingForInput,
      profileOutdated: false,
    }),
    session({
      id: SESSION_B,
      name: "Worker",
      workingDirectory: agentBPath,
      status: backendB.status,
      waitingForInput: backendB.waitingForInput,
      profileOutdated: false,
    }),
    session({
      id: SESSION_C,
      name: "Worker2",
      workingDirectory: agentCPath,
      status: backendC.status,
      waitingForInput: backendC.waitingForInput,
      profileOutdated: outdatedC,
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
        { name: "Worker2", path: agentCPath, roleExists: true },
      ],
      teams: [],
      workgroups: [],
    }),
  );
  fake.resolve("get_project_groups", { groups: [], showAll: true, showUngrouped: true });
  fake.resolve("search_repos", []);
  // Always in the order [A, B, C]: the settling gate below and M3's kill both
  // depend on SESSION_C being the last row the reconcile loop visits.
  fake.onInvoke("list_sessions", () => backendRows());
  fake.resolve("get_active_session", liveSelection(SESSION_A));
  fake.resolve("list_detached_sessions", []);
  fake.resolve("telegram_list_bridges", []);
}

function row(root: HTMLElement, id: string): HTMLElement {
  const el = root.querySelector(`[data-ac-testid="session.${id}"]`);
  if (!el) throw new Error(`row ${id} not rendered`);
  return el as HTMLElement;
}

function dot(root: HTMLElement, id: string): HTMLElement {
  const el = row(root, id).querySelector(".session-item-status");
  if (!el) throw new Error(`status dot missing for ${id}`);
  return el as HTMLElement;
}

function badge(root: HTMLElement, id: string): Element | null {
  return row(root, id).querySelector(".profile-outdated-badge");
}

describe("SidebarApp pending-review latch reconciliation (#1779)", () => {
  let cleanupDom: (() => void) | null = null;
  let reconcileTicks: () => unknown[][];
  let clearReconcileIntervals: () => void;
  let restoreIntervalSpy: () => void;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    backendA = { status: "running", waitingForInput: false };
    backendB = { status: "running", waitingForInput: false };
    backendC = { status: "running", waitingForInput: false };
    outdatedC = false;

    // vi.spyOn with no implementation keeps the real setInterval, so the app's
    // timers still run and mock.results[i].value is the real handle, index-aligned
    // with mock.calls[i]. File-wide rather than per-test because under M6 the
    // component stops clearing its interval and would otherwise leak a live 5000 ms
    // timer into every test that follows.
    const intervalSpy = vi.spyOn(globalThis, "setInterval");
    reconcileTicks = () => intervalSpy.mock.calls.filter((c) => c[1] === 5000);
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
    resetUiStoresForTests();
  });

  it("a latched row heals on window focus when the backend still reports the session working", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() =>
        expect(fake.listensFor("session_idle").length).toBeGreaterThan(0),
      );

      fake.emitFromBackend("session_idle", { id: SESSION_B });
      expect(dot(rendered.root, SESSION_B).classList.contains("pending")).toBe(true);

      outdatedC = true;
      window.dispatchEvent(new Event("focus"));

      await waitFor(() => expect(badge(rendered.root, SESSION_C)).not.toBeNull(), 3000);
      expect(dot(rendered.root, SESSION_B).classList.contains("running")).toBe(true);
    } finally {
      rendered.cleanup();
    }
  });

  it("a latched row heals with no focus event, on the periodic tick", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() =>
        expect(fake.listensFor("session_idle").length).toBeGreaterThan(0),
      );

      fake.emitFromBackend("session_idle", { id: SESSION_B });
      expect(dot(rendered.root, SESSION_B).classList.contains("pending")).toBe(true);

      outdatedC = true;

      // The delay, the count and the wiring in one assertion.
      const ticks = reconcileTicks();
      expect(ticks.length).toBe(1);

      (ticks[0][0] as () => void)();

      await waitFor(() => expect(badge(rendered.root, SESSION_C)).not.toBeNull(), 3000);
      expect(dot(rendered.root, SESSION_B).classList.contains("running")).toBe(true);
    } finally {
      rendered.cleanup();
    }
  });

  it.each([
    { label: "waiting", backend: { status: "running", waitingForInput: true } },
    { label: "idle", backend: { status: "idle", waitingForInput: false } },
    { label: "exited", backend: { status: { exited: 0 }, waitingForInput: false } },
  ] as { label: string; backend: BackendShape }[])(
    "a session the backend does not report as working keeps its pendingReview dot ($label)",
    async ({ backend }) => {
      const fake = new FakeTransport();
      setupTransport(fake);
      const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
      try {
        await waitFor(() =>
          expect(fake.listensFor("session_idle").length).toBeGreaterThan(0),
        );

        fake.emitFromBackend("session_idle", { id: SESSION_B });
        expect(dot(rendered.root, SESSION_B).classList.contains("pending")).toBe(true);

        backendB = backend;
        outdatedC = true;
        window.dispatchEvent(new Event("focus"));

        await waitFor(() => expect(badge(rendered.root, SESSION_C)).not.toBeNull(), 3000);
        expect(dot(rendered.root, SESSION_B).classList.contains("pending")).toBe(true);
      } finally {
        rendered.cleanup();
      }
    },
  );

  it("a waiting edge that lands while the list is in flight survives the stale snapshot", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() =>
        expect(fake.listensFor("session_idle").length).toBeGreaterThan(0),
      );

      const before = fake.callsFor("list_sessions").length;
      let resolveList!: (v: unknown) => void;
      const pendingList = new Promise((r) => {
        resolveList = r;
      });
      fake.onInvoke("list_sessions", () => pendingList);

      outdatedC = true;
      window.dispatchEvent(new Event("focus"));
      await waitFor(() =>
        expect(fake.callsFor("list_sessions").length).toBeGreaterThan(before),
      );

      // A FRESH, correct edge, landing inside the flight window.
      fake.emitFromBackend("session_idle", { id: SESSION_B });

      // The stale "working" snapshot.
      resolveList([
        session({
          id: SESSION_A,
          name: "General",
          workingDirectory: agentAPath,
          status: "running",
          waitingForInput: false,
          profileOutdated: false,
        }),
        session({
          id: SESSION_B,
          name: "Worker",
          workingDirectory: agentBPath,
          status: "running",
          waitingForInput: false,
          profileOutdated: false,
        }),
        session({
          id: SESSION_C,
          name: "Worker2",
          workingDirectory: agentCPath,
          status: "running",
          waitingForInput: false,
          profileOutdated: true,
        }),
      ]);

      await waitFor(() => expect(badge(rendered.root, SESSION_C)).not.toBeNull(), 3000);
      expect(dot(rendered.root, SESSION_B).classList.contains("pending")).toBe(true);
    } finally {
      rendered.cleanup();
    }
  });

  it(
    "the periodic reconcile stops when the sidebar unmounts",
    async () => {
      const fake = new FakeTransport();
      setupTransport(fake);
      // Rendered manually: renderWithFakeTransport's cleanup calls dispose() and
      // then restoreTransport(), so after it a leaked interval would call the
      // restored transport and fake.calls would not grow. That would be a dead
      // detector for exactly the leak this test exists to catch.
      const restoreTransport = __setTransportForTests(fake);
      const root = document.createElement("div");
      document.body.appendChild(root);
      const dispose = render(() => <SidebarApp embedded />, root);
      let alreadyDisposed = false;
      const disposeOnce = () => {
        if (alreadyDisposed) return;
        alreadyDisposed = true;
        dispose();
      };
      try {
        await waitFor(() =>
          expect(fake.listensFor("session_idle").length).toBeGreaterThan(0),
        );

        const before = fake.callsFor("list_sessions").length;
        disposeOnce();
        await new Promise((r) => setTimeout(r, 6000));

        // Read the observation into a local BEFORE the sweep, and sweep BEFORE the
        // assert: under M6 the expect throws, and a sweep written after it would
        // never run on the one path that actually leaks.
        const after = fake.callsFor("list_sessions").length;
        clearReconcileIntervals();
        expect(after).toBe(before);
      } finally {
        clearReconcileIntervals();
        disposeOnce();
        restoreTransport();
        root.remove();
      }
    },
    20000,
  );

  it("two rows latched at once both heal in a single reconcile", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() =>
        expect(fake.listensFor("session_idle").length).toBeGreaterThan(0),
      );

      fake.emitFromBackend("session_idle", { id: SESSION_B });
      fake.emitFromBackend("session_idle", { id: SESSION_C });
      expect(dot(rendered.root, SESSION_B).classList.contains("pending")).toBe(true);
      expect(dot(rendered.root, SESSION_C).classList.contains("pending")).toBe(true);

      outdatedC = true;
      window.dispatchEvent(new Event("focus"));

      await waitFor(() => expect(badge(rendered.root, SESSION_C)).not.toBeNull(), 3000);
      expect(dot(rendered.root, SESSION_B).classList.contains("running")).toBe(true);
      expect(dot(rendered.root, SESSION_C).classList.contains("running")).toBe(true);
    } finally {
      rendered.cleanup();
    }
  });

  it("the selected row is reconciled too, and an active backend row still counts as working", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() => {
        expect(fake.listensFor("session_idle").length).toBeGreaterThan(0);
        expect(sessionsStore.activeId).toBe(SESSION_A);
      });

      // setSessionWaiting raises waitingForInput but NOT pendingReview, because
      // id === state.activeId, so the characterisation class is `waiting`.
      fake.emitFromBackend("session_idle", { id: SESSION_A });
      expect(dot(rendered.root, SESSION_A).classList.contains("waiting")).toBe(true);

      backendA = { status: "active", waitingForInput: false };
      outdatedC = true;
      window.dispatchEvent(new Event("focus"));

      await waitFor(() => expect(badge(rendered.root, SESSION_C)).not.toBeNull(), 3000);
      // The healed class here is `active`, not `running`: SESSION_A is the
      // live-selected id, so the store holds status "active" for it.
      expect(dot(rendered.root, SESSION_A).classList.contains("active")).toBe(true);
    } finally {
      rendered.cleanup();
    }
  });

  it("a waiting edge on the SELECTED row that lands while the list is in flight survives the stale snapshot", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() => {
        expect(fake.listensFor("session_idle").length).toBeGreaterThan(0);
        expect(sessionsStore.activeId).toBe(SESSION_A);
      });

      const before = fake.callsFor("list_sessions").length;
      let resolveList!: (v: unknown) => void;
      const pendingList = new Promise((r) => {
        resolveList = r;
      });
      fake.onInvoke("list_sessions", () => pendingList);

      outdatedC = true;
      window.dispatchEvent(new Event("focus"));
      await waitFor(() =>
        expect(fake.callsFor("list_sessions").length).toBeGreaterThan(before),
      );

      // A FRESH, correct edge on the SELECTED row, landing inside the flight window.
      fake.emitFromBackend("session_idle", { id: SESSION_A });
      expect(dot(rendered.root, SESSION_A).classList.contains("waiting")).toBe(true);

      // The stale snapshot: it reports the selected session as working and not
      // waiting, which the edge above has just contradicted.
      resolveList([
        session({
          id: SESSION_A,
          name: "General",
          workingDirectory: agentAPath,
          status: "active",
          waitingForInput: false,
          profileOutdated: false,
        }),
        session({
          id: SESSION_B,
          name: "Worker",
          workingDirectory: agentBPath,
          status: "running",
          waitingForInput: false,
          profileOutdated: false,
        }),
        session({
          id: SESSION_C,
          name: "Worker2",
          workingDirectory: agentCPath,
          status: "running",
          waitingForInput: false,
          profileOutdated: true,
        }),
      ]);

      await waitFor(() => expect(badge(rendered.root, SESSION_C)).not.toBeNull(), 3000);
      expect(dot(rendered.root, SESSION_A).classList.contains("waiting")).toBe(true);
    } finally {
      rendered.cleanup();
    }
  });

  it("the periodic tick does nothing while the window is hidden", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() =>
        expect(fake.listensFor("session_idle").length).toBeGreaterThan(0),
      );

      // Real work a reconcile would visibly do. outdatedC stays false and this test
      // uses no reconcile gate: its whole point is that nothing runs.
      fake.emitFromBackend("session_idle", { id: SESSION_B });
      expect(dot(rendered.root, SESSION_B).classList.contains("pending")).toBe(true);

      const ticks = reconcileTicks();
      expect(ticks.length).toBe(1);
      const hiddenBefore = fake.callsFor("list_sessions").length;

      // Shadowing jsdom's prototype getter on the document INSTANCE; the repo's own
      // precedent is src/shared/ipc-blackbox.test.ts:70-73 and :78.
      Object.defineProperty(document, "visibilityState", {
        configurable: true,
        get: () => "hidden",
      });
      (ticks[0][0] as () => void)();
      await new Promise((r) => setTimeout(r, 300));
      const hiddenAfter = fake.callsFor("list_sessions").length;

      expect(hiddenAfter).toBe(hiddenBefore);
      expect(dot(rendered.root, SESSION_B).classList.contains("pending")).toBe(true);
    } finally {
      Reflect.deleteProperty(document, "visibilityState");
      rendered.cleanup();
    }
  });

  it("a list response that resolves after unmount writes nothing into the store", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    // Rendered manually for a related reason to T5's: this test must unmount while
    // the fake is still installed and the list call is still suspended on it.
    const restoreTransport = __setTransportForTests(fake);
    const root = document.createElement("div");
    document.body.appendChild(root);
    const dispose = render(() => <SidebarApp embedded />, root);
    let alreadyDisposed = false;
    const disposeOnce = () => {
      if (alreadyDisposed) return;
      alreadyDisposed = true;
      dispose();
    };
    try {
      await waitFor(() =>
        expect(fake.listensFor("session_idle").length).toBeGreaterThan(0),
      );

      fake.emitFromBackend("session_idle", { id: SESSION_B });
      expect(dot(root, SESSION_B).classList.contains("pending")).toBe(true);

      const before = fake.callsFor("list_sessions").length;
      let resolveList!: (v: unknown) => void;
      const pendingList = new Promise((r) => {
        resolveList = r;
      });
      fake.onInvoke("list_sessions", () => pendingList);

      window.dispatchEvent(new Event("focus"));
      await waitFor(() =>
        expect(fake.callsFor("list_sessions").length).toBeGreaterThan(before),
      );

      // disposed = true is the FIRST statement of onCleanup and Solid runs
      // onCleanup synchronously inside dispose().
      disposeOnce();

      // The HEALING snapshot: the one that would clear the latch had the component
      // still been mounted.
      resolveList([
        session({
          id: SESSION_A,
          name: "General",
          workingDirectory: agentAPath,
          status: "active",
          waitingForInput: false,
          profileOutdated: false,
        }),
        session({
          id: SESSION_B,
          name: "Worker",
          workingDirectory: agentBPath,
          status: "running",
          waitingForInput: false,
          profileOutdated: false,
        }),
        session({
          id: SESSION_C,
          name: "Worker2",
          workingDirectory: agentCPath,
          status: "running",
          waitingForInput: false,
          profileOutdated: false,
        }),
      ]);
      await pendingList;
      await new Promise((r) => setTimeout(r, 300));

      const storedB = sessionsStore.sessions.find((r) => r.id === SESSION_B);
      if (!storedB) throw new Error(`row ${SESSION_B} missing from the store`);
      expect(storedB.waitingForInput).toBe(true);
      expect(storedB.pendingReview).toBe(true);
    } finally {
      clearReconcileIntervals();
      disposeOnce();
      restoreTransport();
      root.remove();
    }
  });
});
