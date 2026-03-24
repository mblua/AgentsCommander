import { getCurrentWindow } from "@tauri-apps/api/window";
import { SettingsAPI } from "./ipc";
import type { AppSettings, WindowGeometry } from "./types";

type WindowType = "sidebar" | "terminal";

let saveTimeout: ReturnType<typeof setTimeout> | null = null;

async function readGeometry(): Promise<WindowGeometry> {
  const win = getCurrentWindow();
  const pos = await win.outerPosition();
  const size = await win.outerSize();
  return {
    x: pos.x,
    y: pos.y,
    width: size.width,
    height: size.height,
  };
}

function debouncedSave(windowType: WindowType) {
  if (saveTimeout) clearTimeout(saveTimeout);
  saveTimeout = setTimeout(async () => {
    try {
      const geo = await readGeometry();
      const settings = await SettingsAPI.get();
      const key: keyof AppSettings =
        windowType === "sidebar" ? "sidebarGeometry" : "terminalGeometry";
      await SettingsAPI.update({ ...settings, [key]: geo });
    } catch (e) {
      console.error("Failed to save window geometry:", e);
    }
  }, 500);
}

/**
 * Track window move/resize and persist geometry.
 * Returns a cleanup function for onCleanup.
 */
export async function initWindowGeometry(
  windowType: WindowType
): Promise<() => void> {
  const win = getCurrentWindow();

  const unlistenMove = await win.onMoved(() => debouncedSave(windowType));
  const unlistenResize = await win.onResized(() => debouncedSave(windowType));

  return () => {
    unlistenMove();
    unlistenResize();
    if (saveTimeout) clearTimeout(saveTimeout);
  };
}
