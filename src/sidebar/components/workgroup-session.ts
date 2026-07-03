import type { AcAgentReplica, AcWorkgroup, Session } from "../../shared/types";
import { normalizeProjectPathForCompare } from "../stores/project-refresh";
import { sessionsStore } from "../stores/sessions";
import { sessionDotClass, type SessionDotClass } from "./session-status";

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

export function replicaDotClass(wg: AcWorkgroup, replica: AcAgentReplica): SessionDotClass {
  return sessionDotClass(findReplicaSession(wg, replica));
}

export function isWorkingReplicaDot(dot: SessionDotClass): boolean {
  return dot === "running" || dot === "active";
}

export function workgroupIsWorking(wg: AcWorkgroup): boolean {
  return wg.agents.some((replica) => isWorkingReplicaDot(replicaDotClass(wg, replica)));
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
