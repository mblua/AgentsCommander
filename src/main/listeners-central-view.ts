import { onResourceMonitorAttach, onSessionSwitched } from "../shared/ipc";
import type { UnlistenFn } from "../shared/transport";
import { centralViewStore } from "./stores/centralView";

/**
 * Wires the #587 central-view event contract for the main window. Extracted
 * into a helper (mirrors `wireHomeListeners`) so the `userInitiated`
 * discriminator on `session_switched` is unit-testable in isolation.
 *
 * Listeners installed:
 * - `session_switched`: reveal the terminal (cover an embedded RM) ONLY when
 *   the backend marks the switch user-initiated (`userInitiated === true`) and
 *   it carries a real id. A non-user transition — boot restore, auto-close,
 *   background cleanup, or liveness reconciliation — must NOT
 *   flip away from RM: `centralViewStore.showTerminal()` persists `false`, which
 *   would defeat the restored `mainResourceMonitorAttached` choice (plan §6) and
 *   yank the RM view away mid-use. This mirrors the Home listener's discriminator
 *   (`listeners-home.ts`). Explicit user clicks are additionally covered by
 *   `SessionItem.handleClick`, which calls `showTerminal()` directly (so an
 *   already-active click, which emits no `session_switched`, still covers RM).
 * - `resource_monitor_attach`: the detached RM window asks the main window to
 *   pull RM back into the central pane.
 *
 * Returns the unlisten functions so the caller can clean up on unmount.
 */
export async function wireCentralViewListeners(): Promise<UnlistenFn[]> {
  const unlisteners: UnlistenFn[] = [];

  unlisteners.push(
    await onSessionSwitched(({ id, userInitiated }) => {
      if (id && userInitiated === true) centralViewStore.showTerminal();
    })
  );

  unlisteners.push(
    await onResourceMonitorAttach(() => centralViewStore.showResourceMonitor())
  );

  return unlisteners;
}
