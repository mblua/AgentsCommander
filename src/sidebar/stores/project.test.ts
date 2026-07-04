import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AcAgentReplica, AcLoopSummary, AcWorkgroup } from "../../shared/types";

const m = vi.hoisted(() => ({
  open: vi.fn(),
  newProject: vi.fn(),
  discover: vi.fn(),
  removeProject: vi.fn(),
  getSettings: vi.fn(),
  updateSettings: vi.fn(),
}));

vi.mock("../../shared/ipc", () => ({
  ProjectAPI: {
    open: m.open,
    new: m.newProject,
    discover: m.discover,
    remove: m.removeProject,
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
import { effectiveAutoClosedAt, replicaVolatileStore } from "./replica-volatile";

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

  // #748 — identity preservation. Object references drive ProjectPanel's
  // reference-keyed <For>s: every reference these methods preserve is a DOM row
  // NOT disposed/re-created (and a click over it NOT lost). The #552 event
  // patch semantics (normalized-path match, set/clear) moved to
  // replicaVolatileStore — see replica-volatile.test.ts.
  describe("identity preservation (#748)", () => {
    const WG_1 = `${PROJECT_PATH}\\.ac\\wg-1-team`;
    const COORD_A = `${WG_1}\\__agent_coord-a`;
    const COORD_B = `${PROJECT_PATH}\\.ac\\wg-2-team\\__agent_coord-b`;
    // Event/task paths arrive with the session working_directory shape, which
    // can differ in slash/case from the discovery path on Windows.
    const WG_1_EVENT = "c:/users/maria/project/.ac/wg-1-team";

    function twoWorkgroups(coordAOverrides: Partial<AcAgentReplica> = {}): AcWorkgroup[] {
      return [
        makeWorkgroup("wg-1-team", [makeReplica(COORD_A, coordAOverrides)]),
        makeWorkgroup("wg-2-team", [makeReplica(COORD_B)]),
      ];
    }

    it("reloadProject with an identical snapshot keeps every object reference", async () => {
      await loadProjectWith(twoWorkgroups());
      const beforeProjects = projectStore.projects;
      const beforeProject = beforeProjects[0];

      // Fresh (deep-equal but reference-distinct) snapshot: the preserved refs
      // below must come from the merge, not from object reuse in the mock.
      m.discover.mockResolvedValueOnce({
        workgroups: twoWorkgroups(),
        agents: [],
        teams: [],
        loops: [],
      });
      await projectStore.reloadProject(PROJECT_PATH);

      expect(projectStore.projects).toBe(beforeProjects);
      expect(projectStore.projects[0]).toBe(beforeProject);
    });

    it("reloadProject re-creates only the changed entities, preserving siblings", async () => {
      await loadProjectWith(twoWorkgroups());
      const before = projectStore.projects[0];

      m.discover.mockResolvedValueOnce({
        workgroups: twoWorkgroups({ currentProfile: "B" }),
        agents: [],
        teams: [],
        loops: [],
      });
      await projectStore.reloadProject(PROJECT_PATH);

      const after = projectStore.projects[0];
      expect(after).not.toBe(before);
      expect(after.workgroups[0]).not.toBe(before.workgroups[0]);
      expect(findReplica(COORD_A)?.currentProfile).toBe("B");
      // The untouched sibling workgroup keeps its identity (its DOM survives).
      expect(after.workgroups[1]).toBe(before.workgroups[1]);
    });

    it("a dedup-discarded duplicate load does NOT wipe live volatile overrides (#748 review fix)", async () => {
      await loadProjectWith(twoWorkgroups());
      const before = projectStore.projects;
      // A live event lands after the initial load.
      replicaVolatileStore.setAutoClosedAt(COORD_A, "2026-06-19T18:05:00Z");

      // createAndLoad on the already-loaded project: discovery runs but the
      // inner dedup guard discards the snapshot — the store keeps the ORIGINAL
      // objects, so the newer live override must survive too.
      m.newProject.mockResolvedValueOnce({ path: PROJECT_PATH });
      m.discover.mockResolvedValueOnce({
        workgroups: twoWorkgroups(),
        agents: [],
        teams: [],
        loops: [],
      });
      await projectStore.createAndLoad(PROJECT_PATH);

      expect(projectStore.projects).toBe(before);
      expect(effectiveAutoClosedAt(findReplica(COORD_A)!)).toBe("2026-06-19T18:05:00Z");
    });

    it("updateWorkgroupTask normalizes an event's null taskTitle to undefined (no phantom diff vs the snapshot)", async () => {
      await loadProjectWith(twoWorkgroups());

      // The task event serializes taskTitle: null; the discovery snapshot OMITS
      // the field. Storing null would make the next reload's deepEqual see a
      // phantom change and re-create the row.
      projectStore.updateWorkgroupTask(WG_1_EVENT, "task body", null);
      const afterEvent = projectStore.projects;
      expect(afterEvent[0].workgroups[0].taskTitle).toBeUndefined();

      // A fresh deep-equal snapshot (taskTitle omitted, same task) is a no-op.
      m.discover.mockResolvedValueOnce({
        workgroups: [
          { ...makeWorkgroup("wg-1-team", [makeReplica(COORD_A)]), task: "task body" },
          makeWorkgroup("wg-2-team", [makeReplica(COORD_B)]),
        ],
        agents: [],
        teams: [],
        loops: [],
      });
      await projectStore.reloadProject(PROJECT_PATH);
      expect(projectStore.projects).toBe(afterEvent);

      // A repeat of the same event (null title again) is also a strict no-op.
      projectStore.updateWorkgroupTask(WG_1_EVENT, "task body", null);
      expect(projectStore.projects).toBe(afterEvent);
    });

    it("reloadProject supersedes volatile overrides with the fresh snapshot", async () => {
      await loadProjectWith(twoWorkgroups());
      replicaVolatileStore.setAutoClosedAt(COORD_A, "2026-06-19T18:05:00Z");
      expect(effectiveAutoClosedAt(findReplica(COORD_A)!)).toBe("2026-06-19T18:05:00Z");

      m.discover.mockResolvedValueOnce({
        workgroups: twoWorkgroups({ autoClosedAt: "2026-06-19T19:00:00Z" }),
        agents: [],
        teams: [],
        loops: [],
      });
      await projectStore.reloadProject(PROJECT_PATH);

      // Discovery wins on reload (the override is cleared), events re-patch after.
      expect(effectiveAutoClosedAt(findReplica(COORD_A)!)).toBe("2026-06-19T19:00:00Z");
    });

    it("updateWorkgroupTask clones only the matched workgroup's chain, by normalized path", async () => {
      await loadProjectWith(twoWorkgroups());
      const before = projectStore.projects;

      projectStore.updateWorkgroupTask(WG_1_EVENT, "task body", "New title");

      const after = projectStore.projects;
      expect(after).not.toBe(before);
      expect(after[0].workgroups[0]).not.toBe(before[0].workgroups[0]);
      expect(after[0].workgroups[0].taskTitle).toBe("New title");
      // The matched workgroup's agents array and the sibling workgroup keep identity.
      expect(after[0].workgroups[0].agents).toBe(before[0].workgroups[0].agents);
      expect(after[0].workgroups[1]).toBe(before[0].workgroups[1]);
    });

    it("updateWorkgroupTask is a strict no-op on no match or no change", async () => {
      await loadProjectWith(twoWorkgroups());
      const before = projectStore.projects;

      projectStore.updateWorkgroupTask("C:\\elsewhere\\wg-9", "task", "Title");
      expect(projectStore.projects).toBe(before);

      projectStore.updateWorkgroupTask(WG_1_EVENT, "task body", "Same title");
      const mid = projectStore.projects;
      projectStore.updateWorkgroupTask(WG_1_EVENT, "task body", "Same title");
      expect(projectStore.projects).toBe(mid);
    });

    it("upsertLoop and removeLoop are strict no-ops for identical or unknown data", async () => {
      const loop: AcLoopSummary = {
        id: "standup",
        name: "Standup",
        enabled: false,
        expr: "0 9 * * 1-5",
        timezone: "local",
        targetKind: "workgroupCoordinator",
        workgroup: "wg-1-team",
        promptPreview: "preview",
        busyCoordinator: "skip",
        path: `${PROJECT_PATH}\\.ac\\_loop_standup`,
        configPath: `${PROJECT_PATH}\\.ac\\_loop_standup\\config.toml`,
        lastCheckedAt: null,
        lastDueAt: null,
        lastDeliveredAt: null,
        lastResult: null,
        pendingDueAt: null,
        lastMissedClosedAt: null,
        nextDueAt: null,
      };
      await loadProjectWith(twoWorkgroups());

      projectStore.upsertLoop(PROJECT_PATH, loop);
      const withLoop = projectStore.projects;
      expect(withLoop[0].loops).toHaveLength(1);

      // Duplicate event with a deep-equal summary: identity untouched.
      projectStore.upsertLoop(PROJECT_PATH, { ...loop });
      expect(projectStore.projects).toBe(withLoop);

      // Unknown loop id: identity untouched.
      projectStore.removeLoop(PROJECT_PATH, "nope");
      expect(projectStore.projects).toBe(withLoop);

      projectStore.removeLoop(PROJECT_PATH, "standup");
      expect(projectStore.projects[0].loops).toHaveLength(0);
    });
  });

  // #778 — project removal routes through the dedicated disk-authoritative
  // remove_project command, NOT a whole-object settings save (which under
  // Design S preserves the on-disk list and would no longer remove anything).
  describe("removeProject (#778)", () => {
    it("routes removal through remove_project and never whole-object saves", async () => {
      await loadProjectWith([]);
      expect(projectStore.projects).toHaveLength(1);
      m.removeProject.mockResolvedValueOnce(undefined);

      await projectStore.removeProject(PROJECT_PATH);

      // In-memory row is gone...
      expect(projectStore.projects).toHaveLength(0);
      // ...persisted via the dedicated command with the raw absolute path...
      expect(m.removeProject).toHaveBeenCalledTimes(1);
      expect(m.removeProject).toHaveBeenCalledWith(PROJECT_PATH);
      // ...and NEVER via a whole-object settings save (the retired clobber path).
      expect(m.updateSettings).not.toHaveBeenCalled();
    });

    it("still calls remove_project for a path not in the store (backend no-op)", async () => {
      m.removeProject.mockResolvedValueOnce(undefined);

      await projectStore.removeProject("C:\\not\\loaded");

      expect(m.removeProject).toHaveBeenCalledWith("C:\\not\\loaded");
      expect(m.updateSettings).not.toHaveBeenCalled();
    });
  });
});
