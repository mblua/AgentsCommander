import { Component, Show, For, createSignal, createMemo, onMount, onCleanup } from "solid-js";
import iconUrl from "../../assets/icon-16.png";
import { ScreenshotAPI, SettingsAPI } from "../../shared/ipc";
import { isTauri } from "../../shared/platform";
import { extractWorkgroupName, computeTrailingText } from "../../shared/path-extractors";
import { terminalStore } from "../../terminal/stores/terminal";
import type { MainSidebarSide } from "../../shared/types";
import WebServerMenu from "./WebServerMenu";
import ZoomStepper from "./ZoomStepper";
import {
  DEFAULT_MAIN_SIDEBAR_WIDTH,
  MAIN_SIDEBAR_MAX_WIDTH,
  MAIN_SIDEBAR_MIN_WIDTH,
} from "../../shared/sidebar-layout";

declare const __APP_VERSION__: string;
const APP_VERSION = __APP_VERSION__;

const SIDEBAR_WIDTH_PRESETS: Array<{ label: string; width: number }> = [
  { label: "Narrow", width: MAIN_SIDEBAR_MIN_WIDTH },
  { label: "Default", width: DEFAULT_MAIN_SIDEBAR_WIDTH },
  { label: "Wide", width: MAIN_SIDEBAR_MAX_WIDTH },
];

const SIDEBAR_SIDE_PRESETS: Array<{ label: string; side: MainSidebarSide }> = [
  { label: "Left", side: "left" },
  { label: "Right", side: "right" },
];

const formatScreenshotHotkeyForDisplay = (canonicalHotkey: string): string | null => {
  const displayTokens = canonicalHotkey.split("+").map((token) => token.trim());
  if (displayTokens.some((token) => token.length === 0)) return null;

  return displayTokens.join(" + ");
};

// #1398 - the backend now registers the hotkey at the top of setup, so the
// first read almost always lands after registration. This bounded re-read is
// the hardening that keeps a slow boot from hiding the chip for the rest of
// the session; the ceiling exists so a permanently undecided runtime cannot
// poll forever.
const HOTKEY_STATUS_RETRY_DELAY_MS = 250;
const HOTKEY_STATUS_MAX_RETRIES = 20;

const ScreenshotHotkeyStatusChip: Component = () => {
  const [canonicalHotkey, setCanonicalHotkey] = createSignal<string | null>(null);
  const displayHotkey = createMemo(() => {
    const canonical = canonicalHotkey();
    return canonical === null ? null : formatScreenshotHotkeyForDisplay(canonical);
  });

  onMount(() => {
    let disposed = false;
    let retryTimer: ReturnType<typeof setTimeout> | null = null;

    onCleanup(() => {
      disposed = true;
      if (retryTimer !== null) clearTimeout(retryTimer);
    });

    const attempt = (remaining: number) => {
      void ScreenshotAPI.getHotkeyStatus()
        .catch(() => null)
        .then((status) => {
          if (disposed) return;
          if (status !== null && status.registered === true) {
            // Registration is decided; the status never changes again this
            // session. The guard below is the current one-shot predicate,
            // unchanged except that `registered` is already known true here.
            if (
              status.error === null &&
              typeof status.configured === "string" &&
              formatScreenshotHotkeyForDisplay(status.configured) !== null
            ) {
              setCanonicalHotkey(status.configured);
            }
            return;
          }
          // `registered: false` with `error: null` is the pre-registration
          // default, the only genuinely transient state. A non-null error is a
          // decided refusal and must never be retried.
          if ((status === null || status.error === null) && remaining > 0) {
            retryTimer = setTimeout(
              () => attempt(remaining - 1),
              HOTKEY_STATUS_RETRY_DELAY_MS,
            );
          }
        });
    };

    attempt(HOTKEY_STATUS_MAX_RETRIES);
  });

  return (
    <Show when={displayHotkey()}>
      {(hotkey) => (
        <span
          class="screenshot-hotkey-status"
          data-ac-testid="screenshot-hotkey-status"
          title={`Screenshot capture shortcut: ${hotkey()}`}
          aria-label={`Screenshot capture shortcut: ${hotkey()}`}
        >
          <span class="screenshot-hotkey-status-icon" aria-hidden="true">&#x1F4F7;</span>
          <span class="screenshot-hotkey-status-text">{hotkey()}</span>
        </span>
      )}
    </Show>
  );
};

const Titlebar: Component = () => {
  const [layoutOpen, setLayoutOpen] = createSignal(false);
  const [webServerOpen, setWebServerOpen] = createSignal(false);
  const [instanceLabel, setInstanceLabel] = createSignal("");
  const [currentSide, setCurrentSide] = createSignal<MainSidebarSide>("right");
  const wgName = createMemo(() => extractWorkgroupName(terminalStore.activeWorkingDirectory));
  const trailingText = createMemo(() =>
    computeTrailingText(terminalStore.activeWorkingDirectory, terminalStore.activeSessionName),
  );

  const setLayoutMenuOpen = (nextOpen: boolean) => {
    setLayoutOpen(nextOpen);
    if (nextOpen) setWebServerOpen(false);
  };

  const setWebServerMenuOpen = (nextOpen: boolean) => {
    setWebServerOpen(nextOpen);
    if (nextOpen) setLayoutOpen(false);
  };

  const handleMinimize = async () => {
    if (!isTauri) return;
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    getCurrentWindow().minimize();
  };
  const handleMaximize = async () => {
    if (!isTauri) return;
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    const win = getCurrentWindow();
    if (await win.isMaximized()) {
      win.unmaximize();
    } else {
      win.maximize();
    }
  };
  const handleClose = async () => {
    if (!isTauri) return;
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    getCurrentWindow().close();
  };

  const applyWidthPreset = async (width: number) => {
    setLayoutMenuOpen(false);
    window.dispatchEvent(new CustomEvent("main-sidebar-width-change", { detail: { width } }));
    try {
      const settings = await SettingsAPI.get();
      await SettingsAPI.update({ ...settings, mainSidebarWidth: width });
    } catch (err) {
      console.error("applyWidthPreset failed:", err);
    }
  };

  const applySidePreset = async (side: MainSidebarSide) => {
    setLayoutMenuOpen(false);
    setCurrentSide(side);
    window.dispatchEvent(new CustomEvent("main-sidebar-side-change", { detail: { side } }));
    try {
      const settings = await SettingsAPI.get();
      await SettingsAPI.update({ ...settings, mainSidebarSide: side });
    } catch (err) {
      console.error("applySidePreset failed:", err);
    }
  };

  const handleClickOutside = (e: MouseEvent) => {
    const target = e.target as HTMLElement;
    if (layoutOpen() && !target.closest(".layout-dropdown-wrapper")) {
      setLayoutOpen(false);
    }
    if (webServerOpen() && !target.closest(".webserver-menu-wrapper")) {
      setWebServerOpen(false);
    }
  };

  onMount(async () => {
    document.addEventListener("click", handleClickOutside);
    onCleanup(() => document.removeEventListener("click", handleClickOutside));
    try {
      const settings = await SettingsAPI.get();
      setCurrentSide(settings.mainSidebarSide === "left" ? "left" : "right");
    } catch { /* keep default */ }
    if (isTauri) {
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        const label = await invoke<string>("get_instance_label");
        if (label) setInstanceLabel(label);
      } catch { /* non-Tauri or command unavailable */ }
    }
  });

  return (
    <div class="titlebar" data-tauri-drag-region>
      <div class="titlebar-brand" data-tauri-drag-region>
        <img src={iconUrl} class="titlebar-icon" alt="" draggable={false} />
        <span class="titlebar-title" data-tauri-drag-region>
          agents commander
        </span>
        <span class="titlebar-version" data-tauri-drag-region>
          v{APP_VERSION}
        </span>
        {import.meta.env.DEV && (
          <span class="titlebar-dev-badge" data-tauri-drag-region>DEV</span>
        )}
        {instanceLabel() && (
          <span class="titlebar-stage-badge" data-tauri-drag-region>{instanceLabel()}</span>
        )}
        <Show when={wgName()}>
          <span class="titlebar-wg-badge" data-tauri-drag-region>{wgName()}</span>
        </Show>
        <Show when={trailingText()} fallback={<span class="titlebar-session-name">Terminal</span>}>
          <span class="titlebar-session-name">{trailingText()}</span>
        </Show>
      </div>
      <div class="titlebar-controls">
        <Show when={isTauri}>
          <>
            <ScreenshotHotkeyStatusChip />
            <WebServerMenu
              open={webServerOpen()}
              onOpenChange={setWebServerMenuOpen}
            />
          </>
        </Show>
        <ZoomStepper />
        <div class="layout-dropdown-wrapper">
          <button
            class={`titlebar-btn titlebar-btn-layout ${layoutOpen() ? "open" : ""}`}
            onClick={(e) => { e.stopPropagation(); setLayoutMenuOpen(!layoutOpen()); }}
            title="Sidebar layout"
            data-ac-testid="titlebar.layout.button"
          >
            &#x2637;
          </button>
          {layoutOpen() && (
            <div class="layout-dropdown" data-ac-testid="titlebar.layout.menu">
              <div class="layout-section-label">Side</div>
              <div class="layout-segmented" role="group" aria-label="Sidebar side">
                <For each={SIDEBAR_SIDE_PRESETS}>
                  {(preset) => (
                    <button
                      class={`layout-segment ${currentSide() === preset.side ? "active" : ""}`}
                      onClick={() => applySidePreset(preset.side)}
                      aria-pressed={currentSide() === preset.side}
                    >
                      {preset.label}
                    </button>
                  )}
                </For>
              </div>
              <div class="layout-section-label">Width</div>
              <For each={SIDEBAR_WIDTH_PRESETS}>
                {(preset) => (
                  <button
                    class="layout-option"
                    onClick={() => applyWidthPreset(preset.width)}
                  >
                    <span class="layout-option-icon">&#x2630;</span>
                    {preset.label} — {preset.width}px
                  </button>
                )}
              </For>
            </div>
          )}
        </div>
        <Show when={isTauri}>
          <button class="titlebar-btn" onClick={handleMinimize} title="Minimize">
            &#x2014;
          </button>
          <button class="titlebar-btn" onClick={handleMaximize} title="Maximize">
            &#x25A1;
          </button>
          <button
            class="titlebar-btn titlebar-btn-close"
            onClick={handleClose}
            title="Close"
          >
            &#x2715;
          </button>
        </Show>
      </div>
    </div>
  );
};

export default Titlebar;
