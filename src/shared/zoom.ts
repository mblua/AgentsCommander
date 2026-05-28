import { SettingsAPI } from "./ipc";
import type { AppSettings } from "./types";
import { isTauri } from "./platform";

// NOTE: main window is the only zoom initializer for the unified app. Embedded
// Sidebar + Terminal MUST skip initZoom() - otherwise Ctrl+= registers two
// wheel/keydown handlers with independent currentZoom closures and the values
// race. Detached windows use "detached" (mapped to terminalZoom). See plan
// §A2.11.N1 and DW.2 embedded-mode contract.

const ZOOM_STEP = 0.1;
const ZOOM_MIN = 0.5;
const ZOOM_MAX = 3.0;

type WindowType = "sidebar" | "terminal" | "main" | "detached" | "guide";

const zoomKeyMap: Record<WindowType, keyof AppSettings> = {
  sidebar: "sidebarZoom",
  terminal: "terminalZoom",
  main: "mainZoom",
  detached: "terminalZoom",
  guide: "guideZoom",
};

function clampZoom(value: number): number {
  return Math.round(Math.max(ZOOM_MIN, Math.min(ZOOM_MAX, value)) * 100) / 100;
}

/**
 * Initialize zoom: restore saved level and attach keyboard/wheel listeners.
 * All mutable state is closure-local so multiple calls (BrowserApp) don't conflict.
 * Returns a cleanup function for onCleanup.
 */
export async function initZoom(windowType: WindowType): Promise<() => void> {
  let currentZoom = 1.0;
  let saveTimeout: ReturnType<typeof setTimeout> | null = null;

  async function applyZoom(zoom: number) {
    currentZoom = clampZoom(zoom);
    if (isTauri) {
      const { getCurrentWebview } = await import("@tauri-apps/api/webview");
      await getCurrentWebview().setZoom(currentZoom);
    } else {
      // Browser mode: apply zoom to #root, not <html>.
      // Zooming <html> breaks xterm FitAddon measurements - it calculates
      // cell counts against pre-zoom container dimensions, producing content
      // that overflows when rendered at the zoomed scale.
      document.documentElement.style.zoom = "";
      const root = document.getElementById("root");
      if (root) root.style.zoom = String(currentZoom);
    }
  }

  function debouncedSave() {
    if (saveTimeout) clearTimeout(saveTimeout);
    saveTimeout = setTimeout(async () => {
      try {
        const settings = await SettingsAPI.get();
        const key = zoomKeyMap[windowType];
        if (settings[key] !== currentZoom) {
          await SettingsAPI.update({ ...settings, [key]: currentZoom });
        }
      } catch (e) {
        console.error("Failed to save zoom:", e);
      }
    }, 500);
  }

  // Restore saved zoom
  try {
    const settings = await SettingsAPI.get();
    const saved = settings[zoomKeyMap[windowType]] as number;
    if (saved && saved !== 1.0) {
      await applyZoom(saved);
    }
  } catch (e) {
    console.error("Failed to restore zoom:", e);
  }

  const onWheel = (e: WheelEvent) => {
    if (!e.ctrlKey) return;
    e.preventDefault();
    const delta = e.deltaY < 0 ? ZOOM_STEP : -ZOOM_STEP;
    applyZoom(currentZoom + delta);
    debouncedSave();
  };

  const onKeydown = (e: KeyboardEvent) => {
    if (!e.ctrlKey || e.shiftKey || e.altKey) return;
    if (e.key === "=" || e.key === "+") {
      e.preventDefault();
      applyZoom(currentZoom + ZOOM_STEP);
      debouncedSave();
    } else if (e.key === "-") {
      e.preventDefault();
      applyZoom(currentZoom - ZOOM_STEP);
      debouncedSave();
    } else if (e.key === "0") {
      e.preventDefault();
      applyZoom(1.0);
      debouncedSave();
    }
  };

  document.addEventListener("wheel", onWheel, { passive: false });
  document.addEventListener("keydown", onKeydown);

  return () => {
    document.removeEventListener("wheel", onWheel);
    document.removeEventListener("keydown", onKeydown);
    if (saveTimeout) clearTimeout(saveTimeout);
  };
}
