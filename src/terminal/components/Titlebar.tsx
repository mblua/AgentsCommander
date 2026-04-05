import { Component, Show, createSignal, onMount } from "solid-js";
import { terminalStore } from "../stores/terminal";
import iconUrl from "../../assets/icon-16.png";
import { isTauri } from "../../shared/platform";
declare const __APP_VERSION__: string;
const APP_VERSION = __APP_VERSION__;

const Titlebar: Component<{ detached?: boolean }> = (props) => {
  const [instanceLabel, setInstanceLabel] = createSignal("");

  onMount(async () => {
    if (isTauri) {
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        const label = await invoke<string>("get_instance_label");
        if (label) setInstanceLabel(label);
      } catch { /* non-Tauri or command unavailable */ }
    }
  });

  const handleShowSidebar = async () => {
    if (!isTauri) return;
    try {
      const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
      const sidebar = await WebviewWindow.getByLabel("sidebar");
      if (sidebar) {
        // unminimize is best-effort — setFocus must always run
        await sidebar.unminimize().catch(() => {});
        await sidebar.show().catch(() => {});
        await sidebar.setFocus();
      }
    } catch (err) {
      console.error("handleShowSidebar failed:", err);
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
        <Show
          when={terminalStore.activeSessionName}
          fallback={<span>Terminal</span>}
        >
          <span class="titlebar-session-name">
            {terminalStore.activeSessionName}
          </span>
          <Show when={terminalStore.activeShell}>
            <span> ({terminalStore.activeShell})</span>
          </Show>
        </Show>
      </div>
      <Show when={isTauri}>
        <div class="titlebar-controls">
          <Show when={!props.detached}>
            <button class="titlebar-btn" onClick={handleShowSidebar} title="Show Sidebar">
              &#x2630;
            </button>
          </Show>
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
        </div>
      </Show>
    </div>
  );
};

export default Titlebar;
