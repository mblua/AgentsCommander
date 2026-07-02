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
