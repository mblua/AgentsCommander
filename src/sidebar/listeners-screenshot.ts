import type { UnlistenFn } from "../shared/transport";
import {
  ScreenshotAPI,
  onScreenshotCaptureFailed,
  onScreenshotCaptureSaved,
} from "../shared/ipc";
import { toastStore } from "../shared/stores/toasts";

/**
 * #714 Wire the sidebar's screenshot-capture notifications:
 *  - `screenshot_capture_saved` → a sticky success toast carrying the saved PNG
 *    path (which is also on the clipboard).
 *  - `screenshot_capture_failed` → a sticky error toast (the global hotkey often
 *    fires while another app is focused, so failures must be un-missable).
 *  - a one-shot startup check: if the hotkey is configured but the OS refused to
 *    register it (e.g. another app owns the combo), warn once so the feature is
 *    not silently dead.
 *
 * Returns the listener unsubscribers for the caller to push onto its cleanup
 * list. Extracted into its own module (mirroring `src/main/listeners-*.ts`) so
 * the toast wiring is unit-testable without rendering the whole sidebar.
 */
export async function wireScreenshotListeners(): Promise<UnlistenFn[]> {
  const unlisteners: UnlistenFn[] = [];

  unlisteners.push(
    await onScreenshotCaptureSaved((result) => {
      toastStore.success(`Screenshot saved. Path copied: ${result.path}`, {
        durationMs: null,
      });
    })
  );

  unlisteners.push(
    await onScreenshotCaptureFailed(({ message }) => {
      toastStore.error(message, { durationMs: null });
    })
  );

  // Fire-and-forget so it never delays mount; a hotkey conflict is surfaced once.
  void ScreenshotAPI.getHotkeyStatus()
    .then((status) => {
      if (!status.registered && status.error) {
        toastStore.error(
          `Screenshot hotkey ${status.configured} is not active: ${status.error}`,
          { durationMs: null }
        );
      }
    })
    .catch((err) => {
      console.error("[screenshot] getHotkeyStatus failed:", err);
    });

  return unlisteners;
}
