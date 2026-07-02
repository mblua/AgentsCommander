import type { AcDiscoveryResult, AcWorkgroup } from "../../shared/types";
import type { ProjectState } from "./project";
import { normalizeProjectPathForCompare } from "./project-refresh";

/**
 * #748 — identity-preserving merge of a fresh discovery snapshot over the
 * loaded project state. ProjectPanel keys everything with reference-keyed
 * <For>s, so an object that changes reference is disposed and re-created in
 * the DOM — and a click whose press straddles that swap is lost (no `click`
 * for a detached mousedown target).
 *
 * DOM guarantee is PROJECT-granular: a fully identical snapshot returns the
 * existing project object itself (the store skips the write — zero DOM work),
 * and other projects always keep their reference. When something inside a
 * project DID change, that project's reference changes and its whole <For>
 * row re-renders; the nested entity references this merge still preserves do
 * not keep DOM alive in that case (the row's nested <For>s are fresh
 * instances) — they keep downstream memos/derivations cheap and make the
 * store contents diff-friendly. Finer-than-project DOM stability would need
 * the createStore+reconcile refactor that is explicitly out of scope here.
 */

/** Structural equality for JSON-shaped discovery data (no functions/Dates).
 *  A key holding `undefined` and a missing key compare as equal, matching
 *  how the optional fields behave everywhere they are read. */
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

/**
 * Keyed identity-preserving array merge: each incoming item keeps the existing
 * object reference when the entity (matched by key) is unchanged; only changed
 * or new entities take the incoming object. Returns the EXISTING array
 * reference when nothing changed at any position.
 */
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
  // The only cast in this module: `existing` is only ever the caller's own
  // mutable array, accepted as readonly for variance.
  return changed ? merged : (existing as T[]);
}

/** Preserve per-agent references inside a workgroup that changed elsewhere. */
function mergeWorkgroup(oldWg: AcWorkgroup, newWg: AcWorkgroup): AcWorkgroup {
  if (deepEqual(oldWg, newWg)) return oldWg;
  const agents = mergeKeyedArray(oldWg.agents, newWg.agents, (agent) =>
    normalizeProjectPathForCompare(agent.path)
  );
  return { ...newWg, agents };
}

/** Merge a discovery result into the loaded project, preserving identity of
 *  every unchanged entity. Returns `existing` itself when nothing changed. */
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
