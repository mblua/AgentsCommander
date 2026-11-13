import type { UnlistenFn } from "../shared/transport";
import {
  ScreenshotAPI,
  onScreenshotCaptureFailed,
  onScreenshotCaptureSaved,
  onScreenshotHotkeyStatusChanged,
} from "../shared/ipc";
import type { ScreenshotHotkeyStatus } from "../shared/types";
import { toastStore } from "../shared/stores/toasts";

export const SCREENSHOT_TOAST_DURATION_MS = 30_000;

/**
 * #714 Wire the sidebar's screenshot-capture notifications:
 *  - `screenshot_capture_saved` → a success toast carrying the saved PNG path
 *    (which is also on the clipboard), auto-dismissing after 30s.
 *  - `screenshot_capture_failed` → a sticky error toast (the global hotkey often
 *    fires while another app is focused, so failures must be un-missable).
 *  - a startup check plus `screenshot_hotkey_status_changed` events (#2774): if
 *    the hotkey is configured but not registered (e.g. another app owns the
 *    combo), warn so the feature is not silently dead — one toast per distinct
 *    `configured + error`; a registered status re-arms the warning.
 *
 * Returns the listener unsubscribers for the caller to push onto its cleanup
 * list. Extracted into its own module (mirroring `src/main/listeners-*.ts`) so
 * the toast wiring is unit-testable without rendering the whole sidebar.
 */
export async function wireScreenshotListeners(): Promise<UnlistenFn[]> {
  const unlisteners: UnlistenFn[] = [];
  let lastWarning: string | null = null;

  const warnIfInactive = (status: ScreenshotHotkeyStatus) => {
    if (status.registered) { lastWarning = null; return; }
    if (!status.error) return;
    const key = `${status.configured}\n${status.error}`;
    if (key === lastWarning) return;
    lastWarning = key;
    toastStore.error(`Screenshot hotkey ${status.configured} is not active: ${status.error}`,
      { durationMs: null });
  };

  unlisteners.push(
    await onScreenshotCaptureSaved((result) => {
      toastStore.success(`Screenshot saved. Path copied: ${result.path}`, {
        durationMs: SCREENSHOT_TOAST_DURATION_MS,
      });
    }),
    await onScreenshotCaptureFailed(({ message }) => {
      toastStore.error(message, { durationMs: null });
    }),
    await onScreenshotHotkeyStatusChanged(warnIfInactive)
  );

  // Fire-and-forget so it never delays mount; shares the dedup with events.
  void ScreenshotAPI.getHotkeyStatus()
    .then(warnIfInactive)
    .catch((err) => {
      console.error("[screenshot] getHotkeyStatus failed:", err);
    });

  return unlisteners;
}
