import type { AcWorkgroup } from "../../shared/types";
import { confirmedRoomIsActive } from "../stores/room-activity";
export {
  replicaSessionName,
  findReplicaSession,
  isReplicaWorking,
  roomIsWorking,
  splitRoomsByWorking,
  configuredReplicaRepoBadgesLive,
  replicaRepoBadgesLive,
  replicaCiRunning,
  roomCiRunning,
  replicaHasRaisedHand,
  roomHasRaisedHand,
  replicaHasBlockedMenu,
  roomHasBlockedMenu,
} from "./room-session-source";

/** Aggregate consumers share confirmed room activity; individual helpers stay raw. */
export function roomIsActive(projectPath: string, room: AcWorkgroup): boolean {
  return confirmedRoomIsActive(projectPath, room);
}

export function splitRoomsByActive(
  projectPath: string,
  rooms: readonly AcWorkgroup[]
): { active: AcWorkgroup[]; notActive: AcWorkgroup[] } {
  const active: AcWorkgroup[] = [];
  const notActive: AcWorkgroup[] = [];
  for (const room of rooms) {
    (roomIsActive(projectPath, room) ? active : notActive).push(room);
  }
  return { active, notActive };
}
