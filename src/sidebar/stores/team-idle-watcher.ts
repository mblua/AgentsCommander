
import { createEffect, createRoot, createSignal } from "solid-js";
import type { AcWorkgroup, Session } from "../../shared/types";
import { playTeamIdleBeep } from "../../shared/sound";
import { settingsStore } from "../../shared/stores/settings";
import { sessionsStore } from "./sessions";
import { projectStore } from "./project";
import type { ProjectState } from "./project";
import { findReplicaSession, isRoomSessionBusy, roomCiRunning, roomIsRawActive } from "../components/room-session-source";
import { confirmedRoomIsActive, roomActivityKey } from "./room-activity";

export const GRACE_MS = 4000;

const [osFocused, setOsFocused] = createSignal(true);

export function shouldSuppressBeep(
  roomKey: string,
  focusedRoomKey: string | null,
  graceUntil: ReadonlyMap<string, number>,
  now: number,
): boolean {
  if (roomKey === focusedRoomKey) return true;
  const until = graceUntil.get(roomKey);
  return until !== undefined && now < until;
}

export function updateGraceOnFocusChange(
  previousFocusedRoomKey: string | null,
  focusedRoomKey: string | null,
  graceUntil: Map<string, number>,
  now: number,
  graceMs: number,
): string | null {
  if (previousFocusedRoomKey !== focusedRoomKey && previousFocusedRoomKey !== null) {
    graceUntil.set(previousFocusedRoomKey, now + graceMs);
  }
  return focusedRoomKey;
}

async function startOsFocusListener(): Promise<() => void> {
  try {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    const win = getCurrentWindow();
    try {
      const focused = await win.isFocused();
      setOsFocused(focused);
    } catch {
    }
    const unlisten = await win.onFocusChanged(({ payload: focused }) => {
      setOsFocused(focused);
    });
    return unlisten;
  } catch {
    return () => {};
  }
}

function isExited(status: Session["status"]): boolean {
  return typeof status === "object" && status !== null && "exited" in status;
}

export function isBusy(session: Session, comanaged: boolean): boolean {
  return isRoomSessionBusy(session, comanaged);
}

type RoomSample = {
  projectPath: string;
  room: AcWorkgroup;
  busy: Map<string, boolean>;
  ci: boolean;
  confirmed: boolean;
};
type CompletionLatch = {
  contributingIds: Set<string>;
  ciAlreadyRunning: boolean;
  generation: number;
};

function registerRoomSessions(
  projects: readonly ProjectState[],
  sessionToRoom: Map<string, string>,
): Map<string, RoomSample> {
  sessionToRoom.clear();
  const currentByRoom = new Map<string, RoomSample>();
  for (const project of projects) {
    for (const room of project.workgroups) {
      const key = roomActivityKey(project.path, room.path);
      if (key === null) continue;
      const busy = new Map<string, boolean>();
      for (const replica of room.agents) {
        const session = findReplicaSession(room, replica);
        if (!session || isExited(session.status)) continue;
        sessionToRoom.set(session.id, key);
        busy.set(session.id, isBusy(session, !!sessionsStore.comanagedBySessionId[session.id]));
      }
      currentByRoom.set(key, {
        projectPath: project.path, room, busy,
        ci: roomCiRunning(room),
        confirmed: confirmedRoomIsActive(project.path, room),
      });
    }
  }
  return currentByRoom;
}

export function hasBusyToIdleTransition(
  previousBusy: ReadonlyMap<string, boolean>,
  currentBusy: ReadonlyMap<string, boolean>,
): boolean {
  for (const [sessionId, wasBusy] of previousBusy) {
    if (!wasBusy) continue;
    if (currentBusy.get(sessionId) === false) return true;
  }
  return false;
}

export function allSessionsIdle(currentBusy: ReadonlyMap<string, boolean>): boolean {
  if (currentBusy.size === 0) return false;
  for (const isBusyNow of currentBusy.values()) {
    if (isBusyNow) return false;
  }
  return true;
}

function latchInvalid(latch: CompletionLatch, sample: RoomSample): boolean {
  if (!allSessionsIdle(sample.busy)) return true;
  for (const id of latch.contributingIds) {
    if (!sample.busy.has(id)) return true;
  }
  return sample.ci && !latch.ciAlreadyRunning;
}

function captureCompletion(
  previous: ReadonlyMap<string, boolean> | undefined,
  sample: RoomSample,
  generation: number,
): CompletionLatch | undefined {
  if (!previous || !allSessionsIdle(sample.busy) || !hasBusyToIdleTransition(previous, sample.busy)) return;
  const contributingIds = new Set<string>();
  for (const [id, busy] of previous) {
    if (busy && sample.busy.get(id) === false) contributingIds.add(id);
  }
  return { contributingIds, ciAlreadyRunning: sample.ci, generation };
}

export function pruneExpiredGrace(graceUntil: Map<string, number>, now: number): void {
  for (const [roomKey, until] of graceUntil) {
    if (now >= until) graceUntil.delete(roomKey);
  }
}

export function startTeamIdleWatcher(): () => void {
  return createRoot((dispose) => {
    const sessionToRoom = new Map<string, string>();
    const previousByRoom = new Map<string, Map<string, boolean>>();
    const latches = new Map<string, CompletionLatch>();
    const graceUntil = new Map<string, number>();
    let previousFocusedRoomKey: string | null = null;
    let initialized = false;
    let generation = 0;
    let disposed = false;
    let unlistenOsFocus: (() => void) | null = null;
    void startOsFocusListener().then((unlisten) => {
      if (disposed) {
        try { unlisten(); } catch {}
        return;
      }
      unlistenOsFocus = unlisten;
    });

    createEffect(() => {
      const enabled = settingsStore.current?.teamIdleBeepEnabled ?? true;
      const currentByRoom = registerRoomSessions(projectStore.projects, sessionToRoom);
      const activeId = sessionsStore.activeId;
      const focusedRoomKey = osFocused() && activeId ? sessionToRoom.get(activeId) ?? null : null;
      const now = Date.now();
      if (initialized) {
        previousFocusedRoomKey = updateGraceOnFocusChange(
          previousFocusedRoomKey, focusedRoomKey, graceUntil, now, GRACE_MS,
        );
      }
      for (const key of latches.keys()) {
        if (!currentByRoom.has(key) || !enabled) latches.delete(key);
      }
      for (const [key, sample] of currentByRoom) {
        let latch = latches.get(key);
        const invalid = latch !== undefined && latchInvalid(latch, sample);
        if (invalid) {
          latches.delete(key);
          latch = undefined;
        }
        // Capture before history replacement; an invalidated contributing ID
        // discards the entire episode, even if a surviving ID just went idle.
        if (enabled && !invalid && !latch) {
          latch = captureCompletion(previousByRoom.get(key), sample, ++generation);
          if (latch) latches.set(key, latch);
        }
        if (!latch) continue;
        if (!sample.ci) latch.ciAlreadyRunning = false;
        // This same effect runs on latch creation AND confirmation publication.
        // Sampling rebuilt exact live identities; no remembered second edge.
        if (sample.confirmed || roomIsRawActive(sample.room)) continue;
        latches.delete(key);
        if (!shouldSuppressBeep(key, focusedRoomKey, graceUntil, now)) void playTeamIdleBeep();
      }
      previousByRoom.clear();
      for (const [key, sample] of currentByRoom) previousByRoom.set(key, sample.busy);
      initialized = true;
      pruneExpiredGrace(graceUntil, now);
    });

    return () => {
      if (disposed) return;
      disposed = true;
      generation++;
      latches.clear();
      previousByRoom.clear();
      sessionToRoom.clear();
      graceUntil.clear();
      if (unlistenOsFocus) {
        try { unlistenOsFocus(); } catch {}
        unlistenOsFocus = null;
      }
      dispose();
    };
  });
}
