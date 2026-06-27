import { Component, createSignal, onCleanup, onMount } from "solid-js";
import SidebarApp from "../sidebar/App";
import TerminalApp from "../terminal/App";
import type { UnlistenFn } from "../shared/transport";
import { SettingsAPI, onThemeChanged } from "../shared/ipc";
import "../sidebar/styles/sidebar.css";
import "../terminal/styles/terminal.css";
import "./styles/browser.css";

/**
 * Combined browser layout: sidebar + terminal with a draggable divider.
 * Used when accessing AgentsCommander via web browser instead of Tauri.
 */
const BrowserApp: Component = () => {
  const [sidebarWidth, setSidebarWidth] = createSignal(300);
  const [dragging, setDragging] = createSignal(false);
  let disposed = false;
  let unlistenThemeChanged: UnlistenFn | null = null;

  const applyTheme = (light: boolean) => {
    document.documentElement.classList.toggle("light-theme", light);
  };

  onMount(async () => {
    // BrowserApp owns the document theme because its children are embedded to
    // suppress titlebars/window geometry. Dark is the CSS base, so first paint
    // is dark with no optimistic class; correct from persisted settings below.
    const unlisten = await onThemeChanged(({ light }) => {
      applyTheme(light);
    });
    if (disposed) {
      unlisten();
      return;
    }
    unlistenThemeChanged = unlisten;

    try {
      const settings = await SettingsAPI.get();
      if (!disposed) {
        applyTheme(settings.themeLight);
      }
    } catch (err) {
      console.error("[browser] failed to load theme setting:", err);
    }
  });

  onCleanup(() => {
    disposed = true;
    unlistenThemeChanged?.();
  });

  const onMouseDown = (e: MouseEvent) => {
    e.preventDefault();
    setDragging(true);

    const onMouseMove = (e: MouseEvent) => {
      const newWidth = Math.max(200, Math.min(600, e.clientX));
      setSidebarWidth(newWidth);
    };

    const onMouseUp = () => {
      setDragging(false);
      document.removeEventListener("mousemove", onMouseMove);
      document.removeEventListener("mouseup", onMouseUp);
    };

    document.addEventListener("mousemove", onMouseMove);
    document.addEventListener("mouseup", onMouseUp);
  };

  // Touch support for mobile
  const onTouchStart = (e: TouchEvent) => {
    e.preventDefault();
    setDragging(true);

    const onTouchMove = (e: TouchEvent) => {
      const touch = e.touches[0];
      const newWidth = Math.max(200, Math.min(600, touch.clientX));
      setSidebarWidth(newWidth);
    };

    const onTouchEnd = () => {
      setDragging(false);
      document.removeEventListener("touchmove", onTouchMove);
      document.removeEventListener("touchend", onTouchEnd);
    };

    document.addEventListener("touchmove", onTouchMove);
    document.addEventListener("touchend", onTouchEnd);
  };

  return (
    <div class="browser-layout" classList={{ "browser-dragging": dragging() }}>
      <div class="browser-sidebar" style={{ width: `${sidebarWidth()}px` }}>
        <SidebarApp embedded />
      </div>
      <div
        class="browser-divider"
        onMouseDown={onMouseDown}
        onTouchStart={onTouchStart}
      >
        <div class="browser-divider-handle" />
      </div>
      <div class="browser-terminal">
        <TerminalApp embedded />
      </div>
    </div>
  );
};

export default BrowserApp;
