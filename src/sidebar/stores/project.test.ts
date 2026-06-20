import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AcAgentReplica, AcWorkgroup } from "../../shared/types";

const m = vi.hoisted(() => ({
  open: vi.fn(),
  discover: vi.fn(),
  getSettings: vi.fn(),
  updateSettings: vi.fn(),
}));

vi.mock("../../shared/ipc", () => ({
  ProjectAPI: {
    open: m.open,
    discover: m.discover,
  },
  SettingsAPI: {
    get: m.getSettings,
    update: m.updateSettings,
  },
  AgentCreatorAPI: {
    pickFolder: vi.fn(),
  },
}));

import { projectStore } from "./project";

const PROJECT_PATH = "C:\\Users\\Maria\\Project";

function makeReplica(
  path: string,
  overrides: Partial<AcAgentReplica> = {}
): AcAgentReplica {
  return {
    name: path.split(/[\\/]/).pop() ?? "agent",
    path,
    repoPaths: [],
    isCoordinator: true,
    ...overrides,
  };
}

function makeWorkgroup(name: string, agents: AcAgentReplica[]): AcWorkgroup {
  return { name, path: `${PROJECT_PATH}\\.ac\\${name}`, task: null, agents };
}

/** Drive the real loadProject() path so the replicas land in the store exactly
 *  as discovery would deliver them, then patch them via the methods under test. */
async function loadProjectWith(workgroups: AcWorkgroup[]) {
  m.discover.mockResolvedValueOnce({ workgroups, agents: [], teams: [], loops: [] });
  await projectStore.loadProject(PROJECT_PATH);
}

/** Locate a replica by its ORIGINAL (discovery) path across the loaded tree. */
function findReplica(path: string): AcAgentReplica | undefined {
  for (const proj of projectStore.projects) {
    for (const wg of proj.workgroups) {
      const found = wg.agents.find((a) => a.path === path);
      if (found) return found;
    }
  }
  return undefined;
}

describe("projectStore", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    projectStore.clear();
    m.open.mockResolvedValue({ path: "C:\\Users\\Maria\\Project" });
    m.discover.mockResolvedValue({ workgroups: [], agents: [], teams: [], loops: [] });
  });

  afterEach(() => {
    projectStore.clear();
  });

  it("shares concurrent loadProject calls for equivalent paths", async () => {
    await Promise.all([
      projectStore.loadProject("C:\\Users\\Maria\\Project\\"),
      projectStore.loadProject("c:/users/maria/project"),
      projectStore.loadProject("\\\\?\\C:\\Users\\Maria\\Project"),
    ]);

    expect(m.open).toHaveBeenCalledTimes(1);
    expect(m.discover).toHaveBeenCalledTimes(1);
    expect(projectStore.projects).toHaveLength(1);
    expect(projectStore.projects[0].path).toBe("C:\\Users\\Maria\\Project");
    expect(projectStore.projects[0].loops).toEqual([]);
  });

  it("records lastLoadError when a backend command rejects (empty-tree triage)", async () => {
    m.open.mockRejectedValueOnce("Path is not a directory: C:\\nope");

    await projectStore.loadProject("C:\\nope");

    expect(projectStore.projects).toHaveLength(0);
    expect(projectStore.lastLoadError).toBe("Path is not a directory: C:\\nope");
  });

  it("clears lastLoadError after a subsequent successful load", async () => {
    m.open.mockRejectedValueOnce("boom");
    await projectStore.loadProject("C:\\Users\\Maria\\Project");
    expect(projectStore.lastLoadError).toBe("boom");

    await projectStore.loadProject("C:\\Users\\Maria\\Project");
    expect(projectStore.projects).toHaveLength(1);
    expect(projectStore.lastLoadError).toBeNull();
  });

  it("records initState so an empty tree can be triaged", async () => {
    expect(projectStore.initState).toEqual({ attempted: false, pathCount: 0 });

    await projectStore.initFromSettings(["C:\\Users\\Maria\\Project"], null);

    expect(projectStore.initState).toEqual({ attempted: true, pathCount: 1 });
  });

  it("clear() resets the load diagnostics", async () => {
    m.open.mockRejectedValueOnce("boom");
    await projectStore.initFromSettings(["C:\\bad"], null);
    expect(projectStore.lastLoadError).toBe("boom");
    expect(projectStore.initState.attempted).toBe(true);

    projectStore.clear();

    expect(projectStore.lastLoadError).toBeNull();
    expect(projectStore.initState).toEqual({ attempted: false, pathCount: 0 });
  });

  // #552 coordinator idle badge + auto-closed pill. Both patch methods match by
  // NORMALIZED path: the event carries the session working_directory, which can
  // differ in slash/case from the discovery path on Windows. These cover that
  // load-bearing match plus the field-level set/clear semantics.
  describe("coordinator clock + auto-closed patches (#552)", () => {
    const COORD_A = `${PROJECT_PATH}\\.ac\\wg-1-team\\__agent_coord-a`;
    const COORD_B = `${PROJECT_PATH}\\.ac\\wg-2-team\\__agent_coord-b`;
    // Same replica as COORD_A but lower-cased with forward slashes, as the
    // backend session working_directory arrives on the clock/auto-close events.
    const COORD_A_EVENT = "c:/users/maria/project/.ac/wg-1-team/__agent_coord-a";

    it("updateCoordinatorClock patches the matching replica by normalized path and leaves others untouched", async () => {
      await loadProjectWith([
        makeWorkgroup("wg-1-team", [
          // A carries a pre-existing auto-closed marker to prove the clock patch
          // preserves sibling fields (it must only touch lastUserMessageAt).
          makeReplica(COORD_A, { autoClosedAt: "2026-06-19T17:00:00Z" }),
          makeReplica(COORD_B, { lastUserMessageAt: "2020-01-01T00:00:00Z" }),
        ]),
      ]);

      projectStore.updateCoordinatorClock(COORD_A_EVENT, "2026-06-19T18:00:00Z");

      const a = findReplica(COORD_A);
      expect(a?.lastUserMessageAt).toBe("2026-06-19T18:00:00Z");
      expect(a?.autoClosedAt).toBe("2026-06-19T17:00:00Z"); // sibling field preserved
      // Non-matching replica is left exactly as discovered.
      expect(findReplica(COORD_B)?.lastUserMessageAt).toBe("2020-01-01T00:00:00Z");
    });

    it("updateCoordinatorAutoClosed sets autoClosedAt from a string by normalized path and leaves others untouched", async () => {
      await loadProjectWith([
        makeWorkgroup("wg-1-team", [
          makeReplica(COORD_A),
          makeReplica(COORD_B),
        ]),
      ]);

      projectStore.updateCoordinatorAutoClosed(COORD_A_EVENT, "2026-06-19T18:05:00Z");

      expect(findReplica(COORD_A)?.autoClosedAt).toBe("2026-06-19T18:05:00Z");
      expect(findReplica(COORD_B)?.autoClosedAt).toBeUndefined();
    });

    it("updateCoordinatorAutoClosed clears autoClosedAt when passed null (reopen)", async () => {
      await loadProjectWith([
        makeWorkgroup("wg-1-team", [
          makeReplica(COORD_A, { autoClosedAt: "2026-06-19T18:05:00Z" }),
        ]),
      ]);
      // Precondition: the marker is present before the clear.
      expect(findReplica(COORD_A)?.autoClosedAt).toBe("2026-06-19T18:05:00Z");

      projectStore.updateCoordinatorAutoClosed(COORD_A_EVENT, null);

      expect(findReplica(COORD_A)?.autoClosedAt).toBeUndefined();
    });
  });
});
