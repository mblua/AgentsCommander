import { createEffect, onCleanup } from "solid-js";
import type { AcWorkgroup } from "../../shared/types";
import { findReplicaSession, workgroupCiRunning } from "../components/workgroup-session";
import { projectStore, type ProjectState } from "./project";
import { sessionsStore } from "./sessions";
import { normalizeProjectPathForCompare } from "./project-refresh";

/** #2202 - previous CI state per room, module-level because the watcher must see
 *  the edge across effect runs. Cleared by `resetCiActivityStampForTests` (the
 *  `#1624`/`#965` hazard: it is out of `setSessions`' reach, so a second render in
 *  one test file would inherit a stale `true` or a cleared map). */
const previous = new Map<string, boolean>();

function keyFor(project: ProjectState, wg: AcWorkgroup): string {
  return `${normalizeProjectPathForCompare(project.path)}|${normalizeProjectPathForCompare(wg.path || wg.name)}`;
}

/** #2202 - stamps `lastActivityBySessionId` on a room's CI falling edge, so a room
 *  whose CI just finished floats under sort-by-activity exactly as a session that
 *  just stopped working does. Reads `workgroupCiRunning`, the same predicate the
 *  rail counter, dot, tooltip, row tint and watchdog read, so the stamp cannot
 *  disagree with what the user sees. Call once, from a component body. */
export function startCiActivityStamp(): void {
  createEffect(() => {
    const seen = new Set<string>();
    for (const project of projectStore.projects) {
      for (const wg of project.workgroups) {
        const key = keyFor(project, wg);
        seen.add(key);
        const now = workgroupCiRunning(wg);
        const before = previous.get(key);
        previous.set(key, now);
        if (before !== true || now) continue; // stamp only on true -> false
        for (const replica of wg.agents) {
          if (!replica.isCoordinator) continue;
          const session = findReplicaSession(wg, replica);
          if (session) sessionsStore.markActivity(session.id);
        }
      }
    }
    // A room gone from the store drops its key without stamping.
    for (const key of [...previous.keys()]) if (!seen.has(key)) previous.delete(key);
  });
  onCleanup(() => previous.clear());
}

export function resetCiActivityStampForTests(): void {
  previous.clear();
}
