import { SettingsAPI, WindowAPI } from "./ipc";
import type { AppSettings, MainWindowDisplayState, WindowGeometry } from "./types";
import { isTauri } from "./platform";

type WindowType = "sidebar" | "terminal" | "main";
type PersistedWindowType = Exclude<WindowType, "main">;

const geometryKeyMap: Record<PersistedWindowType, keyof AppSettings> = {
  sidebar: "sidebarGeometry",
  terminal: "terminalGeometry",
};

/** #2349 - the quit path bounds its placement flush so a stuck invoke cannot
 *  trap the user; at the deadline the flush settles as `failed` and quit
 *  continues with the previous durable placement. */
export const MAIN_WINDOW_QUIT_FLUSH_TIMEOUT_MS = 2_000;

const MAIN_WINDOW_GEOMETRY_DEBOUNCE_MS = 500;

/** The exact rejection Rust returns when the local settings overlay owns either
 *  placement key. Every other failure stays an ordinary diagnostic. */
const OVERLAY_PINNED_ERROR = "main_window_placement_overlay_pinned";

export type MainWindowGeometryFlushResult =
  | { kind: "saved" }
  | { kind: "noop" }
  | { kind: "overlay-pinned" }
  | { kind: "failed" };

async function readGeometry(): Promise<WindowGeometry> {
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
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

interface MainWindowObservation {
  geometry: WindowGeometry;
  isMaximized: boolean;
  isFullscreen: boolean;
  isMinimized: boolean;
}

interface MainWindowGeometryController {
  dispose: () => void;
  flush: () => Promise<MainWindowGeometryFlushResult>;
}

/** The module-owned main-window controller. Sidebar and terminal keep the plain
 *  callable cleanup; only the main window exposes the quit flush. */
let mainWindowGeometryController: MainWindowGeometryController | null = null;
let pendingMainGeometryFlush: Promise<MainWindowGeometryFlushResult> | null = null;

function isValidGeometry(
  value: WindowGeometry | null | undefined,
): value is WindowGeometry {
  return (
    value !== null &&
    value !== undefined &&
    Number.isFinite(value.x) &&
    Number.isFinite(value.y) &&
    Number.isFinite(value.width) &&
    Number.isFinite(value.height) &&
    value.width > 0 &&
    value.height > 0
  );
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** One observation samples the display flags and the physical rectangle
 *  together, so a special-state rect can never masquerade as normal bounds. */
async function observeMainWindow(): Promise<MainWindowObservation> {
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const win = getCurrentWindow();
  const [isMaximized, isFullscreen, isMinimized, position, size] = await Promise.all([
    win.isMaximized(),
    win.isFullscreen(),
    win.isMinimized(),
    win.outerPosition(),
    win.outerSize(),
  ]);
  return {
    geometry: {
      x: position.x,
      y: position.y,
      width: size.width,
      height: size.height,
    },
    isMaximized,
    isFullscreen,
    isMinimized,
  };
}

/** #2393 - the rectangle an observation may contribute as normal bounds, or
 *  `null` when the window is in a special state or the rectangle is unusable.
 *  Shared by the fold and by the startup sample so both obey one rule. */
function normalBoundsOf(
  observation: MainWindowObservation,
): WindowGeometry | null {
  if (
    observation.isMaximized ||
    observation.isFullscreen ||
    observation.isMinimized
  ) {
    return null;
  }
  return isValidGeometry(observation.geometry)
    ? { ...observation.geometry }
    : null;
}

async function createMainWindowGeometryController(): Promise<MainWindowGeometryController> {
  // The retained rectangle is seeded from persisted normal bounds only. A
  // special-state observation above never feeds it.
  let retainedBounds: WindowGeometry | null = null;
  try {
    const settings = await SettingsAPI.get();
    if (isValidGeometry(settings.mainGeometry)) {
      retainedBounds = { ...settings.mainGeometry };
    }
  } catch (error) {
    console.error("Failed to seed main window geometry:", error);
  }
  let retainedDisplayState: MainWindowDisplayState = "normal";
  let saveTimeout: ReturnType<typeof setTimeout> | null = null;
  let disposed = false;

  const foldObservation = (observation: MainWindowObservation): void => {
    if (observation.isFullscreen || observation.isMinimized) {
      // Transient: keep the pre-fullscreen / last non-minimized placement.
      return;
    }
    retainedDisplayState = observation.isMaximized ? "maximized" : "normal";
    const bounds = normalBoundsOf(observation);
    if (bounds !== null) {
      retainedBounds = bounds;
    }
  };

  const persistRetainedPlacement = async (): Promise<void> => {
    const geometry = retainedBounds;
    if (geometry === null) {
      return;
    }
    await WindowAPI.setMainWindowPlacement({ ...geometry }, retainedDisplayState);
  };

  const observeAndPersist = async (): Promise<void> => {
    foldObservation(await observeMainWindow());
    await persistRetainedPlacement();
  };

  const scheduleSave = (): void => {
    if (disposed) {
      return;
    }
    if (saveTimeout !== null) {
      clearTimeout(saveTimeout);
    }
    saveTimeout = setTimeout(() => {
      saveTimeout = null;
      void observeAndPersist().catch((error) => {
        console.error("Failed to save main window geometry:", error);
      });
    }, MAIN_WINDOW_GEOMETRY_DEBOUNCE_MS);
  };

  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const win = getCurrentWindow();

  // #2393 - with no valid persisted rectangle, sample the window once before the
  // listeners attach. Otherwise a first run whose first event arrives maximized
  // retains no rectangle at all and persists nothing, not on the debounce and not
  // on the quit flush. The special-state rule is unchanged: a maximized,
  // fullscreen or minimized sample contributes nothing.
  if (retainedBounds === null) {
    try {
      retainedBounds = normalBoundsOf(await observeMainWindow());
    } catch (error) {
      console.error("Failed to sample main window placement at startup:", error);
    }
  }

  const unlistenMove = await win.onMoved(scheduleSave);
  const unlistenResize = await win.onResized(scheduleSave);

  /** Cancels the coalescing timer, persists the latest observation through the
   *  narrow command, and settles no later than the two-second deadline. A late
   *  transport settlement is detached: it may log, but it cannot resolve this
   *  promise, alert, or clear a newer attempt's slot. */
  const flush = (): Promise<MainWindowGeometryFlushResult> => {
    if (saveTimeout !== null) {
      clearTimeout(saveTimeout);
      saveTimeout = null;
    }
    return new Promise<MainWindowGeometryFlushResult>((resolve) => {
      let settled = false;
      const deadline = setTimeout(() => {
        if (settled) {
          return;
        }
        settled = true;
        console.error(
          `Main window geometry flush timed out after ${MAIN_WINDOW_QUIT_FLUSH_TIMEOUT_MS} ms; quit continues with the previous placement.`,
        );
        resolve({ kind: "failed" });
      }, MAIN_WINDOW_QUIT_FLUSH_TIMEOUT_MS);
      const settle = (result: MainWindowGeometryFlushResult): void => {
        if (settled) {
          return;
        }
        settled = true;
        clearTimeout(deadline);
        resolve(result);
      };
      void (async () => {
        let observation: MainWindowObservation;
        try {
          observation = await observeMainWindow();
        } catch (error) {
          console.error("Failed to read main window placement on quit:", error);
          settle({ kind: "failed" });
          return;
        }
        foldObservation(observation);
        if (retainedBounds === null) {
          settle({ kind: "noop" });
          return;
        }
        try {
          await persistRetainedPlacement();
        } catch (error) {
          if (settled) {
            console.error(
              "Main window placement save settled after the quit flush deadline:",
              error,
            );
            return;
          }
          if (errorText(error) === OVERLAY_PINNED_ERROR) {
            settle({ kind: "overlay-pinned" });
          } else {
            console.error("Failed to save main window placement on quit:", error);
            settle({ kind: "failed" });
          }
          return;
        }
        settle({ kind: "saved" });
      })();
    });
  };

  const dispose = (): void => {
    if (disposed) {
      return;
    }
    disposed = true;
    unlistenMove();
    unlistenResize();
    if (saveTimeout !== null) {
      clearTimeout(saveTimeout);
      saveTimeout = null;
    }
  };

  return { dispose, flush };
}

/**
 * #2349 - the bounded placement flush every accepted close route awaits before
 * its first quit command. When no main controller exists (browser build, or
 * after cleanup) it resolves `noop`. Concurrent calls share one bounded promise
 * and result; the slot clears when that promise settles, so a later accepted
 * close attempt starts a fresh invoke from the controller's latest observation.
 */
export function flushMainWindowGeometry(): Promise<MainWindowGeometryFlushResult> {
  const controller = mainWindowGeometryController;
  if (controller === null) {
    return Promise.resolve({ kind: "noop" });
  }
  if (pendingMainGeometryFlush !== null) {
    return pendingMainGeometryFlush;
  }
  const pending = controller.flush().finally(() => {
    if (pendingMainGeometryFlush === pending) {
      pendingMainGeometryFlush = null;
    }
  });
  pendingMainGeometryFlush = pending;
  return pending;
}

export async function initWindowGeometry(
  windowType: WindowType
): Promise<() => void> {
  if (!isTauri) {
    return () => {};
  }

  if (windowType === "main") {
    const controller = await createMainWindowGeometryController();
    const previous = mainWindowGeometryController;
    mainWindowGeometryController = controller;
    previous?.dispose();
    return () => {
      controller.dispose();
      if (mainWindowGeometryController === controller) {
        mainWindowGeometryController = null;
      }
    };
  }

  let saveTimeout: ReturnType<typeof setTimeout> | null = null;
  const key = geometryKeyMap[windowType];

  const debouncedSave = () => {
    if (saveTimeout) clearTimeout(saveTimeout);
    saveTimeout = setTimeout(async () => {
      try {
        const geo = await readGeometry();
        const settings = await SettingsAPI.get();
        await SettingsAPI.update({ ...settings, [key]: geo });
      } catch (e) {
        console.error("Failed to save window geometry:", e);
      }
    }, MAIN_WINDOW_GEOMETRY_DEBOUNCE_MS);
  };

  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const win = getCurrentWindow();

  const unlistenMove = await win.onMoved(() => debouncedSave());
  const unlistenResize = await win.onResized(() => debouncedSave());

  return () => {
    unlistenMove();
    unlistenResize();
    if (saveTimeout) clearTimeout(saveTimeout);
  };
}

export async function initDetachedWindowGeometry(
  sessionId: string
): Promise<() => void> {
  if (!isTauri) return () => {};

  let saveTimeout: ReturnType<typeof setTimeout> | null = null;

  const debouncedSave = () => {
    if (saveTimeout) clearTimeout(saveTimeout);
    saveTimeout = setTimeout(async () => {
      try {
        const geo = await readGeometry();
        await WindowAPI.setDetachedGeometry(sessionId, geo);
      } catch (e) {
        console.error("Failed to save detached window geometry:", e);
      }
    }, MAIN_WINDOW_GEOMETRY_DEBOUNCE_MS);
  };

  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const win = getCurrentWindow();

  const unlistenMove = await win.onMoved(() => debouncedSave());
  const unlistenResize = await win.onResized(() => debouncedSave());

  return () => {
    unlistenMove();
    unlistenResize();
    if (saveTimeout) clearTimeout(saveTimeout);
  };
}
