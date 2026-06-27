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

import { isRuntimeStringStatus, preserveVisibleOrder, reconcileVisibleOrderKeys, upsertSessionList } from "./sessions-helpers";
import { sessionsStore } from "./sessions";
import { rootAgentCodingAgentAction } from "../components/root-agent-action";

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

/**
 * Simulates the mutation that sessionsStore.setActiveId applies to the
 * sessions array. Mirrors the predicates used inside sessions.ts so the
 * Exited-preservation contract is locked down without spinning up a
 * SolidJS root (vitest has no Solid render harness here).
 */
function applySetActiveIdMutation(sessions: Session[], id: string | null): Session[] {
  return sessions.map((s) => {
    if (s.id === id) {
      const status: SessionStatus = isRuntimeStringStatus(s.status) ? "active" : s.status;
      return { ...s, status, pendingReview: false };
    }
    if (s.status === "active") {
      return { ...s, status: "running" as SessionStatus };
    }
    return s;
  });
}

describe("setActiveId mutation (#274 dormant-root preservation)", () => {
  it("preserves Exited object status when the dormant root is selected", () => {
    const sessions = [mkSession("root", { exited: 0 }, { isRootAgent: true })];
    const next = applySetActiveIdMutation(sessions, "root");
    expect(next[0].status).toEqual({ exited: 0 });
  });

  it("preserves Exited({exited:1}) when the dormant root is selected", () => {
    const sessions = [mkSession("root", { exited: 1 }, { isRootAgent: true })];
    const next = applySetActiveIdMutation(sessions, "root");
    expect(next[0].status).toEqual({ exited: 1 });
  });

  it("clears pendingReview on the newly selected session even when Exited", () => {
    const sessions = [mkSession("root", { exited: 0 }, { isRootAgent: true, pendingReview: true })];
    const next = applySetActiveIdMutation(sessions, "root");
    expect(next[0].pendingReview).toBe(false);
    expect(next[0].status).toEqual({ exited: 0 });
  });

  it("promotes a runtime-string-status selection to active", () => {
    const sessions = [
      mkSession("a", "idle"),
      mkSession("b", "running"),
    ];
    const next = applySetActiveIdMutation(sessions, "b");
    expect(next.find((s) => s.id === "b")!.status).toBe("active");
  });

  it("demotes the previously active session to running", () => {
    const sessions = [
      mkSession("a", "active"),
      mkSession("b", "idle"),
    ];
    const next = applySetActiveIdMutation(sessions, "b");
    expect(next.find((s) => s.id === "a")!.status).toBe("running");
    expect(next.find((s) => s.id === "b")!.status).toBe("active");
  });

  it("after setActiveId on a dormant root, rootAgentCodingAgentAction still picks skipAutoResume:false", () => {
    // Regression: setActiveId used to overwrite { exited } with "active",
    // which made rootAgentCodingAgentAction treat the root as live and skip
    // provider resume. Verify the Exited object survives the mutation so
    // the wake path is preserved end-to-end.
    const sessions = [mkSession("root-id", { exited: 0 }, { isRootAgent: true })];
    const next = applySetActiveIdMutation(sessions, "root-id");
    const action = rootAgentCodingAgentAction(next[0], "claude");
    expect(action).toEqual({
      kind: "restart",
      id: "root-id",
      agentId: "claude",
      skipAutoResume: false,
    });
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
