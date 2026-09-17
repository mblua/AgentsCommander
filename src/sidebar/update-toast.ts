import { toastStore } from "../shared/stores/toasts";
import type { UpdateInfo } from "../shared/types";

/**
 * #2135 — `navigator.clipboard` is undefined in a non-secure context / a webview
 * without the permission, so the property access itself throws SYNCHRONOUSLY,
 * before any promise exists. A dangling `.catch()` would not see it and the
 * TypeError would escape into ToastHost's click handler. try/catch around the
 * await covers both that and the rejection, like ExternalLinkConfirm.tsx:52.
 */
async function copyUpgradeCommand(command: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(command);
    toastStore.success("Command copied");
  } catch (err) {
    console.error("[update-toast] clipboard write failed:", err);
  }
}

/**
 * #609 — build a sticky "npm update available" toaster with per-version dedup.
 *
 * The sidebar subscribes to the `npm_update_available` event BEFORE it snapshots
 * `get_update_status`, so a startup emit fired during mount is never dropped.
 * That means the same `UpdateInfo` can arrive twice (event + snapshot). The
 * returned closure shows the info toast at most once per `latestVersion`, so the
 * race never double-toasts. Each factory call owns its own dedup state, so a
 * fresh mount starts clean (and unit tests stay isolated).
 */
export function createUpdateToaster(): (info: UpdateInfo) => void {
  let lastVersion: string | null = null;
  return (info: UpdateInfo): void => {
    if (lastVersion === info.latestVersion) return;
    lastVersion = info.latestVersion;
    // O2: English, sticky (durationMs: null) so the upgrade command stays
    // readable until the user dismisses it.
    toastStore.info(
      `Update available: v${info.latestVersion} (you have v${info.currentVersion}). Run: ${info.upgradeCommand}`,
      {
        durationMs: null,
        // #2135: copying must NOT dismiss the sticky toast — the command stays
        // readable, and the copy can be repeated.
        action: {
          label: "Copy",
          dismissOnClick: false,
          onClick: () => {
            void copyUpgradeCommand(info.upgradeCommand);
          },
        },
      },
    );
  };
}
