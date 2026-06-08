import { Component, createSignal, onMount, onCleanup, Show } from "solid-js";
import type { UnlistenFn } from "../shared/transport";
import type { MainSidebarSide } from "../shared/types";
import { SettingsAPI } from "../shared/ipc";
import { isTauri } from "../shared/platform";
import { initZoom } from "../shared/zoom";
import { initWindowGeometry } from "../shared/window-geometry";
import SidebarApp from "../sidebar/App";
import TerminalApp from "../terminal/App";
import Titlebar from "../sidebar/components/Titlebar";
import QuitConfirmModal from "./components/QuitConfirmModal";
import RtkBanner from "./components/RtkBanner";
import ErrorModal from "./components/ErrorModal";
import { wireHomeListeners } from "./listeners-home";
import {
  DEFAULT_MAIN_SIDEBAR_WIDTH,
  MAIN_SIDEBAR_MAX_WIDTH,
  MAIN_SIDEBAR_MIN_WIDTH,
  MAIN_TERMINAL_MIN_WIDTH,
  clampMainSidebarWidth,
} from "../shared/sidebar-layout";
import "./styles/main.css";

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

  // Keyboard resize for a11y (plan §DW.10). ← / → adjust ±10px,
  // Shift+← / Shift+→ ±40px, Home/End snap to clamp bounds.
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

  // Stateless detached-window count (plan §A3B.3 / G3-B1 — must NOT read
  // sessionsStore because the store is Phase-2 and not authoritative anyway).
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

  // Re-clamp splitter width when the OS resizes the window (e.g. monitor
  // disconnect, Win+Arrow snap). Without this the saved width can exceed
  // windowWidth - 300 and the terminal pane collapses (R2.5).
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

  onMount(async () => {
    // #289 — optimistic light-mode first paint (legacy default); the
    // SettingsAPI.get() block below overrides to dark for users who chose it.
    // The embedded SidebarApp + TerminalApp each run their own corrective
    // step on the same documentElement — idempotent overlap is fine.
    document.documentElement.classList.add("light-theme");

    // Main window owns zoom + geometry persistence. Embedded Sidebar+Terminal
    // skip these initializers per DW.2.
    cleanupZoom = await initZoom("main");
    cleanupGeometry = await initWindowGeometry("main");

    // Load splitter width + always-on-top from settings.
    try {
      const settings = await SettingsAPI.get();
      if (!settings.themeLight) {
        document.documentElement.classList.remove("light-theme");
      }
      const saved = settings.mainSidebarWidth ?? DEFAULT_MAIN_SIDEBAR_WIDTH;
      setSidebarWidth(clampMainSidebarWidth(saved, window.innerWidth));
      setSidebarSide(settings.mainSidebarSide === "left" ? "left" : DEFAULT_SIDEBAR_SIDE);
      if (isTauri && settings.mainAlwaysOnTop) {
        const { getCurrentWindow } = await import("@tauri-apps/api/window");
        await getCurrentWindow().setAlwaysOnTop(true);
      }
    } catch (e) {
      console.error("Failed to load main-window settings:", e);
    }

    // Home auto-visibility contract (issue #183 + #164). Wired in a
    // dedicated helper so the gating logic — especially the userInitiated
    // discriminator on session_switched — is unit-testable in isolation.
    unlisteners.push(...(await wireHomeListeners()));

    window.addEventListener("resize", onWindowResize);
    window.addEventListener("main-sidebar-width-change", onSidebarWidthChange);
    window.addEventListener("main-sidebar-side-change", onSidebarSideChange);

    // Quit-confirmation guard (plan §A3B.3 / G.13 / G3-M1).
    // - If 0 detached windows → let the close proceed (Tauri exits normally).
    // - If ≥1 detached → preventDefault, open custom modal.
    // Re-entry guard covers double-X / Alt+F4 while modal is already open.
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
    >
      <Titlebar />
      <RtkBanner />
      <div class="main-body">
        <div class="main-sidebar-pane" style={{ width: `${sidebarWidth()}px` }}>
          <SidebarApp embedded />
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
        />
        <div class="main-terminal-pane">
          <TerminalApp embedded />
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
    </div>
  );
};

export default MainApp;
