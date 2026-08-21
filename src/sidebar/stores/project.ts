import { batch, createSignal } from "solid-js";
import type {
  AcWorkgroup,
  AcAgentMatrix,
  AcDiscoveryResult,
  AcTeam,
  AcLoopSummary,
  ContextTemplateUpdate,
  ProjectArchiveChanged,
  ProjectPathConflictIssue,
  ProjectPathIssue,
  ProjectPathIssueSource,
  ProjectPathReconciliationError,
  ProjectPathResolution,
  RawJsonFieldState,
  RawStringFieldState,
} from "../../shared/types";
import { ProjectAPI, AgentCreatorAPI } from "../../shared/ipc";
import { toastStore } from "../../shared/stores/toasts";
import {
  findLoadedProjectPathForRefresh,
  normalizeProjectPathForCompare,
} from "./project-refresh";
import { deepEqual, mergeDiscoveryResult } from "./project-merge";
// #1283 baseline cycle break (2026-08-12 plan amendment): the ProjectState
// interface moved to ./project-merge; the type-only import below creates the
// local binding used at createSignal<ProjectState[]> and the re-export keeps
// every existing consumer compiling.
import type { ProjectState } from "./project-merge";
export type { ProjectState };
import { replicaVolatileStore } from "./replica-volatile";

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
// #1077: conflict IDs already surfaced as a sticky toast. Retained across
// repeated initFromSettings runs (reconnects, remounts) so one conflict yields
// exactly one toast; cleared by the store's reset lifecycle (clear()).
const seenConflictIds = new Set<string>();
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

// ── #1077 resolution report: runtime validation + conflict presentation ──────

const INVALID_REPORT_MESSAGE =
  "The project list could not be read from settings (unexpected response). " +
  "No projects were loaded. Restart AgentsCommander or check your settings file.";

const ISSUE_ID_HEX = /^[0-9a-f]{64}$/;
const ISSUE_SOURCES: ReadonlySet<string> = new Set<ProjectPathIssueSource>([
  "projectPath",
  "projectPaths",
  "archivedProjectPaths",
]);

type ReportValidation =
  | { status: "absent" }
  | { status: "invalid" }
  | { status: "valid"; report: ProjectPathResolution };

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isSafeCount(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}

function isNullableString(value: unknown): value is string | null {
  return value === null || typeof value === "string";
}

/**
 * Iteratively validate an arbitrary value as a finite JsonValue. An explicit
 * work stack plus a visited set keep it cycle-safe for hostile/cyclic test
 * doubles and free of recursive call-stack trust: a cycle or shared reference
 * (never present in a real serialized backend response) fails closed rather
 * than looping or overflowing, and any non-finite number is rejected.
 */
function isJsonValue(root: unknown): boolean {
  const stack: unknown[] = [root];
  const seen = new Set<object>();
  while (stack.length > 0) {
    const value = stack.pop();
    if (value === null) continue;
    const kind = typeof value;
    if (kind === "string" || kind === "boolean") continue;
    if (kind === "number") {
      if (!Number.isFinite(value)) return false;
      continue;
    }
    if (Array.isArray(value)) {
      if (seen.has(value)) return false;
      seen.add(value);
      for (const item of value as unknown[]) stack.push(item);
      continue;
    }
    if (isPlainObject(value)) {
      if (seen.has(value)) return false;
      seen.add(value);
      for (const key of Object.keys(value)) stack.push(value[key]);
      continue;
    }
    return false; // undefined, function, symbol, bigint, non-plain object
  }
  return true;
}

function isRawStringFieldState(value: unknown): value is RawStringFieldState {
  if (!isPlainObject(value)) return false;
  if (typeof value.present !== "boolean") return false;
  if (value.present === false) return value.value === null;
  return isNullableString(value.value);
}

function isRawJsonFieldState(value: unknown): value is RawJsonFieldState {
  if (!isPlainObject(value)) return false;
  if (typeof value.present !== "boolean") return false;
  if (value.present === false) return value.value === null;
  return isJsonValue(value.value);
}

function isValidIssueHead(issue: Record<string, unknown>): boolean {
  if (typeof issue.id !== "string" || !ISSUE_ID_HEX.test(issue.id)) return false;
  if (typeof issue.source !== "string" || !ISSUE_SOURCES.has(issue.source)) return false;
  if (issue.index !== undefined && !isSafeCount(issue.index)) return false;
  return true;
}

function isValidIssue(value: unknown): value is ProjectPathIssue {
  if (!isPlainObject(value)) return false;
  if (!isValidIssueHead(value)) return false;
  switch (value.kind) {
    case "conflict":
      return (
        typeof value.absoluteCandidate === "string" &&
        typeof value.instanceRelativeCandidate === "string" &&
        typeof value.absoluteResolvedPath === "string" &&
        typeof value.instanceRelativeResolvedPath === "string" &&
        typeof value.message === "string"
      );
    case "missing":
      return (
        isRawStringFieldState(value.absoluteCandidate) &&
        isRawStringFieldState(value.instanceRelativeCandidate) &&
        isNullableString(value.absoluteResolvedPath) &&
        isNullableString(value.instanceRelativeResolvedPath) &&
        typeof value.message === "string"
      );
    case "invalid":
      return (
        isRawJsonFieldState(value.absoluteCandidate) &&
        isRawJsonFieldState(value.instanceRelativeCandidate) &&
        isNullableString(value.absoluteResolvedPath) &&
        isNullableString(value.instanceRelativeResolvedPath) &&
        typeof value.reason === "string"
      );
    default:
      return false; // unknown discriminant
  }
}

function isReconciliationError(
  value: unknown
): value is ProjectPathReconciliationError {
  if (!isPlainObject(value)) return false;
  if (value.stage !== "read" && value.stage !== "write") return false;
  if (typeof value.message !== "string") return false;
  return value.retryable === true;
}

/**
 * Total validator/normalizer for the transport-untrusted resolution report. An
 * absent/undefined report is the sole legacy fallback; anything present but
 * malformed fails closed so no returned project path is trusted. Returns a
 * freshly built, fully validated report — never the raw transport object.
 */
function validateProjectPathResolution(report: unknown): ReportValidation {
  if (report === undefined) return { status: "absent" };
  if (!isPlainObject(report)) return { status: "invalid" };
  if (!isSafeCount(report.activeRegistrationCount)) return { status: "invalid" };
  if (!isSafeCount(report.archivedRegistrationCount)) return { status: "invalid" };

  let reconciliationError: ProjectPathReconciliationError | null = null;
  const rawError = report.reconciliationError;
  if (rawError !== null) {
    if (!isReconciliationError(rawError)) return { status: "invalid" };
    reconciliationError = rawError;
  }

  const rawIssues = report.issues;
  if (!Array.isArray(rawIssues)) return { status: "invalid" };
  const issues: ProjectPathIssue[] = [];
  for (const candidate of rawIssues as unknown[]) {
    if (!isValidIssue(candidate)) return { status: "invalid" };
    issues.push(candidate);
  }

  return {
    status: "valid",
    report: {
      activeRegistrationCount: report.activeRegistrationCount,
      archivedRegistrationCount: report.archivedRegistrationCount,
      issues,
      reconciliationError,
    },
  };
}

function isActiveIssueSource(source: ProjectPathIssueSource): boolean {
  return source === "projectPath" || source === "projectPaths";
}

/**
 * Escape C0/C1 controls and bidi override/isolate marks to visible `\u{…}`
 * sequences without truncating printable text, iterating by code point so
 * astral characters survive intact. A hostile POSIX filename containing a
 * newline or a bidi marker therefore cannot forge a second label line.
 */
function escapeControlAndBidi(text: string): string {
  let out = "";
  for (const ch of text) {
    const cp = ch.codePointAt(0) ?? 0;
    const escape =
      cp <= 0x1f ||
      cp === 0x7f ||
      (cp >= 0x80 && cp <= 0x9f) ||
      cp === 0x061c ||
      cp === 0x200e ||
      cp === 0x200f ||
      (cp >= 0x202a && cp <= 0x202e) ||
      (cp >= 0x2066 && cp <= 0x2069);
    out += escape
      ? `\\u{${cp.toString(16).toUpperCase().padStart(4, "0")}}`
      : ch;
  }
  return out;
}

/** Build the sticky conflict notice: both resolved paths, control/bidi-escaped,
 *  on their own labelled lines (rendered pre-wrap by the existing ToastHost). */
function formatConflictMessage(issue: ProjectPathConflictIssue): string {
  const absolute = escapeControlAndBidi(issue.absoluteResolvedPath);
  const relative = escapeControlAndBidi(issue.instanceRelativeResolvedPath);
  return (
    "Project path conflict: two saved locations resolve to different directories, " +
    "so this project was not loaded.\n" +
    `Absolute path: ${absolute}\n` +
    `Instance-relative path: ${relative}`
  );
}

/** Actionable text retained in lastLoadError for a blocking active issue. */
function issueBlockingText(issue: ProjectPathIssue): string {
  switch (issue.kind) {
    case "conflict":
      return formatConflictMessage(issue);
    case "missing":
      return issue.message;
    case "invalid":
      return issue.reason;
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
    archivedProjectPaths: string[] = [],
    // #1077: the transport-untrusted resolution report. Absent/undefined is the
    // sole legacy fallback; a present-but-malformed report fails closed.
    report?: unknown
  ) {
    setArchivedPaths((prev) => {
      const keys = new Set(prev.map(normalizePath));
      return [...prev, ...archivedProjectPaths.filter((p) => !keys.has(normalizePath(p)))];
    });

    const validation = validateProjectPathResolution(report);

    if (validation.status === "invalid") {
      // Fail closed: a malformed report means the returned paths are not
      // trustworthy either. Load nothing, keep an actionable status, and log a
      // single diagnostic without echoing raw candidates.
      console.error(
        "[project] Ignoring malformed project-path resolution report; loading no projects."
      );
      batch(() => {
        setLastLoadError(INVALID_REPORT_MESSAGE);
        setInitState({ attempted: true, pathCount: 0 });
      });
      return;
    }

    const paths = [...projectPaths];
    if (legacyPath && !paths.some((p) => normalizePath(p) === normalizePath(legacyPath))) {
      paths.push(legacyPath);
    }

    if (validation.status === "absent") {
      // Legacy/mixed-version fallback: treat the returned paths as selected,
      // with the merged legacy count and no structured issues.
      setInitState({ attempted: true, pathCount: paths.length });
      for (const path of paths) {
        await projectStore.loadProject(path);
      }
      return;
    }

    const { report: resolution } = validation;

    // Surface every conflict once (per ID) BEFORE loading; the selected paths
    // already exclude conflict/quarantined candidates, so we never open or
    // discover an issue candidate here.
    for (const issue of resolution.issues) {
      if (issue.kind !== "conflict") continue;
      if (seenConflictIds.has(issue.id)) continue;
      seenConflictIds.add(issue.id);
      toastStore.error(formatConflictMessage(issue), { durationMs: null });
    }

    // Retain the first ACTIVE blocking issue (archived-only issues do not claim
    // an active load failure). Captured before loads so a later success cannot
    // silently clear the evidence.
    const firstActiveIssue = resolution.issues.find((issue) =>
      isActiveIssueSource(issue.source)
    );
    const activeBlockingText = firstActiveIssue
      ? issueBlockingText(firstActiveIssue)
      : null;

    // Use the merged logical count so a conflict-only startup is not misreported
    // as a pristine no-project state.
    setInitState({ attempted: true, pathCount: resolution.activeRegistrationCount });

    for (const path of paths) {
      await projectStore.loadProject(path);
    }

    if (activeBlockingText !== null) {
      setLastLoadError(activeBlockingText);
    }
  },

  async createAndLoad(path: string) {
    const reg = await ProjectAPI.new(path);
    const normalized = normalizePath(reg.path);
    await runSerializedArchiveChange(normalized, () =>
      discoverAndAppendIfCurrent(reg.path, normalized)
    );
  },

  async pickAndCheck(): Promise<{ picked: string | null; hasAcRoot: boolean }> {
    const picked = await AgentCreatorAPI.pickFolder();
    if (!picked) return { picked: null, hasAcRoot: false };

    const hasAcRoot = await ProjectAPI.checkPath(picked);
    if (hasAcRoot) {
      await projectStore.loadProject(picked);
    }
    return { picked, hasAcRoot };
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
    seenConflictIds.clear();
    replicaVolatileStore.clearAll();
  },
};
