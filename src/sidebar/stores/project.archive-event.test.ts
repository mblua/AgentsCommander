import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ArchiveChangeReason, ProjectArchiveChanged } from "../../shared/types";

const m = vi.hoisted(() => ({
  open: vi.fn(),
  newProject: vi.fn(),
  discover: vi.fn(),
  remove: vi.fn(),
  archive: vi.fn(),
  unarchive: vi.fn(),
}));

vi.mock("../../shared/ipc", () => ({
  ProjectAPI: {
    open: m.open,
    new: m.newProject,
    discover: m.discover,
    remove: m.remove,
    archive: m.archive,
    unarchive: m.unarchive,
  },
  AgentCreatorAPI: {
    pickFolder: vi.fn(),
  },
}));

import { projectStore } from "./project";
import { replicaVolatileStore } from "./replica-volatile";

const PROJECT_PATH = "C:\\Users\\Maria\\Project";
const REPLICA_PATH = `${PROJECT_PATH}\\.ac\\wg-1-dev-team\\__agent_dev-webpage-ui`;

function discovery() {
  return {
    workgroups: [
      {
        name: "wg-1-dev-team",
        path: `${PROJECT_PATH}\\.ac\\wg-1-dev-team`,
        task: null,
        taskTitle: null,
        agents: [
          {
            name: "dev-webpage-ui",
            path: REPLICA_PATH,
            repoPaths: [],
            isCoordinator: true,
          },
        ],
      },
    ],
    agents: [],
    teams: [],
    loops: [],
    contextTemplateUpdates: [],
  };
}

function event(
  reason: ArchiveChangeReason,
  archived: boolean,
  path = PROJECT_PATH,
): ProjectArchiveChanged {
  return {
    path,
    folderName: "Project",
    archived,
    reason,
    sessionName: reason === "autoUnarchive" ? "wg-1-dev-team/dev-webpage-ui" : undefined,
  };
}

describe("projectStore archive event reconciliation (#881)", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    projectStore.clear();
    m.open.mockResolvedValue({ path: PROJECT_PATH, registered: false, created: false });
    m.newProject.mockResolvedValue({ path: PROJECT_PATH, registered: true, created: false });
    m.discover.mockResolvedValue(discovery());
    m.remove.mockResolvedValue(undefined);
    m.archive.mockResolvedValue(undefined);
    m.unarchive.mockResolvedValue({ path: PROJECT_PATH, registered: true, created: false });
  });

  afterEach(() => {
    projectStore.clear();
    vi.restoreAllMocks();
  });

  it("keeps the archive echo idempotent after archiveProject already mutated the store", async () => {
    const clearSpy = vi.spyOn(replicaVolatileStore, "clearForPaths");
    await projectStore.loadProject(PROJECT_PATH);
    clearSpy.mockClear();

    await projectStore.archiveProject(PROJECT_PATH);
    expect(projectStore.projects).toHaveLength(0);
    expect(projectStore.archivedPaths).toEqual([PROJECT_PATH]);
    expect(clearSpy).toHaveBeenCalledTimes(1);

    await projectStore.applyArchiveChange(event("archive", true));
    await projectStore.applyArchiveChange(event("archive", true));

    expect(projectStore.projects).toHaveLength(0);
    expect(projectStore.archivedPaths).toEqual([PROJECT_PATH]);
    expect(clearSpy).toHaveBeenCalledTimes(1);
  });

  it("is idempotent for archive, unarchive, autoUnarchive, open, and remove reasons", async () => {
    const clearSpy = vi.spyOn(replicaVolatileStore, "clearForPaths");

    await projectStore.loadProject(PROJECT_PATH);
    clearSpy.mockClear();
    await projectStore.applyArchiveChange(event("archive", true));
    await projectStore.applyArchiveChange(event("archive", true));
    expect(projectStore.projects).toHaveLength(0);
    expect(projectStore.archivedPaths).toEqual([PROJECT_PATH]);
    expect(clearSpy).toHaveBeenCalledTimes(1);

    for (const reason of ["unarchive", "autoUnarchive", "open"] as const) {
      projectStore.clear();
      m.discover.mockClear();
      await projectStore.initFromSettings([], null, [PROJECT_PATH]);

      await projectStore.applyArchiveChange(event(reason, false));
      await projectStore.applyArchiveChange(event(reason, false));

      expect(projectStore.archivedPaths).toEqual([]);
      expect(projectStore.projects).toHaveLength(1);
      expect(projectStore.projects[0].path).toBe(PROJECT_PATH);
      expect(m.discover).toHaveBeenCalledTimes(1);
      expect(m.discover).toHaveBeenCalledWith(PROJECT_PATH);
    }

    projectStore.clear();
    await projectStore.initFromSettings([], null, [PROJECT_PATH]);
    await projectStore.loadProject(PROJECT_PATH);
    await projectStore.applyArchiveChange(event("remove", false));
    await projectStore.applyArchiveChange(event("remove", false));

    expect(projectStore.projects).toHaveLength(0);
    expect(projectStore.archivedPaths).toEqual([]);
  });
});
