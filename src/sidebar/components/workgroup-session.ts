import type { AcAgentReplica, AcWorkgroup, Session, SessionRepo } from "../../shared/types";
import { isSessionWorking } from "../../shared/session-activity";
import { normalizeProjectPathForCompare } from "../stores/project-refresh";
import { sessionsStore } from "../stores/sessions";
import {
  effectiveRepoBranch,
  effectiveRepoBranchByPath,
  effectiveRepoDirtyByPath,
} from "../stores/replica-volatile";
import { remoteActivityStore } from "../stores/remote-activity";
import { configuredReplicaRepoBadges } from "./replica-repo-badges";

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

export function configuredReplicaRepoBadgesLive(
  replica: AcAgentReplica,
  workgroup: Pick<AcWorkgroup, "repoPath">
): SessionRepo[] {
  return configuredReplicaRepoBadges(
    {
      repoPaths: replica.repoPaths,
      repoBranch: effectiveRepoBranch(replica),
      repoBranchByPath: effectiveRepoBranchByPath(replica),
      repoDirtyByPath: effectiveRepoDirtyByPath(replica),
    },
    workgroup
  );
}

/** #2202 - the SAME rule the room row's `repoBadges()` memo uses (session
 *  `gitRepos` first, configured badges as fallback), reading the SAME
 *  `findReplicaSession` the row reads, so the rail counter and the row tint
 *  cannot disagree for one room in one frame. */
export function replicaRepoBadgesLive(wg: AcWorkgroup, replica: AcAgentReplica): SessionRepo[] {
  const session = findReplicaSession(wg, replica);
  return session && session.gitRepos.length > 0
    ? session.gitRepos
    : configuredReplicaRepoBadgesLive(replica, wg);
}

/** #2202 - the coordinator gate matches the row's `isCoord()` term: a
 *  non-coordinator replica's CI counts on no surface. */
export function replicaCiRunning(wg: AcWorkgroup, replica: AcAgentReplica): boolean {
  if (!replica.isCoordinator) return false;
  return replicaRepoBadgesLive(wg, replica).some(
    (repo) => remoteActivityStore.forPath(repo.sourcePath)?.ci === "running"
  );
}

export function workgroupCiRunning(wg: AcWorkgroup): boolean {
  return wg.agents.some((replica) => replicaCiRunning(wg, replica));
}

/** #2202 - "working" on every user-facing surface: a session is working OR the
 *  room's repo is running CI. `workgroupIsWorking` stays session-only. */
export function workgroupIsActive(wg: AcWorkgroup): boolean {
  return workgroupIsWorking(wg) || workgroupCiRunning(wg);
}

export function splitWorkgroupsByActive(
  workgroups: readonly AcWorkgroup[]
): { active: AcWorkgroup[]; notActive: AcWorkgroup[] } {
  const active: AcWorkgroup[] = [];
  const notActive: AcWorkgroup[] = [];
  for (const wg of workgroups) {
    (workgroupIsActive(wg) ? active : notActive).push(wg);
  }
  return { active, notActive };
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

export function replicaHasBlockedMenu(wg: AcWorkgroup, replica: AcAgentReplica): boolean {
  const communication = findReplicaSession(wg, replica)?.communication;
  return communication?.kind === "blockedMenu" && communication?.visible === true;
}

export function workgroupHasBlockedMenu(wg: AcWorkgroup): boolean {
  return wg.agents.some((replica) => replicaHasBlockedMenu(wg, replica));
}
