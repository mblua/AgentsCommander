// @vitest-environment jsdom

import { describe, expect, it, vi } from "vitest";
import type { Session, SessionStatus } from "../../shared/types";

vi.hoisted(() => {
  class MockWebSocket {
    static readonly CONNECTING = 0;
    static readonly OPEN = 1;
    static readonly CLOSING = 2;
    static readonly CLOSED = 3;

    readonly url: string;
    binaryType: BinaryType = "blob";
    readyState = MockWebSocket.CLOSED;

    constructor(url: string) {
      this.url = url;
    }

    send(): void {}

    close(): void {
      this.readyState = MockWebSocket.CLOSED;
    }
  }

  Object.defineProperty(globalThis, "WebSocket", {
    configurable: true,
    writable: true,
    value: MockWebSocket,
  });
});

import { applySelectionToSessionList, isRuntimeStringStatus, preserveVisibleOrder, reconcileVisibleOrderKeys, upsertSessionList } from "./sessions-helpers";
import { sessionsStore } from "./sessions";
import { rootAgentCodingAgentAction } from "../components/root-agent-action";
import { dormantSelection, liveSelection, noneSelection, SESSION_A, SESSION_B, TEST_EPOCH, TEST_EPOCH_2 } from "../../shared/testing/session-selection";

function mkSession(id: string, status: SessionStatus, overrides: Partial<Session> = {}): Session {
  return {
    id,
    name: `session-${id}`,
    shell: "",
    shellArgs: [],
    effectiveShellArgs: null,
    createdAt: "",
    workingDirectory: "",
    status,
    waitingForInput: false,
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
  };
}

describe("isRuntimeStringStatus", () => {
  it("returns true for runtime string statuses", () => {
    expect(isRuntimeStringStatus("active")).toBe(true);
    expect(isRuntimeStringStatus("running")).toBe(true);
    expect(isRuntimeStringStatus("idle")).toBe(true);
  });

  it("returns false for Exited object statuses", () => {
    expect(isRuntimeStringStatus({ exited: 0 })).toBe(false);
    expect(isRuntimeStringStatus({ exited: 1 })).toBe(false);
    expect(isRuntimeStringStatus({ exited: 137 })).toBe(false);
  });
});

describe("applySelectionToSessionList", () => {
  it("promotes only the live target and demotes the previous active row", () => {
    const applied = applySelectionToSessionList(
      [mkSession(SESSION_A, "active"), mkSession(SESSION_B, "idle")],
      liveSelection(SESSION_B, 2),
    );
    expect(applied.activeId).toBe(SESSION_B);
    expect(applied.sessions.find((session) => session.id === SESSION_A)?.status).toBe("running");
    expect(applied.sessions.find((session) => session.id === SESSION_B)?.status).toBe("active");
  });

  it("preserves the exact dormant exit code and clears pending review", () => {
    const applied = applySelectionToSessionList(
      [mkSession(SESSION_A, { exited: 1 }, { isRootAgent: true, pendingReview: true })],
      dormantSelection(SESSION_A, 2, 137),
    );
    expect(applied.activeId).toBe(SESSION_A);
    expect(applied.sessions[0].status).toEqual({ exited: 137 });
    expect(applied.sessions[0].pendingReview).toBe(false);
    const action = rootAgentCodingAgentAction(applied.sessions[0], "claude");
    expect(action).toEqual({
      kind: "restart",
      id: SESSION_A,
      agentId: "claude",
      skipAutoResume: false,
    });
  });

  it("clears highlight for none", () => {
    const applied = applySelectionToSessionList(
      [mkSession(SESSION_A, "active")],
      noneSelection(2),
    );
    expect(applied.activeId).toBeNull();
    expect(applied.sessions[0].status).toBe("running");
  });

  it("refuses to re-promote an exited row from a stored live payload", () => {
    const applied = applySelectionToSessionList(
      [mkSession(SESSION_A, { exited: 9 })],
      liveSelection(SESSION_A, 2),
    );
    expect(applied.activeId).toBeNull();
    expect(applied.sessions[0].status).toEqual({ exited: 9 });
  });

  it("retains selection authority without highlighting a missing row", () => {
    const applied = applySelectionToSessionList([], liveSelection(SESSION_A, 2));
    expect(applied.activeId).toBeNull();
    expect(applied.sessions).toEqual([]);
  });
});

describe("upsertSessionList (#274 banner-reuse hydration)", () => {
  it("appends a new session when id is not present", () => {
    const sessions = [mkSession("a", "idle")];
    const next = upsertSessionList(sessions, mkSession("b", "running"));
    expect(next).toHaveLength(2);
    expect(next[1].id).toBe("b");
  });

  it("updates fields on an existing id rather than no-op", () => {
    const sessions = [mkSession("root", { exited: 0 }, { isRootAgent: true, agentLabel: "old" })];
    const next = upsertSessionList(
      sessions,
      mkSession("root", "running", { isRootAgent: true, agentLabel: "new" }),
    );
    expect(next).toHaveLength(1);
    expect(next[0].status).toBe("running");
    expect(next[0].agentLabel).toBe("new");
  });

  // #647 (test 7, E5): a verified RM Kill makes the backend emit session_created
  // carrying the Exited SessionInfo for the SAME id. The sidebar's listener
  // routes it through addSession -> upsertSessionList, which must UPDATE the
  // existing tile's status to Exited (not append a duplicate) so the flip renders.
  it("flips an existing running session to Exited on a verified-kill session_created (E5)", () => {
    const sessions = [mkSession("agent-1", "running")];
    const next = upsertSessionList(sessions, mkSession("agent-1", { exited: 0 }));
    expect(next).toHaveLength(1);
    expect(next[0].id).toBe("agent-1");
    expect(next[0].status).toEqual({ exited: 0 });
  });

  it("returns a new array reference on every call so SolidJS reactivity triggers", () => {
    const sessions = [mkSession("a", "idle")];
    const next = upsertSessionList(sessions, mkSession("a", "running"));
    expect(next).not.toBe(sessions);
  });

  it("upsert with the returned root Session hydrates the store after createRootAgent reuse", () => {
    // Simulates the failure scenario from grinch finding #2: backend
    // already had the root, the initial list_sessions raced ahead of the
    // listener registration, banner called createRootAgent() and the
    // backend returned the existing Session WITHOUT emitting
    // session_created. The banner now calls sessionsStore.addSession with
    // that returned Session — upsertSessionList must replace the prior
    // (possibly stale or missing) entry.
    const sessions: Session[] = [];
    const returned = mkSession("root", "running", { isRootAgent: true, agentLabel: "claude" });
    const next = upsertSessionList(sessions, returned);
    expect(next.find((s) => s.isRootAgent)).toBeDefined();
    expect(next[0].agentLabel).toBe("claude");
  });
});

describe("sessionsStore authoritative ordering", () => {
  function connect(): void {
    sessionsStore.resetSelectionForTests();
    sessionsStore.setSessions([
      mkSession(SESSION_A, "running"),
      mkSession(SESSION_B, "running"),
    ]);
    sessionsStore.observeConnection({ state: "connected", generation: 0 });
  }

  it("ignores older and normal equal revisions", () => {
    connect();
    expect(sessionsStore.applySelection(liveSelection(SESSION_B, 2), 0)).toBe(true);
    expect(sessionsStore.applySelection(liveSelection(SESSION_A, 1), 0)).toBe(false);
    expect(sessionsStore.applySelection(liveSelection(SESSION_A, 2), 0)).toBe(false);
    expect(sessionsStore.activeId).toBe(SESSION_B);
  });

  it("lets a new epoch supersede a high revision and permanently retires the old epoch", () => {
    connect();
    expect(sessionsStore.applySelection(liveSelection(SESSION_A, 500, TEST_EPOCH), 0)).toBe(true);
    expect(sessionsStore.applySelection(liveSelection(SESSION_B, 1, TEST_EPOCH_2), 0)).toBe(true);
    expect(sessionsStore.applySelection(liveSelection(SESSION_A, 501, TEST_EPOCH), 0)).toBe(false);
    expect(sessionsStore.activeId).toBe(SESSION_B);
  });

  it("allows one exact equal reconnect rebind only for the awaited generation", () => {
    connect();
    expect(sessionsStore.applySelection(liveSelection(SESSION_A, 2), 0)).toBe(true);
    sessionsStore.observeConnection({ state: "disconnected", generation: 0 });
    sessionsStore.observeConnection({ state: "connected", generation: 1 });
    expect(sessionsStore.beginHydration(1)).toBe(true);
    expect(sessionsStore.applySelection(liveSelection(SESSION_A, 2), 1, true)).toBe(true);
    expect(sessionsStore.applySelection(liveSelection(SESSION_A, 2), 1, true)).toBe(false);
  });

  it("does not reproject the previous generation from a row update before reconnect hydration", () => {
    connect();
    expect(sessionsStore.applySelection(liveSelection(SESSION_A, 2), 0)).toBe(true);
    sessionsStore.observeConnection({ state: "disconnected", generation: 0 });
    sessionsStore.observeConnection({ state: "connected", generation: 1 });
    expect(sessionsStore.beginHydration(1)).toBe(true);
    sessionsStore.addSession(mkSession(SESSION_A, "active"));
    expect(sessionsStore.activeId).toBeNull();
    expect(sessionsStore.sessions.find((row) => row.id === SESSION_A)?.status).toBe("running");
    expect(sessionsStore.applySelection(liveSelection(SESSION_A, 2), 1, true)).toBe(true);
    expect(sessionsStore.activeId).toBe(SESSION_A);
  });

  it("reapplies stored selection after upsert but lets an exited upsert win", () => {
    connect();
    sessionsStore.setSessions([]);
    expect(sessionsStore.applySelection(liveSelection(SESSION_A, 2), 0)).toBe(true);
    sessionsStore.addSession(mkSession(SESSION_A, "running"));
    expect(sessionsStore.activeId).toBe(SESSION_A);
    sessionsStore.addSession(mkSession(SESSION_A, { exited: 17 }));
    expect(sessionsStore.activeId).toBeNull();
    expect(sessionsStore.sessions[0].status).toEqual({ exited: 17 });
  });
});

describe("preserveVisibleOrder", () => {
  it("keeps previous visible positions when next order changes", () => {
    const previous = ["coord-a", "coord-b", "coord-c"];
    const next = ["coord-c", "coord-b", "coord-a"];

    expect(preserveVisibleOrder(next, previous, (item) => item)).toEqual([
      "coord-a",
      "coord-b",
      "coord-c",
    ]);
  });

  it("removes missing items and appends newly visible items", () => {
    const previous = ["coord-a", "coord-b", "coord-c"];
    const next = ["coord-d", "coord-c", "coord-a"];

    expect(preserveVisibleOrder(next, previous, (item) => item)).toEqual([
      "coord-a",
      "coord-c",
      "coord-d",
    ]);
  });

  it("uses the next order when there is no previous visible order", () => {
    const next = ["coord-c", "coord-b", "coord-a"];
    expect(preserveVisibleOrder(next, undefined, (item) => item)).toEqual(next);
  });
});

describe("reconcileVisibleOrderKeys", () => {
  it("keeps existing frozen keys in place while removing disappeared keys and appending new keys", () => {
    expect(reconcileVisibleOrderKeys(["coord-d", "coord-c", "coord-a"], ["coord-a", "coord-b", "coord-c"])).toEqual([
      "coord-a",
      "coord-c",
      "coord-d",
    ]);
  });
});

describe("sidebar coordinator hover freeze", () => {
  it("survives recent-first recompute and releases after hover ends", () => {
    const projectPath = "test-project-hover-recompute";

    sessionsStore.setSidebarPointerInside(false);
    sessionsStore.recordCoordinatorVisibleOrder(projectPath, ["coord-a", "coord-b", "coord-c"]);

    sessionsStore.setSidebarPointerInside(true);
    expect(sessionsStore.coordinatorVisibleOrder(projectPath, ["coord-c", "coord-a", "coord-b"])).toEqual([
      "coord-a",
      "coord-b",
      "coord-c",
    ]);

    // Simulates project/workgroup object replacement recreating ProjectPanel memos.
    expect(sessionsStore.coordinatorVisibleOrder(projectPath, ["coord-c", "coord-a", "coord-b"])).toEqual([
      "coord-a",
      "coord-b",
      "coord-c",
    ]);

    sessionsStore.setSidebarPointerInside(false);
    expect(sessionsStore.coordinatorVisibleOrder(projectPath, ["coord-c", "coord-a", "coord-b"])).toEqual([
      "coord-c",
      "coord-a",
      "coord-b",
    ]);
  });

  it("removes disappeared coordinators and appends newly visible coordinators while hovered", () => {
    const projectPath = "test-project-hover-structural-change";

    sessionsStore.setSidebarPointerInside(false);
    sessionsStore.recordCoordinatorVisibleOrder(projectPath, ["coord-a", "coord-b", "coord-c"]);

    sessionsStore.setSidebarPointerInside(true);
    expect(sessionsStore.coordinatorVisibleOrder(projectPath, ["coord-d", "coord-c", "coord-a"])).toEqual([
      "coord-a",
      "coord-c",
      "coord-d",
    ]);

    sessionsStore.setSidebarPointerInside(false);
  });
});

describe("sidebar menu-open order lock", () => {
  it("holds the frozen order while a menu is open with the pointer outside", () => {
    const projectPath = "test-project-menu-open";

    sessionsStore.setSidebarMenuOpen(false);
    sessionsStore.recordCoordinatorVisibleOrder(projectPath, ["coord-a", "coord-b", "coord-c"]);

    sessionsStore.setSidebarMenuOpen(true);
    expect(sessionsStore.coordinatorVisibleOrder(projectPath, ["coord-c", "coord-a", "coord-b"])).toEqual([
      "coord-a",
      "coord-b",
      "coord-c",
    ]);

    sessionsStore.setSidebarMenuOpen(false);
  });

  it("releases when the menu closes", () => {
    const projectPath = "test-project-menu-release";

    sessionsStore.setSidebarMenuOpen(false);
    sessionsStore.recordCoordinatorVisibleOrder(projectPath, ["coord-a", "coord-b", "coord-c"]);

    sessionsStore.setSidebarMenuOpen(true);
    expect(sessionsStore.coordinatorVisibleOrder(projectPath, ["coord-c", "coord-a", "coord-b"])).toEqual([
      "coord-a",
      "coord-b",
      "coord-c",
    ]);

    sessionsStore.setSidebarMenuOpen(false);
    expect(sessionsStore.coordinatorVisibleOrder(projectPath, ["coord-c", "coord-a", "coord-b"])).toEqual([
      "coord-c",
      "coord-a",
      "coord-b",
    ]);
  });

  it("pointer-leave while a menu is open keeps the freeze", () => {
    const projectPath = "test-project-menu-pointer-leave";

    sessionsStore.recordCoordinatorVisibleOrder(projectPath, ["coord-a", "coord-b", "coord-c"]);

    sessionsStore.setSidebarPointerInside(true);
    sessionsStore.setSidebarMenuOpen(true);
    sessionsStore.setSidebarPointerInside(false);
    expect(sessionsStore.coordinatorVisibleOrder(projectPath, ["coord-c", "coord-a", "coord-b"])).toEqual([
      "coord-a",
      "coord-b",
      "coord-c",
    ]);

    sessionsStore.setSidebarMenuOpen(false);
    expect(sessionsStore.coordinatorVisibleOrder(projectPath, ["coord-c", "coord-a", "coord-b"])).toEqual([
      "coord-c",
      "coord-a",
      "coord-b",
    ]);
  });

  it("drops disappeared and appends new coordinators while menu-locked", () => {
    const projectPath = "test-project-menu-structural-change";

    sessionsStore.setSidebarMenuOpen(false);
    sessionsStore.recordCoordinatorVisibleOrder(projectPath, ["coord-a", "coord-b", "coord-c"]);

    sessionsStore.setSidebarMenuOpen(true);
    expect(sessionsStore.coordinatorVisibleOrder(projectPath, ["coord-d", "coord-c", "coord-a"])).toEqual([
      "coord-a",
      "coord-c",
      "coord-d",
    ]);

    sessionsStore.setSidebarMenuOpen(false);
  });

  it("re-snapshots the last recorded visible order when the lock re-engages", () => {
    const projectPath = "test-project-menu-re-snapshot";

    sessionsStore.setSidebarMenuOpen(false);
    sessionsStore.recordCoordinatorVisibleOrder(projectPath, ["coord-a", "coord-b", "coord-c"]);
    sessionsStore.setSidebarMenuOpen(true);
    expect(sessionsStore.coordinatorVisibleOrder(projectPath, ["coord-c", "coord-a", "coord-b"])).toEqual([
      "coord-a",
      "coord-b",
      "coord-c",
    ]);
    sessionsStore.setSidebarMenuOpen(false);
    // Explicit mirror of ProjectPanel.coordinatorItems recording the recomputed
    // order once the lock is off; without it, "last" still holds [a,b,c].
    sessionsStore.recordCoordinatorVisibleOrder(projectPath, ["coord-c", "coord-a", "coord-b"]);
    sessionsStore.setSidebarMenuOpen(true);
    expect(sessionsStore.coordinatorVisibleOrder(projectPath, ["coord-b", "coord-c", "coord-a"])).toEqual([
      "coord-c",
      "coord-a",
      "coord-b",
    ]);
    sessionsStore.setSidebarMenuOpen(false); // end released — module state stays clean
  });
});
