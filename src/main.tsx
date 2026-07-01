import "./shared/console-capture";
import { render } from "solid-js/web";
import { isTauri } from "./shared/platform";
import TerminalApp from "./terminal/App";
import GuideApp from "./guide/App";
import BrowserApp from "./browser/App";
import SpecBoardApp from "./spec-board/App";
import ResourceMonitorApp from "./resource-monitor/App";
import ScreenshotOverlayApp from "./screenshot-overlay/App";
import MainApp from "./main/App";
import { initAutomationBridge } from "./shared/automation-bridge";
import { initLogLevelForWindow } from "./shared/log-level";

const params = new URLSearchParams(window.location.search);
const windowType = params.get("window");

// Capture remote token from URL for WebSocket auth (browser mode)
const remoteToken = params.get("remoteToken");
if (remoteToken) {
  sessionStorage.setItem("remoteToken", remoteToken);
}

const root = document.getElementById("root");
if (!root) throw new Error("Root element not found");

if (isTauri) {
  void initAutomationBridge();
}

// #612 Install the console log-level gate for THIS window (all 6 window roots
// route through here). Ungated on purpose: the function's internal try/catch
// falls back to a static Info level in non-Tauri/browser contexts where the
// event bus is absent. Runs after the `console-capture` side-effect import (the
// first import above) has installed the console monkey-patch this gate flips.
void initLogLevelForWindow();

// Browser mode (no Tauri): BrowserApp regardless of ?window param.
// Remote web clients load ?window=main but still need the split-browser UX.
const isLegacyDetached =
  windowType === "terminal" && params.get("detached") === "true";

if (!isTauri) {
  render(() => <BrowserApp />, root);
} else if (windowType === "detached" || isLegacyDetached) {
  // New URL: ?window=detached&sessionId=<id>
  // Legacy URL (pre-0.8 backend): ?window=terminal&sessionId=<id>&detached=true
  // Kept for one version so an in-flight detach survives a mid-upgrade.
  const lockedSessionId = params.get("sessionId") || undefined;
  render(
    () => <TerminalApp lockedSessionId={lockedSessionId} detached={true} />,
    root
  );
} else if (windowType === "guide") {
  render(() => <GuideApp />, root);
} else if (windowType === "resource-monitor") {
  render(() => <ResourceMonitorApp />, root);
} else if (windowType === "screenshot-overlay") {
  // #714 Mark the document so the overlay's transparent html/body/#root CSS
  // (scoped to this attribute) applies here ONLY, never leaking into the other
  // windows that share this single bundle. Set BEFORE rendering.
  document.documentElement.setAttribute("data-window", "screenshot-overlay");
  render(() => <ScreenshotOverlayApp />, root);
} else if (windowType === "spec-board") {
  render(() => <SpecBoardApp />, root);
} else {
  // "main", legacy "sidebar", legacy non-detached "terminal", or no param →
  // unified MainApp.
  render(() => <MainApp />, root);
}
