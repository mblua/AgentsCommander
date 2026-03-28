import { Component, createSignal, onMount, onCleanup, Show } from "solid-js";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { initZoom } from "../shared/zoom";
import { DarkFactoryAPI } from "../shared/ipc";
import type { DarkFactoryConfig } from "../shared/types";
import OrgChart from "./components/OrgChart";
import iconUrl from "../assets/icon-16.png";
import "./styles/darkfactory.css";

const DarkFactoryApp: Component = () => {
  const appWindow = getCurrentWindow();
  const [config, setConfig] = createSignal<DarkFactoryConfig | null>(null);
  const [loading, setLoading] = createSignal(true);
  let cleanupZoom: (() => void) | null = null;

  const handleMinimize = () => appWindow.minimize();
  const handleClose = () => appWindow.close();

  const hasContent = () => {
    const c = config();
    if (!c || c.layers.length === 0) return false;
    return c.teams.some((t) => t.layerId);
  };

  onMount(async () => {
    cleanupZoom = await initZoom("darkfactory");
    try {
      const data = await DarkFactoryAPI.get();
      setConfig(data);
    } catch (e) {
      console.error("Failed to load Dark Factory config:", e);
    } finally {
      setLoading(false);
    }
  });

  onCleanup(() => {
    if (cleanupZoom) cleanupZoom();
  });

  return (
    <div class="df-layout">
      <div class="titlebar" data-tauri-drag-region>
        <div class="titlebar-brand" data-tauri-drag-region>
          <img src={iconUrl} class="titlebar-icon" alt="" draggable={false} />
          <span class="titlebar-title" data-tauri-drag-region>dark factory</span>
        </div>
        <div class="titlebar-controls">
          <button class="titlebar-btn" onClick={handleMinimize} title="Minimize">
            &#x2014;
          </button>
          <button class="titlebar-btn titlebar-btn-close" onClick={handleClose} title="Close">
            &#x2715;
          </button>
        </div>
      </div>

      <div class="df-content">
        <Show when={!loading()} fallback={<div class="df-empty"><div class="df-empty-text">Loading...</div></div>}>
          <Show
            when={hasContent()}
            fallback={
              <div class="df-empty">
                <div class="df-empty-icon">&#x1F3ED;</div>
                <div class="df-empty-title">No layers configured</div>
                <div class="df-empty-text">
                  Go to Settings &gt; Dark Factory to set up your organization layers and assign teams.
                </div>
              </div>
            }
          >
            <OrgChart config={config()!} />
          </Show>
        </Show>
      </div>
    </div>
  );
};

export default DarkFactoryApp;
