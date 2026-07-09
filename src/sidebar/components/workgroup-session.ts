import type { AcAgentReplica, AcWorkgroup, Session } from "../../shared/types";
import { isSessionWorking } from "../../shared/session-activity";
import { normalizeProjectPathForCompare } from "../stores/project-refresh";
import { sessionsStore } from "../stores/sessions";

export function replicaSessionName(wg: AcWorkgroup, replica: AcAgentReplica): string {
  return `${wg.name}/${replica.name}`;
}

function normalizedReplicaSessionPath(path: string | null | undefined): string | null {
  const trimmed = path?.trim();
  return trimmed ? normalizeProjectPathForCompare(trimmed) : null;
}

export function findReplicaSession(wg: AcWorkgroup, replica: AcAgentReplica): Session | undefined {
  const expectedName = replicaSessionName(wg, replica);
  const expectedPath = normalizedReplicaSessionPath(replica.path);
  if (!expectedPath) return undefined;

  return sessionsStore.sessions.find(
    (session) =>
      session.name === expectedName &&
      normalizedReplicaSessionPath(session.workingDirectory) === expectedPath
  );
}

/**
 * #882 Domain predicate over session state. Replaces the pre-#882 dot-class
 * predicate. Same answer for every at-rest store state.
 */
export function isReplicaWorking(wg: AcWorkgroup, replica: AcAgentReplica): boolean {
  return isSessionWorking(findReplicaSession(wg, replica));
}

export function workgroupIsWorking(wg: AcWorkgroup): boolean {
  return wg.agents.some((replica) => isReplicaWorking(wg, replica));
}

/**
 * #777/#882 parity primitive. The rail counter and the non-stop watchdog both
 * partition their workgroup list with this one function, in one traversal,
 * preserving input order. Do not reimplement either side with filter().
 *
 * Must be called inside a reactive scope. It reads sessionsStore through
 * findReplicaSession; caching its result at module scope silently kills tracking.
 */
export function splitWorkgroupsByWorking(
  workgroups: readonly AcWorkgroup[]
): { working: AcWorkgroup[]; notWorking: AcWorkgroup[] } {
  const working: AcWorkgroup[] = [];
  const notWorking: AcWorkgroup[] = [];
  for (const wg of workgroups) {
    (workgroupIsWorking(wg) ? working : notWorking).push(wg);
  }
  return { working, notWorking };
}

/**
 * #763 — true when this replica is a coordinator currently showing a raised
 * hand. Mirrors ProjectPanel's per-row `showRaiseHand` predicate exactly so the
 * group-tab badge lights iff a coordinator row shows the hand:
 *  - no liveness gate — #747 keeps a restored/dormant coordinator's persisted
 *    hand visible (every real-exit path clears communication), so the tab must
 *    light for those too;
 *  - the `wg.taskTitle` gate matches the coordinator quick-list row, which only
 *    renders the hand inside its task line (`renderReplicaItem(..., wg.taskTitle,
 *    "quick")`).
 */
export function replicaHasRaisedHand(wg: AcWorkgroup, replica: AcAgentReplica): boolean {
  if (!replica.isCoordinator) return false;
  if (!wg.taskTitle) return false;
  const communication = findReplicaSession(wg, replica)?.communication;
  return communication?.kind === "raiseHand" && communication?.visible === true;
}

/** #763 — true when any coordinator in the workgroup has a raised hand. */
export function workgroupHasRaisedHand(wg: AcWorkgroup): boolean {
  return wg.agents.some((replica) => replicaHasRaisedHand(wg, replica));
}
