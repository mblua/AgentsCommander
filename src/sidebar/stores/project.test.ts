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
});
