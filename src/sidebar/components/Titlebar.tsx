import { Component, createSignal } from "solid-js";
import { getCurrentWindow } from "@tauri-apps/api/window";
import iconUrl from "../../assets/icon-16.png";
import { getConsoleText } from "../../shared/console-capture";
import { DebugAPI } from "../../shared/ipc";

declare const __APP_VERSION__: string;
const APP_VERSION = __APP_VERSION__;

const Titlebar: Component = () => {
  const [copied, setCopied] = createSignal(false);

  const handleMinimize = () => getCurrentWindow().minimize();
  const handleClose = () => getCurrentWindow().close();

  const handleCopyLogs = async (e: MouseEvent) => {
    e.stopPropagation();
    const text = getConsoleText();
    await DebugAPI.saveLogs(text);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  return (
    <div class="titlebar" data-tauri-drag-region>
      <div class="titlebar-brand" data-tauri-drag-region>
        <img src={iconUrl} class="titlebar-icon" alt="" draggable={false} />
        <span class="titlebar-title" data-tauri-drag-region>
          agents commander
        </span>
        {import.meta.env.DEV && (
          <span class="titlebar-dev-badge" data-tauri-drag-region>DEV {APP_VERSION}</span>
        )}
      </div>
      <div class="titlebar-controls">
        {import.meta.env.DEV && (
          <button
            class={`titlebar-btn titlebar-btn-logs ${copied() ? "copied" : ""}`}
            onClick={handleCopyLogs}
            title={copied() ? "Copied!" : "Copy console logs to clipboard"}
          >
            {copied() ? "\u2713" : "\u2261"}
          </button>
        )}
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
