import { Component, Show } from "solid-js";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { terminalStore } from "../stores/terminal";

const Titlebar: Component<{ detached?: boolean }> = (props) => {
  const appWindow = getCurrentWindow();

  const handleMinimize = () => appWindow.minimize();
  const handleMaximize = async () => {
    if (await appWindow.isMaximized()) {
      appWindow.unmaximize();
    } else {
      appWindow.maximize();
    }
  };
  const handleClose = () => appWindow.close();

  return (
    <div class="titlebar" data-tauri-drag-region>
      <div class="titlebar-title" data-tauri-drag-region>
        {import.meta.env.DEV && (
          <span class="titlebar-dev-badge" data-tauri-drag-region>DEV</span>
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
      <div class="titlebar-controls">
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
    </div>
  );
};

export default Titlebar;
