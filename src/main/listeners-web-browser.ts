import { onWebBrowserOpenRequested, onWebBrowserState } from "../shared/ipc";
import type { UnlistenFn } from "../shared/transport";
import { centralViewStore } from "./stores/centralView";
import { webBrowserStore } from "./stores/webBrowser";

/**
 * Wires the #3367 embedded web browser events for the main window.
 *
 * - `web_browser_state`: mirrors URL, title and loading into the store, also
 *   while the browser view is hidden, so the URL bar is current on reopen.
 * - `web_browser_open_requested`: an orchestrator navigated through the CLI
 *   while the view was hidden; bring the browser over the terminal pane.
 */
export async function wireWebBrowserListeners(): Promise<UnlistenFn[]> {
  return [
    await onWebBrowserState((snapshot) => webBrowserStore.apply(snapshot)),
    await onWebBrowserOpenRequested((snapshot) => {
      webBrowserStore.apply(snapshot);
      centralViewStore.showBrowser();
    }),
  ];
}
