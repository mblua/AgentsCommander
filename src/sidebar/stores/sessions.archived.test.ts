import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { Session } from "../../shared/types";
import { projectStore } from "./project";
import { sessionsStore } from "./sessions";

const PROJECT_PATH = "C:\\Users\\Maria\\Project";
const OUTSIDE_PATH = "C:\\Users\\Maria\\Other\\.ac\\wg-9\\__agent_dev";

function session(overrides: Partial<Session>): Session {
  return {
    id: "session-1",
    name: "wg-1-dev-team/dev-webpage-ui",
    shell: "pwsh",
    shellArgs: [],
    effectiveShellArgs: [],
    createdAt: "2026-07-10T00:00:00.000Z",
    workingDirectory: `${PROJECT_PATH}\\.ac\\wg-1-dev-team\\__agent_dev-webpage-ui`,
    status: "running",
    waitingForInput: false,
    communication: null,
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

describe("sessionsStore archived project filtering (#881)", () => {
  beforeEach(() => {
    projectStore.clear();
    sessionsStore.setSessions([]);
    sessionsStore.setTeams([]);
    sessionsStore.setRepos([]);
  });

  afterEach(() => {
    projectStore.clear();
    sessionsStore.setSessions([]);
    sessionsStore.setTeams([]);
    sessionsStore.setRepos([]);
  });

  it("hides both running and exited sessions under archived project roots", async () => {
    sessionsStore.setSessions([
      session({ id: "running", name: "wg-1/z-running", status: "running" }),
      session({ id: "exited", name: "wg-1/a-exited", status: { exited: 0 } }),
      session({
        id: "outside",
        name: "wg-9/m-outside",
        workingDirectory: OUTSIDE_PATH,
        status: "running",
      }),
    ]);

    expect(sessionsStore.filteredSessions.map((s) => s.id)).toEqual([
      "exited",
      "outside",
      "running",
    ]);

    await projectStore.initFromSettings([], null, [PROJECT_PATH]);

    expect(sessionsStore.filteredSessions.map((s) => s.id)).toEqual(["outside"]);
  });

  it("uses the project normalizer so verbatim archived paths still suppress sessions", async () => {
    sessionsStore.setSessions([session({ id: "verbatim" })]);

    await projectStore.initFromSettings([], null, ["\\\\?\\C:\\Users\\Maria\\Project\\"]);

    expect(sessionsStore.filteredSessions).toEqual([]);
  });
});
