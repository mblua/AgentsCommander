import type { AcAgentReplica, AcWorkgroup, Session, SessionRepo } from "../../shared/types";
import { isSessionWorking, sessionRuntimeState } from "../../shared/session-activity";
import { normalizeProjectPathForCompare } from "../stores/project-refresh";
import { sessionsStore } from "../stores/sessions";
import {
  effectiveRepoBranch,
  effectiveRepoBranchByPath,
  effectiveRepoDirtyByPath,
} from "../stores/replica-volatile";
import { remoteActivityStore } from "../stores/remote-activity";
import { configuredReplicaRepoBadges } from "./replica-repo-badges";

export function replicaSessionName(room: AcWorkgroup, replica: AcAgentReplica): string {
  return `${room.name}/${replica.name}`;
}

function normalizedReplicaSessionPath(path: string | null | undefined): string | null {
  const trimmed = path?.trim();
  return trimmed ? normalizeProjectPathForCompare(trimmed) : null;
}

export function findReplicaSession(room: AcWorkgroup, replica: AcAgentReplica): Session | undefined {
  const expectedName = replicaSessionName(room, replica);
  const expectedPath = normalizedReplicaSessionPath(replica.path);
  if (!expectedPath) return undefined;

  return sessionsStore.sessions.find(
    (session) =>
      session.name === expectedName &&
      normalizedReplicaSessionPath(session.workingDirectory) === expectedPath
  );
}

export function isReplicaWorking(room: AcWorkgroup, replica: AcAgentReplica): boolean {
  return isSessionWorking(findReplicaSession(room, replica));
}

export function roomIsWorking(room: AcWorkgroup): boolean {
  return room.agents.some((replica) => isReplicaWorking(room, replica));
}

export function splitRoomsByWorking(
  rooms: readonly AcWorkgroup[]
): { working: AcWorkgroup[]; notWorking: AcWorkgroup[] } {
  const working: AcWorkgroup[] = [];
  const notWorking: AcWorkgroup[] = [];
  for (const room of rooms) {
    (roomIsWorking(room) ? working : notWorking).push(room);
  }
  return { working, notWorking };
}

export function configuredReplicaRepoBadgesLive(
  replica: AcAgentReplica,
  room: Pick<AcWorkgroup, "repoPath">
): SessionRepo[] {
  return configuredReplicaRepoBadges(
    {
      repoPaths: replica.repoPaths,
      repoBranch: effectiveRepoBranch(replica),
      repoBranchByPath: effectiveRepoBranchByPath(replica),
      repoDirtyByPath: effectiveRepoDirtyByPath(replica),
    },
    room
  );
}

/** #2202 - the SAME rule the room row's `repoBadges()` memo uses (session
 *  `gitRepos` first, configured badges as fallback), reading the SAME
 *  `findReplicaSession` the row reads, so the rail counter and the row tint
 *  cannot disagree for one room in one frame. */
export function replicaRepoBadgesLive(room: AcWorkgroup, replica: AcAgentReplica): SessionRepo[] {
  const session = findReplicaSession(room, replica);
  return session && session.gitRepos.length > 0
    ? session.gitRepos
    : configuredReplicaRepoBadgesLive(replica, room);
}

/** #2202 - the coordinator gate matches the row's `isCoord()` term: a
 *  non-coordinator replica's CI counts on no surface. */
export function replicaCiRunning(room: AcWorkgroup, replica: AcAgentReplica): boolean {
  if (!replica.isCoordinator) return false;
  return replicaRepoBadgesLive(room, replica).some(
    (repo) => remoteActivityStore.forPath(repo.sourcePath)?.ci === "running"
  );
}

export function roomCiRunning(room: AcWorkgroup): boolean {
  return room.agents.some((replica) => replicaCiRunning(room, replica));
}

export function replicaHasRaisedHand(room: AcWorkgroup, replica: AcAgentReplica): boolean {
  if (!replica.isCoordinator) return false;
  if (!room.taskTitle) return false;
  const communication = findReplicaSession(room, replica)?.communication;
  return communication?.kind === "raiseHand" && communication?.visible === true;
}

export function roomHasRaisedHand(room: AcWorkgroup): boolean {
  return room.agents.some((replica) => replicaHasRaisedHand(room, replica));
}

export function replicaHasBlockedMenu(room: AcWorkgroup, replica: AcAgentReplica): boolean {
  const communication = findReplicaSession(room, replica)?.communication;
  return communication?.kind === "blockedMenu" && communication?.visible === true;
}

export function roomHasBlockedMenu(room: AcWorkgroup): boolean {
  return room.agents.some((replica) => replicaHasBlockedMenu(room, replica));
}

/** Raw room policy shared by the confirmation observer. */
export function isRoomSessionBusy(session: Session | null | undefined, comanaged: boolean): boolean {
  if (!session || sessionRuntimeState(session.status) === "exited") return false;
  return comanaged || isSessionWorking(session);
}

export function roomIsRawActive(room: AcWorkgroup): boolean {
  return room.agents.some((replica) => {
    const session = findReplicaSession(room, replica);
    return isRoomSessionBusy(session, !!session && !!sessionsStore.comanagedBySessionId[session.id]);
  }) || roomCiRunning(room);
}
