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
    workgroupBrief: null,
    isCoordinator: false,
    isRootAgent: true,
    token: "",
    agentKind: null,
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
    expect(rootAgentCodingAgentAction(mkRoot("idle"), "gemini")).toEqual({
      kind: "restart",
      id: "root-id",
      agentId: "gemini",
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
});
