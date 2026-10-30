import { createSignal } from "solid-js";
import { isTauri } from "../../shared/platform";
import { SettingsAPI } from "../../shared/ipc";

// Issue #587 — the main central pane shows one of two views at a time: the
// always-mounted terminal, or the Resource Monitor overlaid on top of it (same
// shape as the Home overlay, see home.ts). This tiny store owns that choice.
// #3367 adds the embedded web browser as a third view; only the Resource
// Monitor choice is persisted.
export type CentralView = "terminal" | "resourceMonitor" | "browser";

const [view, setView] = createSignal<CentralView>("terminal");

// Best-effort persist of the attached-view choice (optimistic; errors are
// logged, never surfaced — mirrors ActionBar's other optimistic persists).
function persist(attached: boolean) {
  SettingsAPI.setMainResourceMonitorAttached(attached).catch((e) =>
    console.error("Failed to persist central-view choice:", e)
  );
}

// Close a detached Resource Monitor window if one exists (enforces the
// embedded-XOR-detached invariant). Best-effort, Tauri-only. The label literal
// must match the backend RESOURCE_MONITOR_WINDOW_LABEL (window.rs).
async function closeDetachedResourceMonitor() {
  if (!isTauri) return;
  try {
    const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
    const win = await WebviewWindow.getByLabel("resource-monitor");
    if (win) await win.destroy();
  } catch (e) {
    console.warn("[centralView] closing detached RM window failed:", e);
  }
}

export const centralViewStore = {
  get view() {
    return view();
  },
  get isResourceMonitor() {
    return view() === "resourceMonitor";
  },
  get isBrowser() {
    return view() === "browser";
  },

  // Restore-only: set the signal without persisting or touching windows.
  // Called once from MainApp.onMount after settings load.
  setInitialView(next: CentralView) {
    setView(next);
  },

  showResourceMonitor() {
    if (view() === "resourceMonitor") return;
    void closeDetachedResourceMonitor();
    setView("resourceMonitor");
    persist(true);
  },

  showTerminal() {
    // Guard against write storms: session_switched fires often, and without
    // this early return every switch would re-persist `false`.
    if (view() === "terminal") return;
    const leavingResourceMonitor = view() === "resourceMonitor";
    setView("terminal");
    if (leavingResourceMonitor) persist(false);
  },

  showBrowser() {
    if (view() === "browser") return;
    const leavingResourceMonitor = view() === "resourceMonitor";
    setView("browser");
    if (leavingResourceMonitor) persist(false);
  },

  toggleResourceMonitor() {
    if (view() === "resourceMonitor") this.showTerminal();
    else this.showResourceMonitor();
  },

  toggleBrowser() {
    if (view() === "browser") this.showTerminal();
    else this.showBrowser();
  },
};

// Test-only reset (mirror __resetHomeStoreForTests). Gated to Vite MODE ===
// "test" so production never resets the signal.
export function __resetCentralViewStoreForTests() {
  if (import.meta.env.MODE !== "test") return;
  setView("terminal");
}
