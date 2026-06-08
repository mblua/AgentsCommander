import { Component, createSignal, onMount, onCleanup, Show } from "solid-js";
import { initZoom } from "../shared/zoom";
import { isTauri } from "../shared/platform";
import ExternalLinkConfirm from "../shared/components/ExternalLinkConfirm";
import HintsTab from "./components/HintsTab";
import TutorialTab from "./components/TutorialTab";
import iconUrl from "../assets/icon-16.png";
import "./styles/guide.css";
import "../shared/styles/external-link-confirm.css";

type Tab = "hints" | "tutorial";

const tabs: { id: Tab; label: string }[] = [
  { id: "hints", label: "Hints" },
  { id: "tutorial", label: "Tutorial" },
];

const GuideApp: Component = () => {
  const [activeTab, setActiveTab] = createSignal<Tab>("hints");
  let cleanupZoom: (() => void) | null = null;

  const handleMinimize = async () => {
    if (!isTauri) return;
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    getCurrentWindow().minimize();
  };
  const handleClose = async () => {
    if (!isTauri) return;
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    getCurrentWindow().close();
  };

  onMount(async () => {
    cleanupZoom = await initZoom("guide");
  });

  onCleanup(() => {
    if (cleanupZoom) cleanupZoom();
  });

  return (
    <div class="guide-layout">
      <div class="titlebar" data-tauri-drag-region>
        <div class="titlebar-brand" data-tauri-drag-region>
          <img src={iconUrl} class="titlebar-icon" alt="" draggable={false} />
          <span class="titlebar-title" data-tauri-drag-region>guide</span>
        </div>
        <Show when={isTauri}>
          <div class="titlebar-controls">
            <button class="titlebar-btn" onClick={handleMinimize} title="Minimize">
              &#x2014;
            </button>
            <button class="titlebar-btn titlebar-btn-close" onClick={handleClose} title="Close">
              &#x2715;
            </button>
          </div>
        </Show>
      </div>

      <div class="guide-tabs">
        {tabs.map((tab) => (
          <button
            class={`guide-tab ${activeTab() === tab.id ? "active" : ""}`}
            onClick={() => setActiveTab(tab.id)}
          >
            {tab.label}
          </button>
        ))}
      </div>

      <div class="guide-content">
        {activeTab() === "hints" && <HintsTab />}
        {activeTab() === "tutorial" && <TutorialTab />}
      </div>
      <ExternalLinkConfirm />
    </div>
  );
};

export default GuideApp;
