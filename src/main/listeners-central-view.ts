import { onResourceMonitorAttach, onSessionSwitched } from "../shared/ipc";
import type { UnlistenFn } from "../shared/transport";
import { centralViewStore } from "./stores/centralView";

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
