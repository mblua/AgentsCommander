import { createSignal } from "solid-js";
import type { Session } from "../../shared/types";
import { SessionAPI } from "../../shared/ipc";
import { sessionsStore } from "./sessions"; // sidebar->sidebar, mirrors team-idle-watcher.ts

export interface PendingCoordinatorClose {
  sessionId: string;
  name: string;
  workingCount: number;
}

const [pendingCoordinatorClose, setPendingCoordinatorClose] =
  createSignal<PendingCoordinatorClose | null>(null);
export { pendingCoordinatorClose, setPendingCoordinatorClose };

// #588 confirm-modal host presence (ref-counted). ProjectPanel — the ONLY
// renderer of the pendingCoordinatorClose modal — registers itself on mount so
// the by-id helper knows whether a confirmation modal can actually be shown in
// THIS window. In a detached terminal webview no host registers (separate JS
// context, no ProjectPanel), so a busy-coordinator keyboard close falls back to
// a plain destroy instead of setting a signal nothing renders (a silent no-op).
let modalHostRefs = 0;

/** #588 Register the confirm-modal host (ProjectPanel) for the current window.
 *  Returns a disposer to call on cleanup. */
export function registerCoordinatorCloseModalHost(): () => void {
  modalHostRefs += 1;
  return () => {
    modalHostRefs = Math.max(0, modalHostRefs - 1);
  };
}

/** #588 True iff a confirm-modal host is mounted in this window. */
function coordinatorCloseModalHostAvailable(): boolean {
  return modalHostRefs > 0;
}

/** Test-only: reset the modal-host ref-count so each test starts from a known
 *  state regardless of order (mirrors the `__*ForTests` hooks elsewhere). */
export function __resetCoordinatorCloseModalHostForTests(): void {
  modalHostRefs = 0;
}

/** #588 by-id entry point: the single implementation. Resolves the live session
 *  ONLY to give the modal a friendly name; the backend self-routes a
 *  non-coordinator id to a plain destroy, so id-only callers (the keyboard
 *  shortcut) need no `Session`. On needs-confirmation it opens the modal (host
 *  lives in ProjectPanel). */
export async function requestCoordinatorCloseById(id: string): Promise<void> {
  const s = sessionsStore.sessions.find((x) => x.id === id);
  try {
    // Known non-coordinator -> identical net effect to destroy_session.
    if (s && !s.isCoordinator) {
      await SessionAPI.destroy(id);
      return;
    }
    const outcome = await SessionAPI.closeCoordinator(id, false);
    if (!outcome.closed) {
      if (coordinatorCloseModalHostAvailable()) {
        setPendingCoordinatorClose({
          sessionId: id,
          name: s?.name ?? "this coordinator",
          workingCount: outcome.workingCount,
        });
      } else {
        // #588 No confirm-modal host in this window (e.g. the detached terminal
        // webview's keyboard shortcut): fall back to closing JUST the coordinator
        // (plain destroy, no cascade) so the user is never left with a silent
        // no-op. The backend refused to cascade busy members without confirmation;
        // destroying only the coordinator restores the pre-#588 behavior here.
        await SessionAPI.destroy(id);
      }
    }
  } catch (err) {
    console.error("close_coordinator failed:", err);
  }
}

/** #588 entry point for callers that already hold the `Session` (ProjectPanel
 *  "X", SessionItem). Non-coordinator -> plain destroy (unchanged behavior);
 *  coordinator -> delegate to the by-id helper (one implementation). */
export async function requestCoordinatorClose(session: Session): Promise<void> {
  if (!session.isCoordinator) {
    try {
      await SessionAPI.destroy(session.id);
    } catch (error) {
      console.error("destroy_session failed:", error);
    }
    return;
  }
  await requestCoordinatorCloseById(session.id);
}

/** Confirm the pending cascade close (called from the modal). Consume-and-clear:
 *  the modal closes before the confirmed call awaits. Accepted (grinch F6, §7):
 *  a failed close leaves the team visibly running, so there is no false success. */
export async function confirmPendingCoordinatorClose(): Promise<void> {
  const pending = pendingCoordinatorClose();
  if (!pending) return;
  setPendingCoordinatorClose(null);
  try {
    await SessionAPI.closeCoordinator(pending.sessionId, true);
  } catch (err) {
    console.error("close_coordinator (confirmed) failed:", err);
  }
}
