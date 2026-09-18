// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { session } from "../../shared/testing/ui-harness";
import { projectStore } from "./project";
import { sessionsStore } from "./sessions";

const TEAM_A_PATH = "C:\\Project\\.ac\\wg-1-dev-team\\__agent_a";
const COORD_PATH = "C:\\Project\\.ac\\wg-1-dev-team\\__agent_coord";
const OUTSIDE_PATH = "C:\\Project\\.ac\\wg-9-other\\__agent_x";

function setShowInactive(value: boolean): void {
  if (sessionsStore.showInactive !== value) sessionsStore.toggleShowInactive();
}

type SkippedTeamInput = {
  sessionId: string;
  sessionName: string;
  sessionPath: string;
  visible?: boolean;
};

function expectNoGroupForTeam({ sessionId, sessionName, sessionPath, visible }: SkippedTeamInput): void {
  sessionsStore.setSessions([
    session({ id: sessionId, name: sessionName, workingDirectory: sessionPath }),
  ]);
  sessionsStore.setTeams([
    {
      id: "team-1",
      name: "team-1",
      members: [{ name: "a", path: TEAM_A_PATH }],
      ...(visible === undefined ? {} : { visible }),
    },
  ]);

  const grouped = sessionsStore.groupedSessions;

  expect(grouped.groups).toEqual([]);
  expect(grouped.ungrouped.map((s) => s.id)).toEqual([sessionId]);
}

describe("sessionsStore.groupedSessions", () => {
  beforeEach(() => {
    projectStore.clear();
    sessionsStore.setSessions([]);
    sessionsStore.setTeams([]);
    sessionsStore.setRepos([]);
    setShowInactive(false);
  });

  afterEach(() => {
    projectStore.clear();
    sessionsStore.setSessions([]);
    sessionsStore.setTeams([]);
    sessionsStore.setRepos([]);
    setShowInactive(false);
  });

  it("returns every session ungrouped when there are no teams", () => {
    sessionsStore.setSessions([
      session({ id: "s-a", name: "wg-1/a", workingDirectory: TEAM_A_PATH }),
      session({ id: "s-no-cwd", name: "wg-1/no-cwd", workingDirectory: "" }),
    ]);

    const grouped = sessionsStore.groupedSessions;

    expect(grouped.groups).toEqual([]);
    expect(grouped.ungrouped.map((s) => s.id)).toEqual(["s-a", "s-no-cwd"]);
  });

  it("skips a team whose visible flag is false", () => {
    expectNoGroupForTeam({
      sessionId: "s-a",
      sessionName: "wg-1/a",
      sessionPath: TEAM_A_PATH,
      visible: false,
    });
  });

  it("skips a visible team with no matching sessions while showInactive is off", () => {
    expectNoGroupForTeam({
      sessionId: "s-outside",
      sessionName: "wg-9/x",
      sessionPath: OUTSIDE_PATH,
    });
  });

  it("lists inactive members for a session-less team while showInactive is on", () => {
    sessionsStore.setSessions([
      session({ id: "s-outside", name: "wg-9/x", workingDirectory: OUTSIDE_PATH }),
    ]);
    sessionsStore.setTeams([
      {
        id: "team-1",
        name: "team-1",
        members: [
          { name: "coord", path: COORD_PATH },
          { name: "a", path: TEAM_A_PATH },
        ],
        coordinatorName: "coord",
      },
    ]);
    setShowInactive(true);

    const grouped = sessionsStore.groupedSessions;

    expect(grouped.groups).toHaveLength(1);
    expect(grouped.groups[0].coordinator?.name).toBe("coord");
    expect(grouped.groups[0].coordinator?.status).toBe("idle");
    expect(grouped.groups[0].members.map((m) => m.name)).toEqual(["a"]);
    expect(grouped.ungrouped.map((s) => s.id)).toEqual(["s-outside"]);
  });

  it("splits a team's sessions into coordinator and members by coordinatorName", () => {
    sessionsStore.setSessions([
      session({ id: "s-a", name: "wg-1/a", workingDirectory: TEAM_A_PATH }),
      session({ id: "s-coord", name: "wg-1/coord", workingDirectory: COORD_PATH }),
      session({ id: "s-outside", name: "wg-9/x", workingDirectory: OUTSIDE_PATH }),
    ]);
    sessionsStore.setTeams([
      {
        id: "team-1",
        name: "team-1",
        members: [
          { name: "a", path: TEAM_A_PATH },
          { name: "coord", path: COORD_PATH },
        ],
        coordinatorName: "coord",
      },
    ]);

    const grouped = sessionsStore.groupedSessions;

    expect(grouped.groups).toHaveLength(1);
    expect(grouped.groups[0].coordinator?.id).toBe("s-coord");
    expect(grouped.groups[0].members.map((m) => m.id)).toEqual(["s-a"]);
    expect(grouped.ungrouped.map((s) => s.id)).toEqual(["s-outside"]);
  });

  it("keeps sessions with no working directory and sessions outside member paths ungrouped", () => {
    sessionsStore.setSessions([
      session({ id: "s-a", name: "wg-1/a", workingDirectory: TEAM_A_PATH }),
      session({ id: "s-no-cwd", name: "wg-1/no-cwd", workingDirectory: "" }),
      session({ id: "s-outside", name: "wg-9/x", workingDirectory: OUTSIDE_PATH }),
    ]);
    sessionsStore.setTeams([
      {
        id: "team-1",
        name: "team-1",
        members: [{ name: "a", path: TEAM_A_PATH }],
        coordinatorName: "a",
      },
    ]);

    const grouped = sessionsStore.groupedSessions;

    expect(grouped.groups[0].coordinator?.id).toBe("s-a");
    expect(grouped.groups[0].members).toEqual([]);
    expect(grouped.ungrouped.map((s) => s.id)).toEqual(["s-no-cwd", "s-outside"]);
  });
});
