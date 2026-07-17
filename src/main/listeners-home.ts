import { SessionAPI, onSessionDestroyed, onSessionSwitched } from "../shared/ipc";
import type { UnlistenFn } from "../shared/transport";
import { homeStore } from "./stores/home";

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
