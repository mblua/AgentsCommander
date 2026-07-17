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

let modalHostRefs = 0;

export function registerCoordinatorCloseModalHost(): () => void {
  modalHostRefs += 1;
  return () => {
    modalHostRefs = Math.max(0, modalHostRefs - 1);
  };
}

function coordinatorCloseModalHostAvailable(): boolean {
  return modalHostRefs > 0;
}

export function __resetCoordinatorCloseModalHostForTests(): void {
  modalHostRefs = 0;
}

export async function requestCoordinatorCloseById(id: string): Promise<void> {
  const s = sessionsStore.sessions.find((x) => x.id === id);
  if (s && !s.isCoordinator) {
    await SessionAPI.destroy(id);
    return;
  }
  try {
    const outcome = await SessionAPI.closeCoordinator(id, false);
    if (!outcome.closed) {
      if (coordinatorCloseModalHostAvailable()) {
        setPendingCoordinatorClose({
          sessionId: id,
          name: s?.name ?? "this coordinator",
          workingCount: outcome.workingCount,
        });
      } else {
        await SessionAPI.destroy(id);
      }
    }
  } catch (err) {
    console.error("close_coordinator failed:", err);
  }
}

export async function requestCoordinatorClose(session: Session): Promise<void> {
  if (!session.isCoordinator) {
    await SessionAPI.destroy(session.id);
    return;
  }
  await requestCoordinatorCloseById(session.id);
}

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
