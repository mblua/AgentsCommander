import { Component, createSignal, onMount, onCleanup } from "solid-js";
import { getCurrentWindow } from "@tauri-apps/api/window";
import iconUrl from "../../assets/icon-16.png";
import { getConsoleText } from "../../shared/console-capture";
import { DebugAPI } from "../../shared/ipc";
import { applyWindowLayout } from "../../shared/window-layout";

declare const __APP_VERSION__: string;
const APP_VERSION = __APP_VERSION__;

const Titlebar: Component = () => {
  const [copied, setCopied] = createSignal(false);
  const [layoutOpen, setLayoutOpen] = createSignal(false);

  const handleMinimize = () => getCurrentWindow().minimize();
  const handleClose = () => getCurrentWindow().close();

  const handleCopyLogs = async (e: MouseEvent) => {
    e.stopPropagation();
    const text = getConsoleText();
    await DebugAPI.saveLogs(text);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  const handleLayout = async (side: "left" | "right") => {
    setLayoutOpen(false);
    try {
      await applyWindowLayout(side);
    } catch (err) {
      console.error("applyLayout failed:", err);
    }
  };

  const handleClickOutside = (e: MouseEvent) => {
    if (layoutOpen() && !(e.target as HTMLElement).closest(".layout-dropdown-wrapper")) {
      setLayoutOpen(false);
    }
  };

  onMount(() => {
    document.addEventListener("click", handleClickOutside);
    onCleanup(() => document.removeEventListener("click", handleClickOutside));
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
      </div>
      <div class="titlebar-controls">
        <div class="layout-dropdown-wrapper">
          <button
            class={`titlebar-btn titlebar-btn-layout ${layoutOpen() ? "open" : ""}`}
            onClick={(e) => { e.stopPropagation(); setLayoutOpen(!layoutOpen()); }}
            title="Layout"
          >
            &#x2637;
          </button>
          {layoutOpen() && (
            <div class="layout-dropdown">
              <button class="layout-option" onClick={() => handleLayout("right")}>
                <span class="layout-option-icon">&#x25E8;</span>
                Sidebar Right
              </button>
              <button class="layout-option" onClick={() => handleLayout("left")}>
                <span class="layout-option-icon">&#x25E7;</span>
                Sidebar Left
              </button>
            </div>
          )}
        </div>
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
