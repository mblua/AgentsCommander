import { isSelectionCoordinatorBusyError, SessionAPI } from "../shared/ipc";
import { toastStore } from "../shared/stores/toasts";
import type { Session, SessionWarning } from "../shared/types";
import { normalizeProjectPathForCompare } from "./stores/project-refresh";
import { sessionsStore } from "./stores/sessions";

type DepartedSession = { name: string; workingDirectory: string };

const MAX_DEPARTED = 64;
const departed = new Map<string, DepartedSession>();
let pendingList: Promise<Session[]> | null = null;

/** Remember a session's name before it leaves the store, so a later warning can name it. */
export function rememberDestroyedSession(id: string): void {
  const row = sessionsStore.sessions.find((s) => s.id === id);
  if (!row) return;
  departed.delete(id);
  departed.set(id, { name: row.name, workingDirectory: row.workingDirectory });
  while (departed.size > MAX_DEPARTED) {
    const oldest = departed.keys().next().value;
    if (oldest === undefined) break;
    departed.delete(oldest);
  }
}

function sharedSessionList(): Promise<Session[]> {
  if (pendingList) return pendingList;
  const p: Promise<Session[]> = SessionAPI.list()
    .catch(() => [] as Session[])
    .finally(() => {
      if (pendingList === p) pendingList = null;
    });
  pendingList = p;
  return p;
}

function findByWorkingDirectory(rows: readonly Session[], dir: string): Session | undefined {
  const wanted = normalizeProjectPathForCompare(dir);
  return rows.find((s) => normalizeProjectPathForCompare(s.workingDirectory) === wanted);
}

function seeTerminalAction(name: string, target: string) {
  return {
    label: "See terminal",
    dismissOnClick: false,
    onClick: () => {
      SessionAPI.switch(target).catch((e: unknown) => {
        toastStore.info(
          isSelectionCoordinatorBusyError(e)
            ? `${name}: busy, try again`
            : `${name}: could not open the terminal`,
        );
      });
    },
  };
}

/** Show a session warning naming its agent, with a "See terminal" action when one is live. */
export async function showSessionWarningToast(
  warning: SessionWarning,
  isDisposed: () => boolean,
): Promise<void> {
  const id = warning.sessionId;
  let listed: Session[] = [];
  let live = sessionsStore.sessions.find((s) => s.id === id);
  if (!live) {
    listed = await sharedSessionList();
    live = listed.find((s) => s.id === id);
  }
  if (isDisposed()) return;

  let name: string;
  let target: string | undefined;
  const remembered = departed.get(id);
  if (live) {
    name = live.name;
    target = live.id;
  } else if (remembered) {
    name = remembered.name;
    target = (
      findByWorkingDirectory(sessionsStore.sessions, remembered.workingDirectory) ??
      findByWorkingDirectory(listed, remembered.workingDirectory)
    )?.id;
  } else {
    name = `Unknown agent (${id.slice(0, 8)})`;
  }

  const message = `${name}: ${warning.message}`;
  toastStore.error(
    message,
    target ? { secondaryAction: seeTerminalAction(name, target) } : undefined,
  );
}

export function resetSessionWarningToastForTests(): void {
  departed.clear();
  pendingList = null;
}
