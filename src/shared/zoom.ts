import { getCurrentWebview } from "@tauri-apps/api/webview";
import { SettingsAPI } from "./ipc";
import type { AppSettings } from "./types";

const ZOOM_STEP = 0.1;
const ZOOM_MIN = 0.5;
const ZOOM_MAX = 3.0;

let currentZoom = 1.0;
let saveTimeout: ReturnType<typeof setTimeout> | null = null;

type WindowType = "sidebar" | "terminal" | "guide" | "darkfactory";

const zoomKeyMap: Record<WindowType, keyof AppSettings> = {
  sidebar: "sidebarZoom",
  terminal: "terminalZoom",
  guide: "guideZoom",
  darkfactory: "darkfactoryZoom",
};

function clampZoom(value: number): number {
  return Math.round(Math.max(ZOOM_MIN, Math.min(ZOOM_MAX, value)) * 100) / 100;
}

async function applyZoom(zoom: number) {
  currentZoom = clampZoom(zoom);
  await getCurrentWebview().setZoom(currentZoom);
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

function handleWheel(windowType: WindowType, e: WheelEvent) {
  if (!e.ctrlKey) return;
  e.preventDefault();
  const delta = e.deltaY < 0 ? ZOOM_STEP : -ZOOM_STEP;
  applyZoom(currentZoom + delta);
  debouncedSave(windowType);
}

function handleKeydown(windowType: WindowType, e: KeyboardEvent) {
  if (!e.ctrlKey || e.shiftKey || e.altKey) return;

  if (e.key === "=" || e.key === "+") {
    e.preventDefault();
    applyZoom(currentZoom + ZOOM_STEP);
    debouncedSave(windowType);
  } else if (e.key === "-") {
    e.preventDefault();
    applyZoom(currentZoom - ZOOM_STEP);
    debouncedSave(windowType);
  } else if (e.key === "0") {
    e.preventDefault();
    applyZoom(1.0);
    debouncedSave(windowType);
  }
}

/**
 * Initialize zoom: restore saved level and attach keyboard/wheel listeners.
 * Returns a cleanup function for onCleanup.
 */
export async function initZoom(windowType: WindowType): Promise<() => void> {
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

  const onWheel = (e: WheelEvent) => handleWheel(windowType, e);
  const onKeydown = (e: KeyboardEvent) => handleKeydown(windowType, e);

  document.addEventListener("wheel", onWheel, { passive: false });
  document.addEventListener("keydown", onKeydown);

  return () => {
    document.removeEventListener("wheel", onWheel);
    document.removeEventListener("keydown", onKeydown);
    if (saveTimeout) clearTimeout(saveTimeout);
  };
}
