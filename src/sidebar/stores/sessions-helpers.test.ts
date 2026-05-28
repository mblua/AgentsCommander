import { describe, expect, it } from "vitest";
import type { Session, SessionStatus } from "../../shared/types";
import { isRuntimeStringStatus, upsertSessionList } from "./sessions-helpers";
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
    // that returned Session - upsertSessionList must replace the prior
    // (possibly stale or missing) entry.
    const sessions: Session[] = [];
    const returned = mkSession("root", "running", { isRootAgent: true, agentLabel: "claude" });
    const next = upsertSessionList(sessions, returned);
    expect(next.find((s) => s.isRootAgent)).toBeDefined();
    expect(next[0].agentLabel).toBe("claude");
  });
});
