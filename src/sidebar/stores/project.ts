import { batch, createSignal } from "solid-js";
import type {
  AcWorkgroup,
  AcAgentMatrix,
  AcDiscoveryResult,
  AcTeam,
  AcLoopSummary,
  ContextTemplateUpdate,
  ProjectArchiveChanged,
} from "../../shared/types";
import { ProjectAPI, AgentCreatorAPI } from "../../shared/ipc";
import {
  findLoadedProjectPathForRefresh,
  normalizeProjectPathForCompare,
} from "./project-refresh";
import { deepEqual, mergeDiscoveryResult } from "./project-merge";
import { replicaVolatileStore } from "./replica-volatile";

export interface ProjectState {
  path: string;
  folderName: string;
  workgroups: AcWorkgroup[];
  agents: AcAgentMatrix[];
  teams: AcTeam[];
  loops: AcLoopSummary[];
  contextTemplateUpdates: ContextTemplateUpdate[];
}

const [projects, setProjects] = createSignal<ProjectState[]>([]);
const [archivedPaths, setArchivedPaths] = createSignal<string[]>([]);
const [loading, setLoading] = createSignal(false);
const [lastLoadError, setLastLoadError] = createSignal<string | null>(null);
const [initState, setInitState] = createSignal<{ attempted: boolean; pathCount: number }>({
  attempted: false,
  pathCount: 0,
});
const inFlightLoads = new Map<string, Promise<void>>();
const inFlightReloads = new Map<string, Promise<void>>();
const archiveChangeTails = new Map<string, Promise<void>>();
const queuedReloads = new Set<string>();
let loadingCount = 0;

function normalizePath(p: string): string {
  return normalizeProjectPathForCompare(p);
}

function workgroupReplicaPaths(source: { workgroups: AcWorkgroup[] }): string[] {
  return source.workgroups.flatMap((wg) => wg.agents.map((agent) => agent.path));
}

function hasArchivedPath(key: string): boolean {
  return archivedPaths().some((p) => normalizePath(p) === key);
}

function hasLoadedProject(key: string): boolean {
  return projects().some((p) => normalizePath(p.path) === key);
}

async function runSerializedArchiveChange(
  key: string,
  task: () => Promise<void> | void
): Promise<void> {
  const previous = archiveChangeTails.get(key) ?? Promise.resolve();
  const next = previous.catch(() => undefined).then(task);
  archiveChangeTails.set(key, next);
  try {
    await next;
  } finally {
    if (archiveChangeTails.get(key) === next) {
      archiveChangeTails.delete(key);
    }
  }
}

async function discoverAndAppendIfCurrent(path: string, key: string): Promise<void> {
  const result = await ProjectAPI.discover(path);
  if (hasArchivedPath(key)) return;
  appendDiscoveredProject(path, result);
}

function appendDiscoveredProject(regPath: string, result: AcDiscoveryResult) {
  const folderName = regPath.replace(/\\/g, "/").split("/").pop() ?? "unknown";
  const normalizedReg = normalizePath(regPath);
  batch(() => {
    let appended = false;
    setProjects((prev) => {
      if (prev.some((p) => normalizePath(p.path) === normalizedReg)) return prev;
      appended = true;
      return [
        ...prev,
        {
          path: regPath,
          folderName,
          workgroups: result.workgroups,
          agents: result.agents,
          teams: result.teams,
          loops: result.loops,
          contextTemplateUpdates: result.contextTemplateUpdates,
        },
      ];
    });
    if (appended) {
      replicaVolatileStore.clearForPaths(workgroupReplicaPaths(result));
    }
  });
}

function formatLoadError(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  try {
    return JSON.stringify(e);
  } catch {
    return String(e);
  }
}

export const projectStore = {
  get projects() {
    return projects();
  },

  get current() {
    return projects()[0] ?? null;
  },

  get archivedPaths() {
    return archivedPaths();
  },

  get isLoading() {
    return loading();
  },

  get lastLoadError() {
    return lastLoadError();
  },

  get initState() {
    return initState();
  },

  async loadProject(path: string) {
    const normalized = normalizePath(path);
    if (projects().some((p) => normalizePath(p.path) === normalized)) return;
    const existing = inFlightLoads.get(normalized);
    if (existing) return existing;

    const promise = (async () => {
      loadingCount++;
      setLoading(true);
      try {
        const reg = await ProjectAPI.open(path);
        const normalizedReg = normalizePath(reg.path);
        await runSerializedArchiveChange(normalizedReg, () =>
          discoverAndAppendIfCurrent(reg.path, normalizedReg)
        );
        setLastLoadError(null);
      } catch (e) {
        console.error("Failed to load project:", e);
        setLastLoadError(formatLoadError(e));
      } finally {
        loadingCount--;
        if (loadingCount === 0) setLoading(false);
        inFlightLoads.delete(normalized);
      }
    })();
    inFlightLoads.set(normalized, promise);
    return promise;
  },

  async initFromSettings(
    projectPaths: string[],
    legacyPath: string | null,
    archivedProjectPaths: string[] = []
  ) {
    setArchivedPaths((prev) => {
      const keys = new Set(prev.map(normalizePath));
      return [...prev, ...archivedProjectPaths.filter((p) => !keys.has(normalizePath(p)))];
    });
    const paths = [...projectPaths];
    if (legacyPath && !paths.some((p) => normalizePath(p) === normalizePath(legacyPath))) {
      paths.push(legacyPath);
    }
    setInitState({ attempted: true, pathCount: paths.length });
    for (const path of paths) {
      await projectStore.loadProject(path);
    }
  },

  async createAndLoad(path: string) {
    const reg = await ProjectAPI.new(path);
    const normalized = normalizePath(reg.path);
    await runSerializedArchiveChange(normalized, () =>
      discoverAndAppendIfCurrent(reg.path, normalized)
    );
  },

  async pickAndCheck(): Promise<{ picked: string | null; hasWorkspace: boolean }> {
    const picked = await AgentCreatorAPI.pickFolder();
    if (!picked) return { picked: null, hasWorkspace: false };

    const hasWorkspace = await ProjectAPI.checkPath(picked);
    if (hasWorkspace) {
      await projectStore.loadProject(picked);
    }
    return { picked, hasWorkspace };
  },


  updateWorkgroupTask(
    workgroupPath: string,
    task: string | null,
    taskTitle: string | null | undefined
  ) {
    const normalized = normalizePath(workgroupPath);
    const normalizedTitle = taskTitle ?? undefined;
    setProjects((prev) => {
      let anyChanged = false;
      const next = prev.map((proj) => {
        const index = proj.workgroups.findIndex(
          (wg) => normalizePath(wg.path) === normalized
        );
        if (index === -1) return proj;
        const wg = proj.workgroups[index];
        if (wg.task === task && (wg.taskTitle ?? undefined) === normalizedTitle) return proj;
        anyChanged = true;
        const workgroups = proj.workgroups.slice();
        workgroups[index] = { ...wg, task, taskTitle: normalizedTitle };
        return { ...proj, workgroups };
      });
      return anyChanged ? next : prev;
    });
  },

  upsertLoop(projectPath: string, loop: AcLoopSummary) {
    const normalized = normalizePath(projectPath);
    setProjects((prev) => {
      let anyChanged = false;
      const next = prev.map((proj) => {
        if (normalizePath(proj.path) !== normalized) return proj;
        const existingIndex = proj.loops.findIndex((candidate) => candidate.id === loop.id);
        if (existingIndex !== -1 && deepEqual(proj.loops[existingIndex], loop)) return proj;
        anyChanged = true;
        const loops =
          existingIndex === -1
            ? [...proj.loops, loop]
            : proj.loops.map((candidate) => (candidate.id === loop.id ? loop : candidate));
        return { ...proj, loops };
      });
      return anyChanged ? next : prev;
    });
  },

  removeLoop(projectPath: string, loopId: string) {
    const normalized = normalizePath(projectPath);
    setProjects((prev) => {
      let anyChanged = false;
      const next = prev.map((proj) => {
        if (normalizePath(proj.path) !== normalized) return proj;
        if (!proj.loops.some((loop) => loop.id === loopId)) return proj;
        anyChanged = true;
        return { ...proj, loops: proj.loops.filter((loop) => loop.id !== loopId) };
      });
      return anyChanged ? next : prev;
    });
  },

  async reloadProject(path: string) {
    const normalized = normalizePath(path);
    const existing = inFlightReloads.get(normalized);
    if (existing) {
      queuedReloads.add(normalized);
      return existing;
    }

    const promise = (async () => {
      try {
        do {
          queuedReloads.delete(normalized);
          try {
            const result = await ProjectAPI.discover(path);
            if (queuedReloads.has(normalized)) {
              continue;
            }
            const loaded = projects().some((p) => normalizePath(p.path) === normalized);
            batch(() => {
              setProjects((prev) => {
                let anyChanged = false;
                const next = prev.map((p) => {
                  if (normalizePath(p.path) !== normalized) return p;
                  const merged = mergeDiscoveryResult(p, result);
                  if (merged !== p) anyChanged = true;
                  return merged;
                });
                return anyChanged ? next : prev;
              });
              if (loaded) {
                replicaVolatileStore.clearForPaths(workgroupReplicaPaths(result));
              }
            });
          } catch (e) {
            console.error("Failed to reload project:", e);
          }
        } while (queuedReloads.has(normalized));
      } finally {
        inFlightReloads.delete(normalized);
        queuedReloads.delete(normalized);
      }
    })();
    inFlightReloads.set(normalized, promise);
    return promise;
  },

  async reloadProjectIfLoaded(path: string): Promise<boolean> {
    const loadedPath = findLoadedProjectPathForRefresh(projects(), path);
    if (!loadedPath) return false;
    await projectStore.reloadProject(loadedPath);
    return true;
  },

  async removeProject(path: string) {
    const normalized = normalizePath(path);
    const removed = projects().find((p) => normalizePath(p.path) === normalized);
    batch(() => {
      setProjects((prev) => prev.filter((p) => normalizePath(p.path) !== normalized));
      setArchivedPaths((prev) => prev.filter((p) => normalizePath(p) !== normalized));
      if (removed) {
        replicaVolatileStore.clearForPaths(workgroupReplicaPaths(removed));
      }
    });
    await ProjectAPI.remove(path);
  },

  async archiveProject(path: string) {
    const normalized = normalizePath(path);
    await runSerializedArchiveChange(normalized, async () => {
      await ProjectAPI.archive(path);
      const archived = projects().find((p) => normalizePath(p.path) === normalized);
      batch(() => {
        setProjects((prev) => prev.filter((p) => normalizePath(p.path) !== normalized));
        setArchivedPaths((prev) =>
          prev.some((p) => normalizePath(p) === normalized) ? prev : [...prev, path]
        );
        if (archived) {
          replicaVolatileStore.clearForPaths(workgroupReplicaPaths(archived));
        }
      });
    });
  },

  async unarchiveProject(path: string) {
    const key = normalizePath(path);
    await runSerializedArchiveChange(key, async () => {
      const reg = await ProjectAPI.unarchive(path);
      const normalized = normalizePath(reg.path);
      setArchivedPaths((prev) => prev.filter((p) => normalizePath(p) !== normalized));
      if (hasLoadedProject(normalized)) return;
      await discoverAndAppendIfCurrent(reg.path, normalized);
    });
  },

  async applyArchiveChange(event: ProjectArchiveChanged) {
    const key = normalizePath(event.path);
    await runSerializedArchiveChange(key, async () => {
      if (event.archived) {
        const archived = projects().find((p) => normalizePath(p.path) === key);
        batch(() => {
          setProjects((prev) => prev.filter((p) => normalizePath(p.path) !== key));
          setArchivedPaths((prev) =>
            prev.some((p) => normalizePath(p) === key) ? prev : [...prev, event.path]
          );
          if (archived) {
            replicaVolatileStore.clearForPaths(workgroupReplicaPaths(archived));
          }
        });
        return;
      }

      if (event.reason === "remove") {
        const removed = projects().find((p) => normalizePath(p.path) === key);
        batch(() => {
          setProjects((prev) => prev.filter((p) => normalizePath(p.path) !== key));
          setArchivedPaths((prev) => prev.filter((p) => normalizePath(p) !== key));
          if (removed) {
            replicaVolatileStore.clearForPaths(workgroupReplicaPaths(removed));
          }
        });
        return;
      }

      setArchivedPaths((prev) => prev.filter((p) => normalizePath(p) !== key));
      if (hasLoadedProject(key)) return;
      await discoverAndAppendIfCurrent(event.path, key);
    });
  },

  removeContextTemplateUpdate(
    projectPath: string,
    filename: string,
    defaultSha256: string,
    fileSha256: string
  ) {
    const normalized = normalizePath(projectPath);
    setProjects((prev) => {
      let anyChanged = false;
      const next = prev.map((project) => {
        if (normalizePath(project.path) !== normalized) return project;
        const contextTemplateUpdates = project.contextTemplateUpdates.filter(
          (update) =>
            update.filename !== filename ||
            update.currentDefaultSha256 !== defaultSha256 ||
            update.currentFileSha256 !== fileSha256
        );
        if (contextTemplateUpdates.length === project.contextTemplateUpdates.length) {
          return project;
        }
        anyChanged = true;
        return { ...project, contextTemplateUpdates };
      });
      return anyChanged ? next : prev;
    });
  },

  clear() {
    setProjects([]);
    setArchivedPaths([]);
    loadingCount = 0;
    setLoading(false);
    setLastLoadError(null);
    setInitState({ attempted: false, pathCount: 0 });
    inFlightLoads.clear();
    inFlightReloads.clear();
    archiveChangeTails.clear();
    queuedReloads.clear();
    replicaVolatileStore.clearAll();
  },
};
