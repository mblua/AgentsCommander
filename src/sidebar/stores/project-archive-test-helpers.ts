import { vi } from "vitest";
import type { TaskSnapshot } from "../../shared/types";

type ProjectMethods = Record<"open" | "newProject" | "discover" | "remove" | "archive" | "unarchive", ReturnType<typeof vi.fn>>;

export function createArchiveIpcMock({ projectMethods }: { projectMethods: ProjectMethods }) {
  return {
    getTransportConnectionState: () => ({ state: "connected", generation: 0 }),
    TaskAPI: {
      getSnapshotAt: vi.fn(async (workgroupRoot: string): Promise<TaskSnapshot> => ({
        workgroupRoot,
        task: null,
        taskTitle: null,
        description: "",
        status: null,
        revision: "legacy:0",
        statusRecord: null,
        tailIncomplete: false,
      })),
    },
    ProjectAPI: {
      open: projectMethods.open,
      new: projectMethods.newProject,
      discover: projectMethods.discover,
      remove: projectMethods.remove,
      archive: projectMethods.archive,
      unarchive: projectMethods.unarchive,
    },
    AgentCreatorAPI: { pickFolder: vi.fn() },
  };
}

export function createArchiveTestFixture({}: Record<string, never>) {
  const projectMethods = {
    open: vi.fn(),
    newProject: vi.fn(),
    discover: vi.fn(),
    remove: vi.fn(),
    archive: vi.fn(),
    unarchive: vi.fn(),
  };
  const ipcMock = createArchiveIpcMock({ projectMethods });
  return { projectMethods, ipcMock };
}
