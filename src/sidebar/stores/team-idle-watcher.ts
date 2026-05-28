// Per-workgroup busy→all-idle transition detector for Feature #110.
//
// **Spec deviation:** issue #110 calls for per-*team* aggregation, but
// `sessionsStore.state.teams` is currently dead code - `setTeams()` is
// defined but never called from anywhere in the repo. We aggregate by
// workgroup instead, sourcing from `projectStore.projects[].workgroups[]`
// (the live data path that ProjectPanel/TeamFilter actually use).
// **If teams become live (someone wires up `setTeams`), revisit this
// aggregation** - the right unit may be the team-config rather than the
// workgroup instance.
//
// **Why per-session previous-busy tracking** (instead of a busy *set*
// diff): "session destroyed", "session exited", and "session renamed"
// all collapse to "session left the aggregation" under a set-diff
// model, which would fire spurious beeps on user kills, exit-0
// processes, and rename events. Tracking each session's busy flag from
// the previous tick lets us require a *genuine* busy→idle flip on a
// still-alive bound session before we consider beeping.
//
// **Why a stable sessionId→wgPath binding**: sessions match replicas
// by name `${wg.name}/${replica.name}` at creation, but can be renamed
// later. Resolving the binding on every effect run by name means a
// busy session whose name changes drops out of its workgroup's
// aggregation, which is exactly the bug above. We bind once on first
// observation and never unbind, so rename-while-busy is harmless.
//
// **Why createRoot**: the watcher is started from inside an async
// onMount in SidebarApp; by the time we reach `await
// settingsStore.load()` the synchronous reactive owner is gone.
// createRoot gives the effect its own owner and an explicit dispose
// we can call from onCleanup.
//
// **Focused-WG suppression (#254)**: while the user is looking at a
// workgroup (the active session belongs to it AND the AC window holds
// OS focus), that workgroup's beep is gated off - they can already
// see the state change, the beep is redundant. When focus moves away
// (tab switch or alt-tab), the just-left workgroup keeps suppressing
// for GRACE_MS to absorb the user's typical short detour back.

import { createEffect, createRoot, createSignal } from "solid-js";
import type { Session } from "../../shared/types";
import { playTeamIdleBeep } from "../../shared/sound";
import { settingsStore } from "../../shared/stores/settings";
import { sessionsStore } from "./sessions";
import { projectStore } from "./project";

export const GRACE_MS = 4000;

// OS-level focus state for the AC sidebar window. Driven by
// Tauri's onFocusChanged event; treated as true on mount so the
// initial tick (before the listener has reported anything) treats
// the visible WG as focused. Exposed as a signal so the watcher's
// createEffect re-runs on focus flips even when no session has
// changed - without this, alt-tab→idle transitions inside the
// grace window would miss the grace setter.
const [osFocused, setOsFocused] = createSignal(true);

/**
 * Pure decision helper: should we suppress the beep for `wgPath`
 * given the current focus state and grace map?
 *
 * Suppressed when the workgroup is currently focused, OR a grace
 * entry exists for it and `now` is still inside the window.
 */
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

/**
 * Pure transition helper: on a focus change, arm a grace window
 * for the workgroup being left behind (if any) and return the new
 * "previous focused WG" to persist for the next tick.
 *
 * Mutates `graceUntil` in place - callers own the map. Returns the
 * value the caller should assign to its `previousFocusedWg`.
 */
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
    // Best-effort initial sync: if the window is already
    // unfocused at mount, reflect that before any tick runs.
    try {
      const focused = await win.isFocused();
      setOsFocused(focused);
    } catch {
      // Non-fatal - keep the default-true seed.
    }
    const unlisten = await win.onFocusChanged(({ payload: focused }) => {
      setOsFocused(focused);
    });
    return unlisten;
  } catch {
    // Browser/WS transport or import failure - assume always-focused
    // so the legacy behavior (per-WG suppression keyed only on
    // activeId) still applies; we just can't react to OS alt-tab.
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

/**
 * Start the watcher. Returns a dispose function; call from SidebarApp's
 * onCleanup.
 *
 * Behavior on first effect run: snapshot only, no beep (the "no beep
 * at startup" rule). Subsequent runs fire `playTeamIdleBeep()` for any
 * workgroup that meets ALL of:
 *   - At least one session bound to the workgroup, alive last tick,
 *     was busy then and is alive + idle now (genuine transition).
 *   - All currently-bound, currently-alive sessions in the workgroup
 *     are idle.
 *   - The user setting `teamIdleBeepEnabled` is true.
 */
export function startTeamIdleWatcher(): () => void {
  return createRoot((dispose) => {
    // sessionId -> wg.path. Populated on first observation of each
    // session and never overwritten - protects against rename.
    const sessionToWg = new Map<string, string>();

    // wg.path -> Map<sessionId, wasBusy>. The inner map records each
    // bound, alive session's isBusy from the previous tick. Sessions
    // that were not alive last tick (destroyed/exited) won't appear.
    const previousByWg = new Map<string, Map<string, boolean>>();

    // wg.path -> epoch ms at which the grace period for that WG
    // ends. Populated whenever effective focus moves off a WG; the
    // entry suppresses beeps for that WG until Date.now() >= value.
    const graceUntil = new Map<string, number>();

    // The effective focused WG from the previous effect tick. Used
    // to detect focus *transitions* so we can arm the grace window
    // for the WG being left behind.
    let previousFocusedWg: string | null = null;

    let initialized = false;

    // Spawn the OS focus listener; capture the unlisten so we can
    // detach it in dispose. The signal stays at its default until
    // the async listener resolves. `disposed` guards the race where
    // the watcher is torn down before the dynamic import resolves -
    // without it the listener would be registered into an orphan
    // variable and leak forever.
    let disposed = false;
    let unlistenOsFocus: (() => void) | null = null;
    void startOsFocusListener().then((unlisten) => {
      if (disposed) {
        try {
          unlisten();
        } catch {
          // ignore - best-effort detach
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

      // 1. Augment bindings (never unbind). Iterate workgroups and
      //    bind any newly-discovered replica-session pairs by name.
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

      // 2. Build current per-wg busy state from the alive bound
      //    sessions only. Destroyed sessions (not in `sessions`) and
      //    exited sessions are excluded - they don't contribute to
      //    aggregation per spec.
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

      // 3. Resolve the effective focused workgroup for this tick.
      //    A WG is "focused" only when AC has OS focus AND the
      //    active session is bound to that WG. Losing either
      //    condition (alt-tab, tab switch) collapses focusedWg to
      //    null and arms a grace window for the WG being left.
      const focusedWg =
        hasOsFocus && activeId ? sessionToWg.get(activeId) ?? null : null;

      // 4. First run is snapshot-only - see header comment.
      //    Crucially, we skip the focus-transition bookkeeping on
      //    the snapshot tick: the OS-focus listener resolves
      //    asynchronously, so the initial tick may run with the
      //    default-true `osFocused` seed even when AC was launched
      //    in the background. Seeding `previousFocusedWg` here
      //    would then arm a phantom grace window on the first real
      //    tick once `osFocused` resolves to false.
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

      // 5. Detect genuine busy→idle transitions per workgroup.
      if (enabled) {
        const now = Date.now();
        for (const [wgPath, currentBusy] of currentByWg) {
          const previousBusy = previousByWg.get(wgPath);
          if (!previousBusy) continue;

          // Genuine transition: a session that was busy last tick is
          // still alive (still in current map) and is now idle.
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

          // All currently-alive bound sessions are idle?
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

        // Drop expired grace entries so the map stays bounded by
        // the count of currently-tracked WGs (already finite, but
        // tidier).
        for (const [wgPath, until] of graceUntil) {
          if (now >= until) graceUntil.delete(wgPath);
        }
      }

      // 6. Persist current state for the next tick. Workgroups that
      //    no longer have any alive bound sessions drop out, so a
      //    later resurrection starts from a clean snapshot.
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
          // ignore - best-effort detach
        }
        unlistenOsFocus = null;
      }
      dispose();
    };
  });
}
