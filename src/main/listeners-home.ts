import { SessionAPI, onSessionDestroyed, onSessionSwitched } from "../shared/ipc";
import type { UnlistenFn } from "../shared/transport";
import { homeStore } from "./stores/home";

/**
 * Wires the Home auto-visibility contract for the main window (issue #183).
 *
 * On mount: Home is shown unconditionally — every app open lands on Home
 * regardless of any restored active session.
 *
 * Listeners installed:
 * - `session_switched`: hides Home only when the backend marks the switch
 *   as user-initiated (`userInitiated === true`). Restore, auto-close,
 *   liveness reconciliation and other background transitions carry the
 *   required false value, so they leave Home visible. See
 *   `_plans/183-home-first-startup.md`.
 * - `session_destroyed`: shows Home when the LAST session goes away
 *   (issue #164 contract). Yields a microtask so TerminalApp's destroy
 *   handler can settle before we re-query the session list.
 *
 * The `session_created` listener was removed in #183: that event fires for
 * restored AND backend-driven creations (mailbox / web / coordinator
 * spawn), which would tear Home away from the user. User-driven create
 * sites either follow with `SessionAPI.switch(...)` or invoke
 * `homeStore.hide()` imperatively.
 *
 * Returns the unlisten functions so the caller can clean up on unmount.
 */
export async function wireHomeListeners(): Promise<UnlistenFn[]> {
  homeStore.show();

  const unlisteners: UnlistenFn[] = [];

  unlisteners.push(
    await onSessionSwitched(({ id, userInitiated }) => {
      if (id && userInitiated === true) {
        homeStore.hide();
      }
    })
  );

  unlisteners.push(
    await onSessionDestroyed(async () => {
      await Promise.resolve();
      try {
        const remaining = await SessionAPI.list();
        if (remaining.length === 0) {
          homeStore.show();
        }
      } catch (e) {
        console.error("[home] Failed to query session list after destroy:", e);
      }
    })
  );

  return unlisteners;
}
