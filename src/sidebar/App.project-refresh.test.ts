import { describe, expect, it, vi } from "vitest";
import type { AcProjectRefreshReason } from "../shared/types";

vi.mock("./stores/project", () => ({
  projectStore: {
    loadProject: vi.fn(),
    reloadProjectIfLoaded: vi.fn(),
  },
}));

import { handleProjectRefreshRequested } from "./project-refresh-handler";

function refreshPayload(reason: AcProjectRefreshReason) {
  return {
    id: "request-1",
    projectPath: "C:\\Users\\Maria\\Project",
    changedPath: null,
    changedName: null,
    reason,
  };
}

describe("handleProjectRefreshRequested", () => {
  it("loads registered projects", () => {
    const store = {
      loadProject: vi.fn(),
      reloadProjectIfLoaded: vi.fn(),
    };

    handleProjectRefreshRequested(refreshPayload("projectRegistered"), store);

    expect(store.loadProject).toHaveBeenCalledWith("C:\\Users\\Maria\\Project");
    expect(store.reloadProjectIfLoaded).not.toHaveBeenCalled();
  });

  it.each([
    "createAgentMatrix",
    "workgroupCreated",
    "workgroupRemoved",
    "teamMembershipChanged",
    "teamMembershipRemoved",
    "futureReason",
  ] as const)("reloads loaded projects for %s", (reason) => {
    const store = {
      loadProject: vi.fn(),
      reloadProjectIfLoaded: vi.fn(),
    };

    handleProjectRefreshRequested(refreshPayload(reason), store);

    expect(store.reloadProjectIfLoaded).toHaveBeenCalledWith("C:\\Users\\Maria\\Project");
    expect(store.loadProject).not.toHaveBeenCalled();
  });
});
