import { Component, Show } from "solid-js";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { terminalStore } from "../stores/terminal";
import iconUrl from "../../assets/icon-16.png";
declare const __APP_VERSION__: string;
const APP_VERSION = __APP_VERSION__;

const Titlebar: Component<{ detached?: boolean }> = (props) => {
  const handleMinimize = () => getCurrentWindow().minimize();
  const handleMaximize = async () => {
    const win = getCurrentWindow();
    if (await win.isMaximized()) {
      win.unmaximize();
    } else {
      win.maximize();
    }
  };
  const handleClose = () => getCurrentWindow().close();

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
