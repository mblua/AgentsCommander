import { Component, createSignal, onCleanup, onMount, Show } from "solid-js";
import SidebarApp from "../sidebar/App";
import TerminalApp from "../terminal/App";
import type { UnlistenFn } from "../shared/transport";
import type { MainSidebarSide } from "../shared/types";
import { SettingsAPI, onThemeChanged } from "../shared/ipc";
import "../sidebar/styles/sidebar.css";
import "../terminal/styles/terminal.css";
import "./styles/browser.css";

const MIN_SIDEBAR_WIDTH = 200;
const MAX_SIDEBAR_WIDTH = 600;
// #840 — the web view mirrors the desktop default: sidebar on the right unless
// the persisted `mainSidebarSide` preference says "left".
const DEFAULT_SIDEBAR_SIDE: MainSidebarSide = "right";

/**
 * Combined browser layout: sidebar + terminal with a draggable divider.
 * Used when accessing AgentsCommander via web browser instead of Tauri.
 */
const BrowserApp: Component = () => {
  const [sidebarWidth, setSidebarWidth] = createSignal(300);
  const [sidebarSide, setSidebarSide] = createSignal<MainSidebarSide>(DEFAULT_SIDEBAR_SIDE);
  const [dragging, setDragging] = createSignal(false);
  let disposed = false;
  let unlistenThemeChanged: UnlistenFn | null = null;

  const applyTheme = (light: boolean) => {
    document.documentElement.classList.toggle("light-theme", light);
  };

  const clampWidth = (raw: number) =>
    Math.max(MIN_SIDEBAR_WIDTH, Math.min(MAX_SIDEBAR_WIDTH, raw));

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
        // #840 — honor the shared `mainSidebarSide` preference. Default to the
        // right for any value that isn't an explicit "left".
        setSidebarSide(settings.mainSidebarSide === "left" ? "left" : DEFAULT_SIDEBAR_SIDE);
      }
    } catch (err) {
      console.error("[browser] failed to load settings:", err);
    }
  });

  onCleanup(() => {
    disposed = true;
    unlistenThemeChanged?.();
  });

  // #840 — flip the sidebar side and write it through to the shared
  // `mainSidebarSide` setting (the same preference the desktop Titlebar drives),
  // so web and desktop stay in sync. Optimistic: update the signal first for a
  // snappy swap, then persist; on failure we keep the UI state and just log —
  // this mirrors the desktop side-preset handler, which also does not roll back.
  const toggleSide = async () => {
    const next: MainSidebarSide = sidebarSide() === "right" ? "left" : "right";
    setSidebarSide(next);
    try {
      const settings = await SettingsAPI.get();
      await SettingsAPI.update({ ...settings, mainSidebarSide: next });
    } catch (err) {
      console.error("[browser] failed to persist sidebar side:", err);
    }
  };

  const onMouseDown = (e: MouseEvent) => {
    e.preventDefault();
    setDragging(true);
    // Capture the side at drag start. The divider math mirrors main/App.tsx: a
    // left sidebar grows with clientX; a right sidebar grows as the pointer
    // moves toward it (innerWidth - clientX).
    const side = sidebarSide();

    const onMouseMove = (e: MouseEvent) => {
      const raw = side === "left" ? e.clientX : window.innerWidth - e.clientX;
      setSidebarWidth(clampWidth(raw));
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
    const side = sidebarSide();

    const onTouchMove = (e: TouchEvent) => {
      const touch = e.touches[0];
      const raw = side === "left" ? touch.clientX : window.innerWidth - touch.clientX;
      setSidebarWidth(clampWidth(raw));
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
    <div
      class="browser-layout"
      classList={{
        "browser-dragging": dragging(),
        "browser-sidebar-right": sidebarSide() === "right",
      }}
    >
      <div class="browser-sidebar" style={{ width: `${sidebarWidth()}px` }}>
        <SidebarApp embedded railSide={sidebarSide()} />
      </div>
      <div
        class="browser-divider"
        onMouseDown={onMouseDown}
        onTouchStart={onTouchStart}
      >
        <div class="browser-divider-handle" />
      </div>
      <div class="browser-terminal">
        {/* #840 — the web view has no desktop titlebar to host the Left/Right
            preset, so expose a compact toggle here. Anchored to the terminal
            pane's top-right corner because the sidebar's ActionBar spans its
            full top edge — floating over the sidebar would cover those buttons. */}
        <button
          type="button"
          class="browser-side-toggle"
          title={`Sidebar on the ${sidebarSide()} — move it to the ${sidebarSide() === "right" ? "left" : "right"}`}
          aria-label={`Move sidebar to the ${sidebarSide() === "right" ? "left" : "right"}`}
          data-ac-testid="browser.sidebarSideToggle"
          onClick={toggleSide}
        >
          <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true">
            <rect
              x="1.5"
              y="2.5"
              width="13"
              height="11"
              rx="2"
              fill="none"
              stroke="currentColor"
              stroke-width="1.3"
            />
            <Show
              when={sidebarSide() === "right"}
              fallback={<rect x="2.4" y="3.4" width="4.2" height="9.2" rx="1" fill="currentColor" />}
            >
              <rect x="9.4" y="3.4" width="4.2" height="9.2" rx="1" fill="currentColor" />
            </Show>
          </svg>
        </button>
        <TerminalApp embedded />
      </div>
    </div>
  );
};

export default BrowserApp;
