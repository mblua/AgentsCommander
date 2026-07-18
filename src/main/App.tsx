import { Component, createSignal, onMount, onCleanup, Show } from "solid-js";
import type { UnlistenFn } from "../shared/transport";
import type { MainSidebarSide } from "../shared/types";
import { SettingsAPI } from "../shared/ipc";
import { isTauri } from "../shared/platform";
import { initZoom } from "../shared/zoom";
import { initWindowGeometry } from "../shared/window-geometry";
import { startNonStopWatchdogClient } from "../sidebar/watchdog/non-stop-watchdog-client";
import SidebarApp from "../sidebar/App";
import TerminalApp from "../terminal/App";
import ResourceMonitorApp from "../resource-monitor/App";
import { centralViewStore } from "./stores/centralView";
import { wireCentralViewListeners } from "./listeners-central-view";
import Titlebar from "../sidebar/components/Titlebar";
import QuitConfirmModal from "./components/QuitConfirmModal";
import ErrorModal from "./components/ErrorModal";
import ExternalLinkConfirm from "../shared/components/ExternalLinkConfirm";
import { wireHomeListeners } from "./listeners-home";
import {
  DEFAULT_MAIN_SIDEBAR_WIDTH,
  MAIN_SIDEBAR_MAX_WIDTH,
  MAIN_SIDEBAR_MIN_WIDTH,
  MAIN_TERMINAL_MIN_WIDTH,
  clampMainSidebarWidth,
} from "../shared/sidebar-layout";
import "./styles/main.css";
import "../shared/styles/external-link-confirm.css";

const DEFAULT_SIDEBAR_SIDE: MainSidebarSide = "right";

const MainApp: Component = () => {
  const [sidebarWidth, setSidebarWidth] = createSignal(DEFAULT_MAIN_SIDEBAR_WIDTH);
  const [sidebarSide, setSidebarSide] = createSignal<MainSidebarSide>(DEFAULT_SIDEBAR_SIDE);
  const [dragging, setDragging] = createSignal(false);
  const [quitModalCount, setQuitModalCount] = createSignal<number | null>(null);

  const unlisteners: UnlistenFn[] = [];
  let cleanupZoom: (() => void) | null = null;
  let cleanupGeometry: (() => void) | null = null;
  let quitInProgress = false;
  let splitterSaveTimeout: ReturnType<typeof setTimeout> | null = null;

  const persistWidth = (w: number) => {
    if (splitterSaveTimeout) clearTimeout(splitterSaveTimeout);
    splitterSaveTimeout = setTimeout(async () => {
      try {
        const settings = await SettingsAPI.get();
        await SettingsAPI.update({ ...settings, mainSidebarWidth: w });
      } catch (e) {
        console.error("Failed to persist splitter width:", e);
      }
    }, 500);
  };

  const onPointerDown = (e: PointerEvent) => {
    e.preventDefault();
    const divider = e.currentTarget as HTMLElement;
    const sideAtDragStart = sidebarSide();
    try { divider.setPointerCapture(e.pointerId); } catch { /* some targets refuse capture */ }
    document.body.style.cursor = "col-resize";
    setDragging(true);

    const onMove = (m: PointerEvent) => {
      const rawWidth = sideAtDragStart === "left"
        ? m.clientX
        : window.innerWidth - m.clientX;
      setSidebarWidth(clampMainSidebarWidth(rawWidth, window.innerWidth));
    };
    const onUp = (u: PointerEvent) => {
      try { divider.releasePointerCapture(u.pointerId); } catch { /* already released */ }
      document.body.style.cursor = "";
      setDragging(false);
      divider.removeEventListener("pointermove", onMove);
      divider.removeEventListener("pointerup", onUp);
      divider.removeEventListener("pointercancel", onUp);
      persistWidth(sidebarWidth());
    };
    divider.addEventListener("pointermove", onMove);
    divider.addEventListener("pointerup", onUp);
    divider.addEventListener("pointercancel", onUp);
  };

  const onDividerKeyDown = (e: KeyboardEvent) => {
    const step = e.shiftKey ? 40 : 10;
    let next: number | null = null;
    if (e.key === "ArrowLeft") next = sidebarWidth() + (sidebarSide() === "right" ? step : -step);
    else if (e.key === "ArrowRight") next = sidebarWidth() + (sidebarSide() === "right" ? -step : step);
    else if (e.key === "Home") next = MAIN_SIDEBAR_MIN_WIDTH;
    else if (e.key === "End") next = Math.min(MAIN_SIDEBAR_MAX_WIDTH, window.innerWidth - MAIN_TERMINAL_MIN_WIDTH);
    if (next === null) return;
    e.preventDefault();
    const clamped = clampMainSidebarWidth(next, window.innerWidth);
    setSidebarWidth(clamped);
    persistWidth(clamped);
  };

  async function countDetachedWindows(): Promise<number> {
    if (!isTauri) return 0;
    const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
    const all = await WebviewWindow.getAll();
    return all.filter((w) => w.label.startsWith("terminal-")).length;
  }

  const onModalCancel = () => setQuitModalCount(null);

  const onModalQuit = async () => {
    quitInProgress = true;
    try {
      const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      for (const w of await WebviewWindow.getAll()) {
        if (w.label.startsWith("terminal-")) {
          try { await w.destroy(); }
          catch (err) { console.warn("[quit] destroy of", w.label, "failed:", err); }
        }
      }
      try { await getCurrentWindow().destroy(); }
      catch (err) { console.warn("[quit] destroy of main failed:", err); }
    } finally {
      quitInProgress = false;
      setQuitModalCount(null);
    }
  };

  const onWindowResize = () => {
    setSidebarWidth((w) => clampMainSidebarWidth(w, window.innerWidth));
  };

  const onSidebarWidthChange = (event: Event) => {
    const width = (event as CustomEvent<{ width?: number }>).detail?.width;
    if (typeof width === "number") {
      setSidebarWidth(clampMainSidebarWidth(width, window.innerWidth));
    }
  };

  const onSidebarSideChange = (event: Event) => {
    const side = (event as CustomEvent<{ side?: MainSidebarSide }>).detail?.side;
    if (side === "left" || side === "right") {
      setSidebarSide(side);
    }
  };

  startNonStopWatchdogClient();

  onMount(async () => {

    cleanupZoom = await initZoom("main");
    cleanupGeometry = await initWindowGeometry("main");

    try {
      const settings = await SettingsAPI.get();
      document.documentElement.classList.toggle("light-theme", settings.themeLight);
      const saved = settings.mainSidebarWidth ?? DEFAULT_MAIN_SIDEBAR_WIDTH;
      setSidebarWidth(clampMainSidebarWidth(saved, window.innerWidth));
      setSidebarSide(settings.mainSidebarSide === "left" ? "left" : DEFAULT_SIDEBAR_SIDE);
      centralViewStore.setInitialView(
        settings.mainResourceMonitorAttached ? "resourceMonitor" : "terminal"
      );
      if (isTauri && settings.mainAlwaysOnTop) {
        const { getCurrentWindow } = await import("@tauri-apps/api/window");
        await getCurrentWindow().setAlwaysOnTop(true);
      }
    } catch (e) {
      console.error("Failed to load main-window settings:", e);
    }

    unlisteners.push(...(await wireHomeListeners()));

    unlisteners.push(...(await wireCentralViewListeners()));

    window.addEventListener("resize", onWindowResize);
    window.addEventListener("main-sidebar-width-change", onSidebarWidthChange);
    window.addEventListener("main-sidebar-side-change", onSidebarSideChange);

    if (isTauri) {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      const win = getCurrentWindow();
      const unlistenClose = await win.onCloseRequested(async (e) => {
        if (quitInProgress || quitModalCount() !== null) {
          e.preventDefault();
          return;
        }
        const count = await countDetachedWindows();
        if (count === 0) return; // silent quit path
        e.preventDefault();
        setQuitModalCount(count);
      });
      unlisteners.push(unlistenClose);
    }
  });

  onCleanup(() => {
    unlisteners.forEach((u) => u());
    if (cleanupZoom) cleanupZoom();
    if (cleanupGeometry) cleanupGeometry();
    if (splitterSaveTimeout) clearTimeout(splitterSaveTimeout);
    window.removeEventListener("resize", onWindowResize);
    window.removeEventListener("main-sidebar-width-change", onSidebarWidthChange);
    window.removeEventListener("main-sidebar-side-change", onSidebarSideChange);
  });

  return (
    <div
      class="main-root"
      classList={{
        "main-dragging": dragging(),
        "main-sidebar-right": sidebarSide() === "right",
      }}
      data-ac-testid="main.root"
      data-ac-role="surface"
      data-ac-state={dragging() ? "dragging" : "idle"}
    >
      <Titlebar />
      <div class="main-body">
        <div class="main-sidebar-pane" style={{ width: `${sidebarWidth()}px` }}>
          <SidebarApp embedded railSide={sidebarSide()} />
        </div>
        <div
          class="main-divider"
          classList={{ dragging: dragging() }}
          onPointerDown={onPointerDown}
          onKeyDown={onDividerKeyDown}
          role="separator"
          aria-orientation="vertical"
          aria-label={`Resize ${sidebarSide()} sidebar`}
          aria-valuenow={Math.round(sidebarWidth())}
          aria-valuetext={`${Math.round(sidebarWidth())} pixels, sidebar on ${sidebarSide()}`}
          aria-valuemin={MAIN_SIDEBAR_MIN_WIDTH}
          aria-valuemax={MAIN_SIDEBAR_MAX_WIDTH}
          tabindex="0"
          data-ac-testid="main.splitter"
          data-ac-role="separator"
          data-ac-state={dragging() ? "dragging" : "idle"}
        />
        <div class="main-terminal-pane">
          <TerminalApp embedded />
          <Show when={centralViewStore.isResourceMonitor}>
            <div
              class="main-rm-pane"
              data-ac-testid="main.resourceMonitorPane"
              data-ac-role="surface"
            >
              <ResourceMonitorApp embedded />
            </div>
          </Show>
        </div>
      </div>
      <Show when={quitModalCount() !== null}>
        <QuitConfirmModal
          detachedCount={quitModalCount()!}
          onCancel={onModalCancel}
          onQuit={onModalQuit}
        />
      </Show>
      <ErrorModal />
      <ExternalLinkConfirm />
    </div>
  );
};

export default MainApp;
