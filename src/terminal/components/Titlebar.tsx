import { Component, Show, createSignal, createMemo, onMount } from "solid-js";
import { terminalStore } from "../stores/terminal";
import iconUrl from "../../assets/icon-16.png";
import { isTauri } from "../../shared/platform";
import { InstanceAPI, WindowAPI } from "../../shared/ipc";
import { extractWorkgroupName, computeTrailingText } from "../../shared/path-extractors";
declare const __APP_VERSION__: string;
const APP_VERSION = __APP_VERSION__;

interface TitlebarProps {
  detached?: boolean;
  /** Session id this detached window is locked to. Required for Re-attach button. */
  lockedSessionId?: string;
}

const Titlebar: Component<TitlebarProps> = (props) => {
  const [instanceLabel, setInstanceLabel] = createSignal("");
  const wgName = createMemo(() => extractWorkgroupName(terminalStore.activeWorkingDirectory));
  const trailingText = createMemo(() =>
    computeTrailingText(terminalStore.activeWorkingDirectory, terminalStore.activeSessionName),
  );

  onMount(async () => {
    if (isTauri) {
      try {
        const label = await InstanceAPI.getLabel();
        if (label) setInstanceLabel(label);
      } catch { /* non-Tauri or command unavailable */ }
    }
  });

  const handleReattach = async () => {
    if (!props.lockedSessionId) return;
    try {
      await WindowAPI.attach(props.lockedSessionId);
    } catch (err) {
      console.error("Re-attach failed:", err);
    }
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
        <Show when={props.detached}>
          <span class="titlebar-detached-badge">DETACHED</span>
        </Show>
        <Show when={wgName()}>
          <span class="titlebar-wg-badge" data-tauri-drag-region>{wgName()}</span>
        </Show>
        <Show when={trailingText()} fallback={<span class="titlebar-session-name">Terminal</span>}>
          <span class="titlebar-session-name">{trailingText()}</span>
        </Show>
      </div>
      <Show when={isTauri}>
        <div class="titlebar-controls">
          <Show when={props.detached && props.lockedSessionId}>
            <button
              class="titlebar-btn titlebar-btn-attach"
              onClick={handleReattach}
              title="Re-attach session"
              data-ac-testid="terminal.titlebar.reattach"
              data-ac-role="button"
            >
              &#x2934;
            </button>
          </Show>
          <button
            class="titlebar-btn"
            onClick={handleMinimize}
            title="Minimize"
            data-ac-testid="terminal.titlebar.minimize"
            data-ac-role="button"
          >
            &#x2014;
          </button>
          <button
            class="titlebar-btn"
            onClick={handleMaximize}
            title="Maximize"
            data-ac-testid="terminal.titlebar.maximize"
            data-ac-role="button"
          >
            &#x25A1;
          </button>
          <button
            class="titlebar-btn titlebar-btn-close"
            onClick={handleClose}
            title="Close"
            data-ac-testid="terminal.titlebar.close"
            data-ac-role="button"
          >
            &#x2715;
          </button>
        </div>
      </Show>
    </div>
  );
};

export default Titlebar;
