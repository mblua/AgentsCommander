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

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

async function flushArchiveQueueStart(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
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

  it("does not render a loadProject result after an archive event lands during discover", async () => {
    const pendingDiscover = deferred<ReturnType<typeof discovery>>();
    m.discover.mockReturnValueOnce(pendingDiscover.promise);

    const load = projectStore.loadProject(PROJECT_PATH);
    await flushArchiveQueueStart();
    expect(m.open).toHaveBeenCalledWith(PROJECT_PATH);
    expect(m.discover).toHaveBeenCalledWith(PROJECT_PATH);

    const archiveEvent = projectStore.applyArchiveChange(event("archive", true));
    await flushArchiveQueueStart();

    pendingDiscover.resolve(discovery());
    await Promise.all([load, archiveEvent]);

    expect(projectStore.projects).toHaveLength(0);
    expect(projectStore.archivedPaths).toEqual([PROJECT_PATH]);
  });

  it("does not append a loadProject result while the path is already archived", async () => {
    await projectStore.initFromSettings([], null, [PROJECT_PATH]);

    await projectStore.loadProject(PROJECT_PATH);

    expect(m.discover).toHaveBeenCalledWith(PROJECT_PATH);
    expect(projectStore.projects).toHaveLength(0);
    expect(projectStore.archivedPaths).toEqual([PROJECT_PATH]);
  });

  it("does not render a createAndLoad result after an archive event lands during discover", async () => {
    const pendingDiscover = deferred<ReturnType<typeof discovery>>();
    m.discover.mockReturnValueOnce(pendingDiscover.promise);

    const create = projectStore.createAndLoad(PROJECT_PATH);
    await flushArchiveQueueStart();
    expect(m.newProject).toHaveBeenCalledWith(PROJECT_PATH);
    expect(m.discover).toHaveBeenCalledWith(PROJECT_PATH);

    const archiveEvent = projectStore.applyArchiveChange(event("archive", true));
    await flushArchiveQueueStart();

    pendingDiscover.resolve(discovery());
    await Promise.all([create, archiveEvent]);

    expect(projectStore.projects).toHaveLength(0);
    expect(projectStore.archivedPaths).toEqual([PROJECT_PATH]);
  });

  it("does not resurrect loadProject after a remove event lands during discover", async () => {
    const pendingDiscover = deferred<ReturnType<typeof discovery>>();
    m.discover.mockReturnValueOnce(pendingDiscover.promise);

    const load = projectStore.loadProject(PROJECT_PATH);
    await flushArchiveQueueStart();
    expect(m.discover).toHaveBeenCalledWith(PROJECT_PATH);

    const removeEvent = projectStore.applyArchiveChange(event("remove", false));
    await flushArchiveQueueStart();

    pendingDiscover.resolve(discovery());
    await Promise.all([load, removeEvent]);

    expect(projectStore.projects).toHaveLength(0);
    expect(projectStore.archivedPaths).toEqual([]);
  });

  it("does not re-add a project when an archive event lands during an open-event discover", async () => {
    const pendingDiscover = deferred<ReturnType<typeof discovery>>();
    m.discover.mockReturnValueOnce(pendingDiscover.promise);

    const openEvent = projectStore.applyArchiveChange(event("open", false));
    await flushArchiveQueueStart();
    expect(m.discover).toHaveBeenCalledWith(PROJECT_PATH);

    const archiveEvent = projectStore.applyArchiveChange(event("archive", true));

    pendingDiscover.resolve(discovery());
    await Promise.all([openEvent, archiveEvent]);

    expect(projectStore.projects).toHaveLength(0);
    expect(projectStore.archivedPaths).toEqual([PROJECT_PATH]);
  });

  it("applies a queued remove event after an in-flight open-event discover", async () => {
    const pendingDiscover = deferred<ReturnType<typeof discovery>>();
    m.discover.mockReturnValueOnce(pendingDiscover.promise);

    const openEvent = projectStore.applyArchiveChange(event("open", false));
    await flushArchiveQueueStart();
    expect(m.discover).toHaveBeenCalledWith(PROJECT_PATH);
    const removeEvent = projectStore.applyArchiveChange(event("remove", false));

    pendingDiscover.resolve(discovery());
    await Promise.all([openEvent, removeEvent]);

    expect(projectStore.projects).toHaveLength(0);
    expect(projectStore.archivedPaths).toEqual([]);
  });

  it("coalesces a direct unarchive and backend unarchive event into one discover", async () => {
    await projectStore.initFromSettings([], null, [PROJECT_PATH]);
    let backendEvent: Promise<void> | undefined;
    m.unarchive.mockImplementationOnce(() => {
      backendEvent = projectStore.applyArchiveChange(event("unarchive", false));
      return Promise.resolve({ path: PROJECT_PATH, registered: true, created: false });
    });

    await projectStore.unarchiveProject(PROJECT_PATH);
    await backendEvent;

    expect(m.discover).toHaveBeenCalledTimes(1);
    expect(projectStore.projects).toHaveLength(1);
    expect(projectStore.projects[0].path).toBe(PROJECT_PATH);
    expect(projectStore.archivedPaths).toEqual([]);
  });

  it("keeps a concurrent archive event authoritative while direct unarchive is in flight", async () => {
    await projectStore.initFromSettings([], null, [PROJECT_PATH]);
    const pendingUnarchive = deferred<{ path: string; registered: boolean; created: boolean }>();
    m.unarchive.mockReturnValueOnce(pendingUnarchive.promise);

    const unarchive = projectStore.unarchiveProject(PROJECT_PATH);
    await flushArchiveQueueStart();
    expect(m.unarchive).toHaveBeenCalledWith(PROJECT_PATH);

    const archiveEvent = projectStore.applyArchiveChange(event("archive", true));

    pendingUnarchive.resolve({ path: PROJECT_PATH, registered: true, created: false });
    await Promise.all([unarchive, archiveEvent]);

    expect(projectStore.projects).toHaveLength(0);
    expect(projectStore.archivedPaths).toEqual([PROJECT_PATH]);
  });

  it("does not append after a direct archiveProject completes during an open-event discover", async () => {
    const pendingDiscover = deferred<ReturnType<typeof discovery>>();
    m.discover.mockReturnValueOnce(pendingDiscover.promise);

    const openEvent = projectStore.applyArchiveChange(event("open", false));
    await flushArchiveQueueStart();
    expect(m.discover).toHaveBeenCalledWith(PROJECT_PATH);
    const archive = projectStore.archiveProject(PROJECT_PATH);

    pendingDiscover.resolve(discovery());
    await Promise.all([openEvent, archive]);

    expect(projectStore.projects).toHaveLength(0);
    expect(projectStore.archivedPaths).toEqual([PROJECT_PATH]);
  });
});
