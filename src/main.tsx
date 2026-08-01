import "./shared/console-capture";
import { render } from "solid-js/web";
import { isTauri } from "./shared/platform";
import TerminalApp from "./terminal/App";
import GuideApp from "./guide/App";
import BrowserApp from "./browser/App";
import SpecBoardApp from "./spec-board/App";
import ResourceMonitorApp from "./resource-monitor/App";
import WatchersApp from "./watchers/App";
import ScreenshotOverlayApp from "./screenshot-overlay/App";
import MainApp from "./main/App";
import { initAutomationBridge } from "./shared/automation-bridge";
import { initLogLevelForWindow } from "./shared/log-level";

const params = new URLSearchParams(window.location.search);
const windowType = params.get("window");

const remoteToken = params.get("remoteToken");
if (remoteToken) {
  sessionStorage.setItem("remoteToken", remoteToken);
}

const root = document.getElementById("root");
if (!root) throw new Error("Root element not found");

if (isTauri) {
  void initAutomationBridge();
}

void initLogLevelForWindow();

const isLegacyDetached =
  windowType === "terminal" && params.get("detached") === "true";

if (!isTauri) {
  render(() => <BrowserApp />, root);
} else if (windowType === "detached" || isLegacyDetached) {
  const lockedSessionId = params.get("sessionId") || undefined;
  render(
    () => <TerminalApp lockedSessionId={lockedSessionId} detached={true} />,
    root
  );
} else if (windowType === "guide") {
  render(() => <GuideApp />, root);
} else if (windowType === "resource-monitor") {
  render(() => <ResourceMonitorApp />, root);
} else if (windowType === "watchers") {
  // #1171 - the query parameter is only read here, on the window's first creation. The
  // window is a singleton, so every later open focuses it and re-scopes it through the
  // `watchers_scope_request` event instead.
  render(() => <WatchersApp initialSessionId={params.get("sessionId") || undefined} />, root);
} else if (windowType === "screenshot-overlay") {
  document.documentElement.setAttribute("data-window", "screenshot-overlay");
  render(() => <ScreenshotOverlayApp />, root);
} else if (windowType === "spec-board") {
  render(() => <SpecBoardApp />, root);
} else {
  render(() => <MainApp />, root);
}
