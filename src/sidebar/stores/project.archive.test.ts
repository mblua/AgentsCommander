import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

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

const PROJECT_PATH = "C:\\Users\\Maria\\Project";

function discovery() {
  return {
    workgroups: [],
    agents: [],
    teams: [],
    loops: [],
    contextTemplateUpdates: [],
  };
}

describe("projectStore archive operations (#881)", () => {
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
  });

  it("awaits archive_project before removing the project from the store", async () => {
    await projectStore.loadProject(PROJECT_PATH);
    expect(projectStore.projects).toHaveLength(1);

    let releaseArchive!: () => void;
    m.archive.mockReturnValueOnce(new Promise<void>((resolve) => {
      releaseArchive = resolve;
    }));

    const archivePromise = projectStore.archiveProject(PROJECT_PATH);
    expect(m.archive).toHaveBeenCalledWith(PROJECT_PATH);
    expect(projectStore.projects).toHaveLength(1);
    expect(projectStore.archivedPaths).toEqual([]);

    releaseArchive();
    await archivePromise;

    expect(projectStore.projects).toHaveLength(0);
    expect(projectStore.archivedPaths).toEqual([PROJECT_PATH]);
  });

  it("leaves the project visible when archive_project rejects", async () => {
    await projectStore.loadProject(PROJECT_PATH);
    m.archive.mockRejectedValueOnce("Cannot archive: 1 open session in this project.");

    await expect(projectStore.archiveProject(PROJECT_PATH)).rejects.toBe(
      "Cannot archive: 1 open session in this project.",
    );

    expect(projectStore.projects).toHaveLength(1);
    expect(projectStore.projects[0].path).toBe(PROJECT_PATH);
    expect(projectStore.archivedPaths).toEqual([]);
  });

  it("unarchives via the backend registration path, discovers, appends, and clears archivedPaths", async () => {
    await projectStore.initFromSettings([], null, [PROJECT_PATH]);
    expect(projectStore.archivedPaths).toEqual([PROJECT_PATH]);
    m.unarchive.mockResolvedValueOnce({
      path: "C:\\Users\\Maria\\Project",
      registered: true,
      created: false,
    });

    await projectStore.unarchiveProject("c:/users/maria/project");

    expect(m.unarchive).toHaveBeenCalledWith("c:/users/maria/project");
    expect(m.discover).toHaveBeenCalledWith(PROJECT_PATH);
    expect(projectStore.archivedPaths).toEqual([]);
    expect(projectStore.projects).toHaveLength(1);
    expect(projectStore.projects[0].path).toBe(PROJECT_PATH);
  });

  it("initFromSettings seeds archivedPaths", async () => {
    await projectStore.initFromSettings([], null, ["C:\\Archived"]);

    expect(projectStore.archivedPaths).toEqual(["C:\\Archived"]);
  });
});
