import { describe, expect, it } from "vitest";
import type { AcAgentReplica, Session } from "../../shared/types";
import { repoBadgesForReplica } from "./repo-badges";

function mkReplica(overrides: Partial<AcAgentReplica> = {}): AcAgentReplica {
  return {
    name: "coordinator",
    path: "C:/wg/__agent_coordinator",
    repoPaths: [],
    isCoordinator: true,
    ...overrides,
  };
}

function mkSession(overrides: Partial<Session> = {}): Session {
  return {
    id: "session-id",
    name: "wg-1-dev-team/coordinator",
    shell: "",
    shellArgs: [],
    effectiveShellArgs: null,
    createdAt: "",
    workingDirectory: "C:/wg/__agent_coordinator",
    status: "running",
    waitingForInput: false,
    pendingReview: false,
    lastPrompt: null,
    agentId: null,
    agentLabel: null,
    gitRepos: [],
    workgroupTask: null,
    isCoordinator: true,
    isRootAgent: false,
    token: "",
    agentKind: null,
    ...overrides,
  };
}

describe("repoBadgesForReplica", () => {
  it("prefers live session gitRepos", () => {
    const replica = mkReplica({
      repoPaths: ["C:/wg/repo-stale"],
    });
    const session = mkSession({
      gitRepos: [
        {
          label: "live",
          sourcePath: "C:/wg/repo-live",
          branch: "main",
        },
      ],
    });

    expect(repoBadgesForReplica(replica, session)).toEqual([
      {
        label: "live",
        branch: "main",
        sourcePath: "C:/wg/repo-live",
      },
    ]);
  });

  it("renders repo label without branch", () => {
    const replica = mkReplica({
      repoPaths: ["C:/wg/repo-AgentsCommander"],
    });

    expect(repoBadgesForReplica(replica)).toEqual([
      {
        label: "AgentsCommander",
        branch: null,
        sourcePath: "C:/wg/repo-AgentsCommander",
      },
    ]);
  });

  it("renders single discovered repo branch when available", () => {
    const replica = mkReplica({
      repoPaths: ["C:/wg/repo-AgentsCommander"],
      repoBranch: "feature/x",
    });

    expect(repoBadgesForReplica(replica)).toEqual([
      {
        label: "AgentsCommander",
        branch: "feature/x",
        sourcePath: "C:/wg/repo-AgentsCommander",
      },
    ]);
  });

  it("renders multiple discovered repos without branch", () => {
    const replica = mkReplica({
      repoPaths: ["C:/wg/repo-alpha", "C:/wg/repo-beta"],
      repoBranch: "main",
    });

    expect(repoBadgesForReplica(replica)).toEqual([
      {
        label: "alpha",
        branch: null,
        sourcePath: "C:/wg/repo-alpha",
      },
      {
        label: "beta",
        branch: null,
        sourcePath: "C:/wg/repo-beta",
      },
    ]);
  });
});
