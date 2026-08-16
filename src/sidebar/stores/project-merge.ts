import type {
  AcAgentMatrix,
  AcDiscoveryResult,
  AcLoopSummary,
  AcTeam,
  AcWorkgroup,
  ContextTemplateUpdate,
} from "../../shared/types";
import { normalizeProjectPathForCompare } from "./project-refresh";

// #1283 baseline cycle break (2026-08-12 plan amendment): the ProjectState
// interface moved here from ./project so that project.ts imports from
// project-merge only and project-merge imports nothing from project. project.ts
// re-exports this local declaration (import + `export type { ProjectState }`) so
// every existing consumer keeps compiling unchanged.
export interface ProjectState {
  path: string;
  folderName: string;
  workgroups: AcWorkgroup[];
  agents: AcAgentMatrix[];
  teams: AcTeam[];
  loops: AcLoopSummary[];
  contextTemplateUpdates: ContextTemplateUpdate[];
}


export function deepEqual(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (a === null || b === null || typeof a !== "object" || typeof b !== "object") return false;
  const aIsArray = Array.isArray(a);
  if (aIsArray !== Array.isArray(b)) return false;
  if (aIsArray) {
    const arrA = a as unknown[];
    const arrB = b as unknown[];
    if (arrA.length !== arrB.length) return false;
    for (let i = 0; i < arrA.length; i++) {
      if (!deepEqual(arrA[i], arrB[i])) return false;
    }
    return true;
  }
  const objA = a as Record<string, unknown>;
  const objB = b as Record<string, unknown>;
  for (const key in objA) {
    if (!deepEqual(objA[key], objB[key])) return false;
  }
  for (const key in objB) {
    if (!(key in objA) && objB[key] !== undefined) return false;
  }
  return true;
}

function mergeKeyedArray<T>(
  existing: readonly T[],
  incoming: readonly T[],
  keyOf: (item: T) => string,
  mergeItem: (oldItem: T, newItem: T) => T = (oldItem, newItem) =>
    deepEqual(oldItem, newItem) ? oldItem : newItem
): T[] {
  const byKey = new Map<string, T>();
  for (const item of existing) byKey.set(keyOf(item), item);
  let changed = incoming.length !== existing.length;
  const merged = incoming.map((newItem, index) => {
    const oldItem = byKey.get(keyOf(newItem));
    const result = oldItem === undefined ? newItem : mergeItem(oldItem, newItem);
    if (result !== existing[index]) changed = true;
    return result;
  });
  return changed ? merged : (existing as T[]);
}

function mergeWorkgroup(oldWg: AcWorkgroup, newWg: AcWorkgroup): AcWorkgroup {
  if (deepEqual(oldWg, newWg)) return oldWg;
  const agents = mergeKeyedArray(oldWg.agents, newWg.agents, (agent) =>
    normalizeProjectPathForCompare(agent.path)
  );
  return { ...newWg, agents };
}

export function mergeDiscoveryResult(
  existing: ProjectState,
  result: AcDiscoveryResult
): ProjectState {
  const workgroups = mergeKeyedArray(
    existing.workgroups,
    result.workgroups,
    (wg) => normalizeProjectPathForCompare(wg.path),
    mergeWorkgroup
  );
  const agents = mergeKeyedArray(existing.agents, result.agents, (agent) =>
    normalizeProjectPathForCompare(agent.path)
  );
  const teams = mergeKeyedArray(existing.teams, result.teams, (team) => team.name);
  const loops = mergeKeyedArray(existing.loops, result.loops, (loop) => loop.id);
  const contextTemplateUpdates = deepEqual(
    existing.contextTemplateUpdates,
    result.contextTemplateUpdates
  )
    ? existing.contextTemplateUpdates
    : result.contextTemplateUpdates;

  if (
    workgroups === existing.workgroups &&
    agents === existing.agents &&
    teams === existing.teams &&
    loops === existing.loops &&
    contextTemplateUpdates === existing.contextTemplateUpdates
  ) {
    return existing;
  }
  return { ...existing, workgroups, agents, teams, loops, contextTemplateUpdates };
}
