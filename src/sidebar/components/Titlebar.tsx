import { Component } from "solid-js";
import { getCurrentWindow } from "@tauri-apps/api/window";
import iconUrl from "../../assets/icon-16.png";

const APP_VERSION = "0.2.2";

const Titlebar: Component = () => {
  const appWindow = getCurrentWindow();

  const handleMinimize = () => appWindow.minimize();
  const handleClose = () => appWindow.close();

  return (
    <div class="titlebar" data-tauri-drag-region>
      <div class="titlebar-brand" data-tauri-drag-region>
        <img src={iconUrl} class="titlebar-icon" alt="" draggable={false} />
        <span class="titlebar-title" data-tauri-drag-region>
          summongate
        </span>
        <span class="titlebar-version" data-tauri-drag-region>
          v{APP_VERSION}
        </span>
        {import.meta.env.DEV && (
          <span class="titlebar-dev-badge" data-tauri-drag-region>DEV</span>
        )}
      </div>
      <div class="titlebar-controls">
        <button class="titlebar-btn" onClick={handleMinimize} title="Minimize">
          &#x2014;
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
