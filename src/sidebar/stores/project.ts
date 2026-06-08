import { createSignal } from "solid-js";
import type { AcWorkgroup, AcAgentMatrix, AcTeam } from "../../shared/types";
import { ProjectAPI, SettingsAPI, AgentCreatorAPI } from "../../shared/ipc";
import {
  findLoadedProjectPathForRefresh,
  normalizeProjectPathForCompare,
} from "./project-refresh";

export interface ProjectState {
  path: string;
  folderName: string;
  workgroups: AcWorkgroup[];
  agents: AcAgentMatrix[];
  teams: AcTeam[];
}

const [projects, setProjects] = createSignal<ProjectState[]>([]);
const [loading, setLoading] = createSignal(false);
const inFlightLoads = new Map<string, Promise<void>>();
const inFlightReloads = new Map<string, Promise<void>>();
let loadingCount = 0;

function normalizePath(p: string): string {
  return normalizeProjectPathForCompare(p);
}

export const projectStore = {
  /** All loaded projects */
  get projects() {
    return projects();
  },

  /** Legacy single-project accessor (first project or null) */
  get current() {
    return projects()[0] ?? null;
  },

  get isLoading() {
    return loading();
  },

  /** Register a project path in settings (via shared backend) and load its discovery data. */
  async loadProject(path: string) {
    const normalized = normalizePath(path);
    if (projects().some((p) => normalizePath(p.path) === normalized)) return;
    const existing = inFlightLoads.get(normalized);
    if (existing) return existing;

    const promise = (async () => {
      loadingCount++;
      setLoading(true);
      try {
        // #191 — backend owns the validation + dedup + persist atomically.
        // Throws if `.ac/` is missing; caller (createAndLoad / pickAndCheck)
        // is responsible for creating it first via projectStore.createAndLoad
        // when that case is expected.
        const reg = await ProjectAPI.open(path);
        const result = await ProjectAPI.discover(reg.path);
        const folderName =
          reg.path.replace(/\\/g, "/").split("/").pop() ?? "unknown";
        // Round-1 G2: re-check against the BACKEND-absolutised reg.path
        // (which may differ from the input `path` in case/slashes/`..`),
        // mirroring the inner dedup pattern in createAndLoad. Closes the
        // double-render race when two concurrent calls pass differently-
        // shaped strings that resolve to the same registered entry.
        const normalizedReg = normalizePath(reg.path);
        setProjects((prev) => {
          if (prev.some((p) => normalizePath(p.path) === normalizedReg)) return prev;
          return [
            ...prev,
            {
              path: reg.path,
              folderName,
              workgroups: result.workgroups,
              agents: result.agents,
              teams: result.teams,
            },
          ];
        });
      } catch (e) {
        // Round-1 G11 deferred: surface this to the user via toast/sidebar
        // chip in a follow-up. For now, preserve the existing swallow-and-log
        // so behaviour is no worse than today (initFromSettings silently drops
        // a project whose Project AC Root was deleted between sessions.
        console.error("Failed to load project:", e);
      } finally {
        loadingCount--;
        if (loadingCount === 0) setLoading(false);
        inFlightLoads.delete(normalized);
      }
    })();
    inFlightLoads.set(normalized, promise);
    return promise;
  },

  /** Initialize from saved settings (call on mount) */
  async initFromSettings(projectPaths: string[], legacyPath: string | null) {
    // Merge legacy single path into the array (deduplicated)
    const paths = [...projectPaths];
    if (legacyPath && !paths.some((p) => normalizePath(p) === normalizePath(legacyPath))) {
      paths.push(legacyPath);
    }
    for (const path of paths) {
      await projectStore.loadProject(path);
    }
  },

  /** Create `.ac/` Project AC Root in path if missing and register/load it. */
  async createAndLoad(path: string) {
    const reg = await ProjectAPI.new(path);
    // After ensuring Project AC Root exists and persistence is set, run discovery for UI.
    const result = await ProjectAPI.discover(reg.path);
    const folderName =
      reg.path.replace(/\\/g, "/").split("/").pop() ?? "unknown";
    const normalized = normalizePath(reg.path);
    setProjects((prev) => {
      if (prev.some((p) => normalizePath(p.path) === normalized)) return prev;
      return [
        ...prev,
        {
          path: reg.path,
          folderName,
          workgroups: result.workgroups,
          agents: result.agents,
          teams: result.teams,
        },
      ];
    });
  },

  /** Full open flow: pick folder, check Project AC Root, auto-load if found */
  async pickAndCheck(): Promise<{ picked: string | null; hasWorkspace: boolean }> {
    const picked = await AgentCreatorAPI.pickFolder();
    if (!picked) return { picked: null, hasWorkspace: false };

    const hasWorkspace = await ProjectAPI.checkPath(picked);
    if (hasWorkspace) {
      await projectStore.loadProject(picked);
    }
    return { picked, hasWorkspace };
  },

  /** Update a replica's branch from the discovery branch watcher */
  updateReplicaBranch(replicaPath: string, branch: string | null) {
    setProjects((prev) =>
      prev.map((proj) => ({
        ...proj,
        workgroups: proj.workgroups.map((wg) => ({
          ...wg,
          agents: wg.agents.map((a) =>
            a.path === replicaPath
              ? { ...a, repoBranch: branch ?? undefined }
              : a
          ),
        })),
      }))
    );
  },

  /** Update a workgroup's TASK.md fields from the discovery watcher. */
  updateWorkgroupTask(
    workgroupPath: string,
    task: string | null,
    taskTitle: string | null | undefined
  ) {

    const normalized = normalizePath(workgroupPath);
    setProjects((prev) =>
      prev.map((proj) => ({
        ...proj,
        workgroups: proj.workgroups.map((wg) =>
          normalizePath(wg.path) === normalized
            ? { ...wg, task, taskTitle }

            : wg
        ),
      }))
    );
  },

  /** Re-discover a single project and update its data in place */
  async reloadProject(path: string) {
    const normalized = normalizePath(path);
    const existing = inFlightReloads.get(normalized);
    if (existing) return existing;

    const promise = (async () => {
      try {
        const result = await ProjectAPI.discover(path);
        setProjects((prev) =>
          prev.map((p) =>
            normalizePath(p.path) === normalized
              ? { ...p, workgroups: result.workgroups, agents: result.agents, teams: result.teams }
              : p
          )
        );
      } catch (e) {
        console.error("Failed to reload project:", e);
      } finally {
        inFlightReloads.delete(normalized);
      }
    })();
    inFlightReloads.set(normalized, promise);
    return promise;
  },

  /** Re-discover a project only when it is already loaded in the sidebar. */
  async reloadProjectIfLoaded(path: string): Promise<boolean> {
    const loadedPath = findLoadedProjectPathForRefresh(projects(), path);
    if (!loadedPath) return false;
    await projectStore.reloadProject(loadedPath);
    return true;
  },

  /** Remove a project from the list by path */
  async removeProject(path: string) {
    const normalized = normalizePath(path);
    setProjects((prev) => prev.filter((p) => normalizePath(p.path) !== normalized));
    await persistProjectPaths();
  },

  clear() {
    setProjects([]);
    loadingCount = 0;
    setLoading(false);
    inFlightLoads.clear();
    inFlightReloads.clear();
  },
};

/** Persist current project paths to settings */
async function persistProjectPaths() {
  const fresh = await SettingsAPI.get();
  const paths = projects().map((p) => p.path);
  await SettingsAPI.update({
    ...fresh,
    projectPaths: paths,
    projectPath: paths[0] ?? null, // backward compat
  });
}
