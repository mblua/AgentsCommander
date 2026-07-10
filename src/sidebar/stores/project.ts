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
  /** #695 — pending seeded context-template updates surfaced by discovery. Each
   *  entry is a customized template whose baked default has a newer version; the
   *  sidebar resolves it through an explicit keep/overwrite modal. */
  contextTemplateUpdates: ContextTemplateUpdate[];
}

const [projects, setProjects] = createSignal<ProjectState[]>([]);
// #881: archived projects leave `projects`, but their persisted sessions must
// stay suppressed from the generic session list until an unarchive/open event.
const [archivedPaths, setArchivedPaths] = createSignal<string[]>([]);
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

/** Replica paths of every workgroup agent in a project/discovery shape — the
 *  set of paths whose live volatile overrides a fresh snapshot supersedes. */
function workgroupReplicaPaths(source: { workgroups: AcWorkgroup[] }): string[] {
  return source.workgroups.flatMap((wg) => wg.agents.map((agent) => agent.path));
}

/** Append a freshly discovered project unless an equivalent path is already
 *  loaded (Round-1 G2 dedup: re-check against the BACKEND-absolutised regPath,
 *  which may differ from the caller's input in case/slashes/`..` — closes the
 *  double-render race when two concurrent calls pass differently-shaped
 *  strings that resolve to the same registered entry). The volatile overrides
 *  this snapshot supersedes are cleared ONLY when the append actually applied:
 *  a dedup-discarded snapshot must not wipe live event overrides that are
 *  newer than the state the store kept (#748). */
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

  get archivedPaths() {
    return archivedPaths();
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
        appendDiscoveredProject(reg.path, result);
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
  async initFromSettings(
    projectPaths: string[],
    legacyPath: string | null,
    archivedProjectPaths: string[] = []
  ) {
    setArchivedPaths(archivedProjectPaths);
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
    appendDiscoveredProject(reg.path, result);
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

  // #748 — the former updateReplicaBranch / updateCoordinatorClock /
  // updateCoordinatorAutoClosed / updateCoordinatorManuallyClosed patchers
  // lived here and rebuilt EVERY project/workgroup object reference per event,
  // which made ProjectPanel's reference-keyed <For>s dispose and re-create the
  // whole clickable sidebar DOM (losing any click in flight). Those live
  // fields are now written to replicaVolatileStore (stores/replica-volatile.ts)
  // and read through its effective* accessors, so events never touch project
  // identity.

  /** Update a workgroup's TASK.md fields from the discovery watcher. Clones
   *  ONLY the project + workgroup on the matched path (#748 — unrelated rows
   *  must keep their object identity); no match or no change leaves the store
   *  value untouched. `taskTitle` is normalized null→undefined: the task event
   *  serializes an explicit null while the discovery snapshot OMITS the field
   *  (`skip_serializing_if` on task_title), so storing null would make the
   *  next reload's deepEqual see a phantom change and re-create the row. */
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

  /** Apply a Loop summary returned by a mutation/event without waiting for
   *  discovery. Identity-preserving (#748): an already-identical summary (a
   *  duplicate event) leaves the store value untouched. */
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

  /** Remove a Loop summary returned by a delete event without waiting for
   *  discovery. Identity-preserving (#748): an unknown loop id is a no-op. */
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
            // #748 — identity-preserving merge: entities the snapshot did not
            // change keep their object references (an identical snapshot is a
            // complete no-op). The volatile overrides for the replicas this
            // snapshot covers are superseded in the same batch — discovery
            // wins on reload, events re-patch after, exactly as the old
            // wholesale replace behaved — but ONLY when the project is
            // actually loaded: clearing for a snapshot the map cannot apply
            // would wipe live overrides while installing nothing.
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
    const removed = projects().find((p) => normalizePath(p.path) === normalized);
    batch(() => {
      setProjects((prev) => prev.filter((p) => normalizePath(p.path) !== normalized));
      setArchivedPaths((prev) => prev.filter((p) => normalizePath(p) !== normalized));
      // #748 — drop the removed project's live overrides so a later re-add
      // starts from its fresh discovery snapshot, not a stale live layer.
      if (removed) {
        replicaVolatileStore.clearForPaths(workgroupReplicaPaths(removed));
      }
    });
    // #778 — persist the removal through the dedicated disk-authoritative
    // command. A whole-object settings save no longer removes anything under
    // Design S (it preserves the on-disk project_paths so it can't clobber
    // CLI-registered projects), so removal must go through remove_project.
    await ProjectAPI.remove(path);
  },

  /** #881 - hide a project. The backend call runs first and its rejection
   *  propagates so a blocked archive leaves the project visible. */
  async archiveProject(path: string) {
    await ProjectAPI.archive(path);
    const normalized = normalizePath(path);
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
  },

  /** #881 - restore an archived project and load its discovery data. */
  async unarchiveProject(path: string) {
    const reg = await ProjectAPI.unarchive(path);
    const result = await ProjectAPI.discover(reg.path);
    const normalized = normalizePath(reg.path);
    batch(() => {
      setArchivedPaths((prev) => prev.filter((p) => normalizePath(p) !== normalized));
      appendDiscoveredProject(reg.path, result);
    });
  },

  /** #881 - reconcile cross-window/browser archive events. Idempotent because
   *  initiating windows receive their own backend event echoes. */
  async applyArchiveChange(event: ProjectArchiveChanged) {
    const key = normalizePath(event.path);
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
    if (projects().some((p) => normalizePath(p.path) === key)) return;
    const result = await ProjectAPI.discover(event.path);
    appendDiscoveredProject(event.path, result);
  },

  /** #695 — drop exactly one resolved pending context-template update. The key
   *  is `(projectPath, filename, currentDefaultSha256, currentFileSha256)`: the
   *  file hash is part of the key (grinch fix #5) so resolving a stale modal
   *  cannot silently remove a newer pending update queued for the same
   *  project/file/default after the user edited the template again. */
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
        // #748 — nothing matched the exact key: keep the project identity.
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
    queuedReloads.clear();
    replicaVolatileStore.clearAll();
  },
};
