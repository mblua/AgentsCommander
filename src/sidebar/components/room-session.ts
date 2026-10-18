import type { AcWorkgroup } from "../../shared/types";
import { roomIsWorking, roomCiRunning } from "./room-session-source";
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

/** #2202 - "working" on every user-facing surface: a session is working OR the
 *  room's repo is running CI. `roomIsWorking` stays session-only. */
export function roomIsActive(room: AcWorkgroup): boolean {
  return roomIsWorking(room) || roomCiRunning(room);
}

export function splitRoomsByActive(
  rooms: readonly AcWorkgroup[]
): { active: AcWorkgroup[]; notActive: AcWorkgroup[] } {
  const active: AcWorkgroup[] = [];
  const notActive: AcWorkgroup[] = [];
  for (const room of rooms) {
    (roomIsActive(room) ? active : notActive).push(room);
  }
  return { active, notActive };
}
