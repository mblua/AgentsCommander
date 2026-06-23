import { listen } from "@tauri-apps/api/event";
import { SettingsAPI } from "./ipc";
import { setLogLevel } from "./console-capture";
import type { LogLevel } from "./types";

let wired = false;

/**
 * #612 Initialize THIS window's console log-level gate from the persisted
 * setting and keep it live via the backend `log_level_changed` event.
 *
 * Called ONCE from `src/main.tsx` for every window root (main/terminal/guide/
 * browser/spec-board/resource-monitor). It does NOT depend on `settingsStore`:
 * it reads the level via `SettingsAPI.get()` directly, so it works for windows
 * (guide, spec-board) that never hydrate the store. The `wired` guard makes it
 * idempotent, and both async calls are wrapped in try/catch so a non-Tauri /
 * pure-web context (where `listen` may be unavailable) falls back to a static
 * Info level without throwing.
 */
export async function initLogLevelForWindow(): Promise<void> {
  if (wired) return;
  wired = true;
  try {
    const s = await SettingsAPI.get();
    setLogLevel((s.logLevel as LogLevel | null) ?? "info");
  } catch {
    /* keep default Info */
  }
  try {
    await listen<{ level: LogLevel }>("log_level_changed", (e) => {
      setLogLevel((e.payload?.level as LogLevel) ?? "info");
    });
  } catch {
    /* event bus unavailable (e.g. pure web-remote) -> static level, fine */
  }
}
