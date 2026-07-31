import { Component } from "solid-js";

/**
 * #1171 - the window is `decorations(false)`, so the frame is HTML.
 *
 * The Tauri window API is imported inside each handler rather than at module scope,
 * following the Resource Monitor (`resource-monitor/App.tsx:145`, `:155`) rather than
 * `SpecBoardTitlebar`, which calls `getCurrentWindow()` while the component is being
 * constructed. Deferring it means the window still renders where that API does not exist,
 * which is what makes this testable outside Tauri.
 *
 * The controls stop propagation so a click on them is not also read as a window drag.
 */
const WatchersTitlebar: Component = () => {
  const withWindow = async (action: "minimize" | "toggleMaximize" | "close") => {
    try {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      await getCurrentWindow()[action]();
    } catch (err) {
      console.error(`[watchers] window ${action} failed:`, err);
    }
  };

  return (
    <div data-tauri-drag-region class="watchers-titlebar">
      <div data-tauri-drag-region class="watchers-titlebar-title">
        Watcher Activity
      </div>
      <div class="watchers-titlebar-controls">
        <button
          onClick={(e) => {
            e.stopPropagation();
            void withWindow("minimize");
          }}
          title="Minimize"
          data-ac-testid="watchers.titlebar.minimize"
          data-ac-role="button"
        >
          &#x2014;
        </button>
        <button
          onClick={(e) => {
            e.stopPropagation();
            void withWindow("toggleMaximize");
          }}
          title="Maximize"
          data-ac-testid="watchers.titlebar.maximize"
          data-ac-role="button"
        >
          &#x25A1;
        </button>
        <button
          onClick={(e) => {
            e.stopPropagation();
            void withWindow("close");
          }}
          title="Close"
          data-ac-testid="watchers.titlebar.close"
          data-ac-role="button"
        >
          &#x2715;
        </button>
      </div>
    </div>
  );
};

export default WatchersTitlebar;
