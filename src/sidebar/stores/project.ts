import { createSignal } from "solid-js";
import type { AcWorkgroup, AcAgentMatrix, AcTeam, AcLoopSummary } from "../../shared/types";
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
  loops: AcLoopSummary[];
}

const [projects, setProjects] = createSignal<ProjectState[]>([]);
const [loading, setLoading] = createSignal(false);
const [lastLoadError, setLastLoadError] = createSignal<string | null>(null);
const [initState, setInitState] = createSignal<{ attempted: boolean; pathCount: number }>({
  attempted: false,
  pathCount: 0,
});
const inFlightLoads = new Map<string, Promise<void>>();
const inFlightReloads = new Map<string, Promise<void>>();
const queuedReloads = new Set<string>();
let loadingCount = 0;

function normalizePath(p: string): string {
  return normalizeProjectPathForCompare(p);
}

/** Stringify whatever a rejected Tauri command throws (usually the Err string). */
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

  /** Last error from a failed loadProject(). Surfaced as a sidebar status chip
   *  (deferred Round-1 G11) and, critically, to the UI-automation surface so a
   *  swallowed backend open_project/discover_project failure is observable
   *  without devtools (#384 empty-tree triage). */
  get lastLoadError() {
    return lastLoadError();
  },

  /** Whether initFromSettings() ran, and how many paths it received. Lets the
   *  empty-tree diagnostic distinguish "onMount never reached initFromSettings"
   *  (attempted=false) from "ran but projectPaths was empty" (pathCount=0). */
  get initState() {
    return initState();
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
              loops: result.loops,
            },
          ];
        });
        setLastLoadError(null);
      } catch (e) {
        // Round-1 G11: surface the failure instead of only logging it. The
        // sidebar status chip + UI-automation `project.loadStatus` target now
        // expose this so a swallowed open_project/discover_project error is
        // diagnosable without devtools (previously it silently dropped a
        // project whose Project AC Root was deleted between sessions).
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

  /** Initialize from saved settings (call on mount) */
  async initFromSettings(projectPaths: string[], legacyPath: string | null) {
    // Merge legacy single path into the array (deduplicated)
    const paths = [...projectPaths];
    if (legacyPath && !paths.some((p) => normalizePath(p) === normalizePath(legacyPath))) {
      paths.push(legacyPath);
    }
    // Record that boot-time load ran and with how many paths, so an empty tree
    // can be triaged (onMount never got here vs. ran with zero paths).
    setInitState({ attempted: true, pathCount: paths.length });
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
          loops: result.loops,
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

  /** #552/#580 patch a coordinator replica's lastUserMessageAt (which now carries
   *  the unified team-idle anchor, #580 — "team idle since", not just the user's
   *  last message) from the clock event. Match by NORMALIZED path (required, not
   *  a deviation: the event path is the session working_directory, which can
   *  differ in slash/case from the discovery path on Windows). Discovery reload
   *  self-heals on any miss. */
  updateCoordinatorClock(replicaPath: string, lastUserMessageAt: string) {
    const target = normalizePath(replicaPath);
    setProjects((prev) =>
      prev.map((proj) => ({
        ...proj,
        workgroups: proj.workgroups.map((wg) => ({
          ...wg,
          agents: wg.agents.map((a) =>
            normalizePath(a.path) === target ? { ...a, lastUserMessageAt } : a
          ),
        })),
      }))
    );
  },

  /** #552 patch a coordinator replica's autoClosedAt from the auto-close event.
   *  A string sets the marker (auto-closed); `null` clears it (reopen). Matched
   *  by normalized path, same as updateCoordinatorClock. */
  updateCoordinatorAutoClosed(replicaPath: string, autoClosedAt: string | null) {
    const target = normalizePath(replicaPath);
    setProjects((prev) =>
      prev.map((proj) => ({
        ...proj,
        workgroups: proj.workgroups.map((wg) => ({
          ...wg,
          agents: wg.agents.map((a) =>
            normalizePath(a.path) === target
              ? { ...a, autoClosedAt: autoClosedAt ?? undefined }
              : a
          ),
        })),
      }))
    );
  },

  /** #588 patch a coordinator replica's manuallyClosedAt from the event.
   *  A string sets the marker; `null` clears it (reopen). Matched by normalized
   *  path, same as updateCoordinatorAutoClosed. */
  updateCoordinatorManuallyClosed(replicaPath: string, manuallyClosedAt: string | null) {
    const target = normalizePath(replicaPath);
    setProjects((prev) =>
      prev.map((proj) => ({
        ...proj,
        workgroups: proj.workgroups.map((wg) => ({
          ...wg,
          agents: wg.agents.map((a) =>
            normalizePath(a.path) === target
              ? { ...a, manuallyClosedAt: manuallyClosedAt ?? undefined }
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

  /** Apply a Loop summary returned by a mutation/event without waiting for discovery. */
  upsertLoop(projectPath: string, loop: AcLoopSummary) {
    const normalized = normalizePath(projectPath);
    setProjects((prev) =>
      prev.map((proj) => {
        if (normalizePath(proj.path) !== normalized) return proj;
        const existingIndex = proj.loops.findIndex((candidate) => candidate.id === loop.id);
        const loops =
          existingIndex === -1
            ? [...proj.loops, loop]
            : proj.loops.map((candidate) => (candidate.id === loop.id ? loop : candidate));
        return { ...proj, loops };
      })
    );
  },

  /** Remove a Loop summary returned by a delete event without waiting for discovery. */
  removeLoop(projectPath: string, loopId: string) {
    const normalized = normalizePath(projectPath);
    setProjects((prev) =>
      prev.map((proj) =>
        normalizePath(proj.path) === normalized
          ? { ...proj, loops: proj.loops.filter((loop) => loop.id !== loopId) }
          : proj
      )
    );
  },

  /** Re-discover a single project and update its data in place */
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
              // A mutation/event may have already applied fresher summary data while this
              // discovery was awaiting. Let the queued discovery provide the next full state.
              continue;
            }
            setProjects((prev) =>
              prev.map((p) =>
                normalizePath(p.path) === normalized
                  ? {
                      ...p,
                      workgroups: result.workgroups,
                      agents: result.agents,
                      teams: result.teams,
                      loops: result.loops,
                    }
                  : p
              )
            );
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
    setLastLoadError(null);
    setInitState({ attempted: false, pathCount: 0 });
    inFlightLoads.clear();
    inFlightReloads.clear();
    queuedReloads.clear();
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
