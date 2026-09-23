
import { createEffect, createRoot, createSignal } from "solid-js";
import type { Session } from "../../shared/types";
import { playTeamIdleBeep } from "../../shared/sound";
import { settingsStore } from "../../shared/stores/settings";
import { sessionsStore } from "./sessions";
import { projectStore } from "./project";
import type { ProjectState } from "./project";

export const GRACE_MS = 4000;

const [osFocused, setOsFocused] = createSignal(true);

export function shouldSuppressBeep(
  wgPath: string,
  focusedWg: string | null,
  graceUntil: ReadonlyMap<string, number>,
  now: number,
): boolean {
  if (wgPath === focusedWg) return true;
  const until = graceUntil.get(wgPath);
  return until !== undefined && now < until;
}

export function updateGraceOnFocusChange(
  previousFocusedWg: string | null,
  focusedWg: string | null,
  graceUntil: Map<string, number>,
  now: number,
  graceMs: number,
): string | null {
  if (previousFocusedWg !== focusedWg && previousFocusedWg !== null) {
    graceUntil.set(previousFocusedWg, now + graceMs);
  }
  return focusedWg;
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
  if (isExited(session.status)) return false;
  // #2442 - an armed Co-managed session records waiting but is mid-cycle.
  if (comanaged) return true;
  return !session.waitingForInput;
}

function registerSessionsForProjects(
  projects: readonly ProjectState[],
  sessionToWg: Map<string, string>,
  findSessionByName: (name: string) => Session | undefined,
): void {
  for (const project of projects) {
    for (const wg of project.workgroups) {
      for (const replica of wg.agents) {
        const session = findSessionByName(`${wg.name}/${replica.name}`);
        if (session && !sessionToWg.has(session.id)) {
          sessionToWg.set(session.id, wg.path);
        }
      }
    }
  }
}

function collectBusyByWg(
  sessionToWg: ReadonlyMap<string, string>,
  sessionsById: ReadonlyMap<string, Session>,
): Map<string, Map<string, boolean>> {
  const currentByWg = new Map<string, Map<string, boolean>>();
  for (const [sessionId, wgPath] of sessionToWg) {
    const session = sessionsById.get(sessionId);
    if (!session) continue;
    if (isExited(session.status)) continue;
    let inner = currentByWg.get(wgPath);
    if (!inner) {
      inner = new Map<string, boolean>();
      currentByWg.set(wgPath, inner);
    }
    inner.set(
      sessionId,
      isBusy(session, sessionsStore.comanagedBySessionId[session.id] ?? false),
    );
  }
  return currentByWg;
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

export function beepIdleTransitions(
  currentByWg: ReadonlyMap<string, ReadonlyMap<string, boolean>>,
  previousByWg: ReadonlyMap<string, ReadonlyMap<string, boolean>>,
  focusedWg: string | null,
  graceUntil: ReadonlyMap<string, number>,
  now: number,
): void {
  for (const [wgPath, currentBusy] of currentByWg) {
    const previousBusy = previousByWg.get(wgPath);
    if (!previousBusy) continue;
    if (!hasBusyToIdleTransition(previousBusy, currentBusy)) continue;
    if (!allSessionsIdle(currentBusy)) continue;
    if (shouldSuppressBeep(wgPath, focusedWg, graceUntil, now)) continue;
    void playTeamIdleBeep();
  }
}

export function pruneExpiredGrace(graceUntil: Map<string, number>, now: number): void {
  for (const [wgPath, until] of graceUntil) {
    if (now >= until) graceUntil.delete(wgPath);
  }
}

function replacePreviousByWg(
  previousByWg: Map<string, Map<string, boolean>>,
  currentByWg: ReadonlyMap<string, ReadonlyMap<string, boolean>>,
): void {
  previousByWg.clear();
  for (const [wgPath, perSession] of currentByWg) {
    previousByWg.set(wgPath, new Map(perSession));
  }
}

export function startTeamIdleWatcher(): () => void {
  return createRoot((dispose) => {
    const sessionToWg = new Map<string, string>();

    const previousByWg = new Map<string, Map<string, boolean>>();

    const graceUntil = new Map<string, number>();

    let previousFocusedWg: string | null = null;

    let initialized = false;

    let disposed = false;
    let unlistenOsFocus: (() => void) | null = null;
    void startOsFocusListener().then((unlisten) => {
      if (disposed) {
        try {
          unlisten();
        } catch {
        }
        return;
      }
      unlistenOsFocus = unlisten;
    });

    createEffect(() => {
      const sessions = sessionsStore.sessions;
      const projects = projectStore.projects;
      const enabled = settingsStore.current?.teamIdleBeepEnabled ?? true;
      const activeId = sessionsStore.activeId;
      const hasOsFocus = osFocused();

      registerSessionsForProjects(projects, sessionToWg, (name) =>
        sessionsStore.findSessionByName(name),
      );

      const sessionsById = new Map<string, Session>();
      for (const s of sessions) sessionsById.set(s.id, s);

      const currentByWg = collectBusyByWg(sessionToWg, sessionsById);

      const focusedWg =
        hasOsFocus && activeId ? sessionToWg.get(activeId) ?? null : null;

      if (!initialized) {
        initialized = true;
        replacePreviousByWg(previousByWg, currentByWg);
        return;
      }

      previousFocusedWg = updateGraceOnFocusChange(
        previousFocusedWg,
        focusedWg,
        graceUntil,
        Date.now(),
        GRACE_MS,
      );

      if (enabled) {
        const now = Date.now();
        beepIdleTransitions(currentByWg, previousByWg, focusedWg, graceUntil, now);
        pruneExpiredGrace(graceUntil, now);
      }

      replacePreviousByWg(previousByWg, currentByWg);
    });

    return () => {
      disposed = true;
      if (unlistenOsFocus) {
        try {
          unlistenOsFocus();
        } catch {
        }
        unlistenOsFocus = null;
      }
      dispose();
    };
  });
}
