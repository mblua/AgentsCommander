/// #1857: ask the OS to flash the taskbar button when a session starts waiting
/// on a blocking menu while the AgentsCommander window is not focused.
///
/// This mirrors the guard and the dynamic import in
/// `src/main/components/ErrorModal.tsx`. It lives in `src/sidebar/` because that
/// layer already owns the window surface; calling it from
/// `src/shared/stores/toasts.ts` would give a store a UI-transport dependency.
import { isTauri } from "../shared/platform";

export async function requestTaskbarAttention(): Promise<void> {
  if (!isTauri) return;
  if (typeof document !== "undefined" && document.hasFocus()) return;
  try {
    const { getCurrentWindow, UserAttentionType } = await import("@tauri-apps/api/window");
    await getCurrentWindow().requestUserAttention(UserAttentionType.Critical);
  } catch (err) {
    console.error("[blocked-menu] requestUserAttention failed:", err);
  }
}
