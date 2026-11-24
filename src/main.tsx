import "./shared/styles/scrollbars.css";
import "./shared/console-capture";
import { render } from "solid-js/web";
import { WindowErrorBoundary } from "./shared/window-error-boundary";
import { isTauri } from "./shared/platform";
import TerminalApp from "./terminal/App";
import BrowserApp from "./browser/App";
import SpecBoardApp from "./spec-board/App";
import ResourceMonitorApp from "./resource-monitor/App";
import WatchersApp from "./watchers/App";
import ScreenshotOverlayApp from "./screenshot-overlay/App";
import MainApp from "./main/App";
import { initCspProbe } from "./shared/csp-probe";
import { initAutomationBridge } from "./shared/automation-bridge";
import { initLogLevelForWindow } from "./shared/log-level";
import { harvestIpcBlackBox, installIpcBlackBox } from "./shared/ipc-blackbox";
import { BlackBoxAPI } from "./shared/ipc";
import { storeRemoteTokenFromUrl } from "./shared/remote-token";

const params = new URLSearchParams(window.location.search);
const windowType = params.get("window");

storeRemoteTokenFromUrl(params, sessionStorage);

const root = document.getElementById("root");
if (!root) throw new Error("Root element not found");

if (isTauri) {
  void initCspProbe();
  void initAutomationBridge();
}

void initLogLevelForWindow();

// #1652 - installed for EVERY window: which windows kept ticking and which
// stopped is itself evidence, and all windows share one origin's localStorage,
// so each keys its record by its own webview label.
void installIpcBlackBox();

const isLegacyDetached =
  windowType === "terminal" && params.get("detached") === "true";

if (!isTauri) {
  render(
    () => (
      <WindowErrorBoundary label="browser">
        <BrowserApp />
      </WindowErrorBoundary>
    ),
    root
  );
} else if (windowType === "detached" || isLegacyDetached) {
  const lockedSessionId = params.get("sessionId") || undefined;
  render(
    () => (
      <WindowErrorBoundary label="detached">
        <TerminalApp lockedSessionId={lockedSessionId} detached={true} />
      </WindowErrorBoundary>
    ),
    root
  );
} else if (windowType === "resource-monitor") {
  render(
    () => (
      <WindowErrorBoundary label="resource-monitor">
        <ResourceMonitorApp />
      </WindowErrorBoundary>
    ),
    root
  );
} else if (windowType === "watchers") {
  // #1171 - the query parameter is only read here, on the window's first creation. The
  // window is a singleton, so every later open focuses it and re-scopes it through the
  // `watchers_scope_request` event instead.
  render(
    () => (
      <WindowErrorBoundary label="watchers">
        <WatchersApp initialSessionId={params.get("sessionId") || undefined} />
      </WindowErrorBoundary>
    ),
    root
  );
} else if (windowType === "screenshot-overlay") {
  document.documentElement.dataset.window = "screenshot-overlay";
  render(
    () => (
      <WindowErrorBoundary label="screenshot-overlay">
        <ScreenshotOverlayApp />
      </WindowErrorBoundary>
    ),
    root
  );
} else if (windowType === "spec-board") {
  render(
    () => (
      <WindowErrorBoundary label="spec-board">
        <SpecBoardApp />
      </WindowErrorBoundary>
    ),
    root
  );
} else {
  // #1652 - one harvester per app start. The main window always exists, and
  // scanning from exactly one window is what keeps a single previous-run record
  // from being reported by several windows at once. `harvestIpcBlackBox` awaits
  // the install above internally, so this line does not need to.
  void harvestIpcBlackBox(BlackBoxAPI.report);
  render(
    () => (
      <WindowErrorBoundary label="main">
        <MainApp />
      </WindowErrorBoundary>
    ),
    root
  );
}
