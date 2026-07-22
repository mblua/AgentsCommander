import { SettingsAPI } from "./ipc";
import type { AppSettings } from "./types";
import { isTauri } from "./platform";

// NOTE: main window is the only zoom initializer for the unified app. Embedded
// Sidebar + Terminal MUST skip initZoom() so Ctrl+= does not register two
// wheel/keydown handlers with independent currentZoom values. Detached windows
// use "detached" (mapped to terminalZoom). See plan §A2.11.N1 and DW.2
// embedded-mode contract.

export const ZOOM_STEP = 0.1;
export const ZOOM_MIN = 0.5;
export const ZOOM_MAX = 3.0;

export type WindowType = "sidebar" | "terminal" | "main" | "detached" | "guide";

const zoomKeyMap: Record<WindowType, keyof AppSettings> = {
  sidebar: "sidebarZoom",
  terminal: "terminalZoom",
  main: "mainZoom",
  detached: "terminalZoom",
  guide: "guideZoom",
};

export interface ZoomState {
  zoom: number;
  windowType: WindowType | null;
}

type ZoomListener = (state: ZoomState) => void;

let currentZoom = 1.0;
let activeWindowType: WindowType | null = null;
let saveTimeout: ReturnType<typeof setTimeout> | null = null;
const zoomListeners = new Set<ZoomListener>();

export function getZoomState(): ZoomState {
  return { zoom: currentZoom, windowType: activeWindowType };
}

export function onZoomChange(listener: ZoomListener): () => void {
  zoomListeners.add(listener);
  return () => zoomListeners.delete(listener);
}

function notifyZoomChange() {
  const state = getZoomState();
  for (const listener of Array.from(zoomListeners)) listener(state);
}

function clampZoom(value: number): number {
  return Math.round(Math.max(ZOOM_MIN, Math.min(ZOOM_MAX, value)) * 100) / 100;
}

async function applyZoom(zoom: number) {
  currentZoom = clampZoom(zoom);
  if (isTauri) {
    const { getCurrentWebview } = await import("@tauri-apps/api/webview");
    await getCurrentWebview().setZoom(currentZoom);
  } else {
    // Browser mode: apply zoom to #root, not <html>.
    // Zooming <html> breaks xterm FitAddon measurements because it calculates
    // cell counts against pre-zoom container dimensions, producing content
    // that overflows when rendered at the zoomed scale.
    document.documentElement.style.zoom = "";
    const root = document.getElementById("root");
    if (root) root.style.zoom = String(currentZoom);
  }
  notifyZoomChange();
}

function debouncedSave(windowType: WindowType) {
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

export async function setUiZoom(zoom: number): Promise<void> {
  const windowType = activeWindowType;
  await applyZoom(zoom);
  if (windowType) debouncedSave(windowType);
}

function handleZoomError(err: unknown) {
  console.error("Failed to change zoom:", err);
}

/**
 * Initialize zoom: restore saved level and attach keyboard/wheel listeners.
 * Returns a cleanup function for onCleanup.
 */
export async function initZoom(windowType: WindowType): Promise<() => void> {
  activeWindowType = windowType;

  // Restore saved zoom
  try {
    const settings = await SettingsAPI.get();
    const saved = settings[zoomKeyMap[windowType]] as number;
    if (typeof saved === "number") {
      await applyZoom(saved);
    }
  } catch (e) {
    console.error("Failed to restore zoom:", e);
  }

  const onWheel = (e: WheelEvent) => {
    if (!e.ctrlKey) return;
    e.preventDefault();
    const delta = e.deltaY < 0 ? ZOOM_STEP : -ZOOM_STEP;
    void setUiZoom(currentZoom + delta).catch(handleZoomError);
  };

  const onKeydown = (e: KeyboardEvent) => {
    if (!(e.ctrlKey || e.metaKey) || e.shiftKey || e.altKey) return;
    if (e.key === "=" || e.key === "+") {
      e.preventDefault();
      void setUiZoom(currentZoom + ZOOM_STEP).catch(handleZoomError);
    } else if (e.key === "-") {
      e.preventDefault();
      void setUiZoom(currentZoom - ZOOM_STEP).catch(handleZoomError);
    } else if (e.key === "0") {
      e.preventDefault();
      void setUiZoom(1.0).catch(handleZoomError);
    }
  };

  document.addEventListener("wheel", onWheel, { passive: false });
  document.addEventListener("keydown", onKeydown);

  return () => {
    document.removeEventListener("wheel", onWheel);
    document.removeEventListener("keydown", onKeydown);
    if (saveTimeout) clearTimeout(saveTimeout);
    if (activeWindowType === windowType) activeWindowType = null;
  };
}
