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

/** #588 by-id entry point: the single implementation. Resolves the live session
 *  ONLY to give the modal a friendly name; the backend self-routes a
 *  non-coordinator id to a plain destroy, so id-only callers (the keyboard
 *  shortcut) need no `Session`. On needs-confirmation it opens the modal (host
 *  lives in ProjectPanel). */
export async function requestCoordinatorCloseById(id: string): Promise<void> {
  const s = sessionsStore.sessions.find((x) => x.id === id);
  // Known non-coordinator -> identical net effect to destroy_session.
  if (s && !s.isCoordinator) {
    await SessionAPI.destroy(id);
    return;
  }
  try {
    const outcome = await SessionAPI.closeCoordinator(id, false);
    if (!outcome.closed) {
      setPendingCoordinatorClose({
        sessionId: id,
        name: s?.name ?? "this coordinator",
        workingCount: outcome.workingCount,
      });
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
    await SessionAPI.destroy(session.id);
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
