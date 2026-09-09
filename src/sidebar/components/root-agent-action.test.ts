import { describe, expect, it } from "vitest";
import type { Session, SessionStatus } from "../../shared/types";
import { rootAgentCodingAgentAction } from "./root-agent-action";

function mkRoot(status: SessionStatus): Session {
  return {
    id: "root-id",
    name: "Agent's Commander",
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
    isRootAgent: true,
    token: "",
    agentKind: null,
    requestedProfile: null,
    effectiveProfile: null,
    profileFallbackChain: [],
    profileFallbackApplied: false,
  };
}

describe("rootAgentCodingAgentAction", () => {
  it("creates a new root with the chosen agent when no root exists", () => {
    expect(rootAgentCodingAgentAction(undefined, "claude")).toEqual({
      kind: "create",
      agentId: "claude",
    });
  });

  it("restarts a live root without skipAutoResume (defaults to fresh restart)", () => {
    expect(rootAgentCodingAgentAction(mkRoot("running"), "claude")).toEqual({
      kind: "restart",
      id: "root-id",
      agentId: "claude",
    });
  });

  it("restarts an active root without skipAutoResume", () => {
    expect(rootAgentCodingAgentAction(mkRoot("active"), "codex")).toEqual({
      kind: "restart",
      id: "root-id",
      agentId: "codex",
    });
  });

  it("restarts an idle root without skipAutoResume", () => {
    expect(rootAgentCodingAgentAction(mkRoot("idle"), "antigravity")).toEqual({
      kind: "restart",
      id: "root-id",
      agentId: "antigravity",
    });
  });

  it("restarts a dormant root with skipAutoResume:false so provider resume runs", () => {
    expect(rootAgentCodingAgentAction(mkRoot({ exited: 0 }), "claude")).toEqual({
      kind: "restart",
      id: "root-id",
      agentId: "claude",
      skipAutoResume: false,
    });
  });

  it("restarts a dormant root that exited non-zero with skipAutoResume:false", () => {
    expect(rootAgentCodingAgentAction(mkRoot({ exited: 1 }), "codex")).toEqual({
      kind: "restart",
      id: "root-id",
      agentId: "codex",
      skipAutoResume: false,
    });
  });

  // #1861 - Muse rides the same provider-neutral mapping. The frontend never
  // invents Muse argv; the backend (#1873) maps the intent to a fresh `muse`
  // or `muse resume --last`.
  describe("Muse Code (#1861)", () => {
    it("creates a new root with muse when no root exists (fresh create path)", () => {
      expect(rootAgentCodingAgentAction(undefined, "muse")).toStrictEqual({
        kind: "create",
        agentId: "muse",
      });
    });

    it("restarts a live root with skipAutoResume OMITTED: fresh replacement, never live reuse", () => {
      const live = rootAgentCodingAgentAction(mkRoot("running"), "muse");
      expect(live).toStrictEqual({ kind: "restart", id: "root-id", agentId: "muse" });
      // Equality alone is not omission proof: the key must not exist at all, so
      // it serializes as null and the backend defaults null to fresh.
      expect(live).not.toHaveProperty("skipAutoResume");
      expect("skipAutoResume" in live).toBe(false);
    });

    it("restarts a dormant root with an own skipAutoResume:false so the backend may resume", () => {
      const dormant = rootAgentCodingAgentAction(mkRoot({ exited: 0 }), "muse");
      expect(dormant).toStrictEqual({
        kind: "restart",
        id: "root-id",
        agentId: "muse",
        skipAutoResume: false,
      });
      expect(Object.prototype.hasOwnProperty.call(dormant, "skipAutoResume")).toBe(true);
      expect(dormant).toHaveProperty("skipAutoResume", false);
    });
  });
});
