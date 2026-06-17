import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

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
});
