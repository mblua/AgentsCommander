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

export function isReplicaWorking(wg: AcWorkgroup, replica: AcAgentReplica): boolean {
  return isSessionWorking(findReplicaSession(wg, replica));
}

export function workgroupIsWorking(wg: AcWorkgroup): boolean {
  return wg.agents.some((replica) => isReplicaWorking(wg, replica));
}

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

export function replicaHasRaisedHand(wg: AcWorkgroup, replica: AcAgentReplica): boolean {
  if (!replica.isCoordinator) return false;
  if (!wg.taskTitle) return false;
  const communication = findReplicaSession(wg, replica)?.communication;
  return communication?.kind === "raiseHand" && communication?.visible === true;
}

export function workgroupHasRaisedHand(wg: AcWorkgroup): boolean {
  return wg.agents.some((replica) => replicaHasRaisedHand(wg, replica));
}
