
import { createEffect, createRoot, createSignal } from "solid-js";
import type { Session } from "../../shared/types";
import { playTeamIdleBeep } from "../../shared/sound";
import { settingsStore } from "../../shared/stores/settings";
import { sessionsStore } from "./sessions";
import { projectStore } from "./project";

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

function isBusy(session: Session): boolean {
  if (isExited(session.status)) return false;
  return !session.waitingForInput;
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

      for (const project of projects) {
        for (const wg of project.workgroups) {
          for (const replica of wg.agents) {
            const session = sessionsStore.findSessionByName(
              `${wg.name}/${replica.name}`,
            );
            if (session && !sessionToWg.has(session.id)) {
              sessionToWg.set(session.id, wg.path);
            }
          }
        }
      }

      const sessionsById = new Map<string, Session>();
      for (const s of sessions) sessionsById.set(s.id, s);

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
        inner.set(sessionId, isBusy(session));
      }

      const focusedWg =
        hasOsFocus && activeId ? sessionToWg.get(activeId) ?? null : null;

      if (!initialized) {
        initialized = true;
        for (const [wgPath, perSession] of currentByWg) {
          previousByWg.set(wgPath, new Map(perSession));
        }
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
        for (const [wgPath, currentBusy] of currentByWg) {
          const previousBusy = previousByWg.get(wgPath);
          if (!previousBusy) continue;

          let hadTransition = false;
          for (const [sessionId, wasBusy] of previousBusy) {
            if (!wasBusy) continue;
            const isBusyNow = currentBusy.get(sessionId);
            if (isBusyNow === false) {
              hadTransition = true;
              break;
            }
          }
          if (!hadTransition) continue;

          let allIdle = currentBusy.size > 0;
          if (allIdle) {
            for (const isBusyNow of currentBusy.values()) {
              if (isBusyNow) {
                allIdle = false;
                break;
              }
            }
          }
          if (!allIdle) continue;

          if (shouldSuppressBeep(wgPath, focusedWg, graceUntil, now)) continue;

          void playTeamIdleBeep();
        }

        for (const [wgPath, until] of graceUntil) {
          if (now >= until) graceUntil.delete(wgPath);
        }
      }

      previousByWg.clear();
      for (const [wgPath, perSession] of currentByWg) {
        previousByWg.set(wgPath, new Map(perSession));
      }
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
