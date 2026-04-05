import "./shared/console-capture";
import { render } from "solid-js/web";
import { isTauri } from "./shared/platform";
import SidebarApp from "./sidebar/App";
import TerminalApp from "./terminal/App";
import GuideApp from "./guide/App";
import BrowserApp from "./browser/App";
const params = new URLSearchParams(window.location.search);
const windowType = params.get("window");

// Capture remote token from URL for WebSocket auth (browser mode)
const remoteToken = params.get("remoteToken");
if (remoteToken) {
  sessionStorage.setItem("remoteToken", remoteToken);
}

const root = document.getElementById("root");
if (!root) throw new Error("Root element not found");
root.textContent = "";

if (windowType === "terminal") {
  const lockedSessionId = params.get("sessionId") || undefined;
  const isDetached = params.get("detached") === "true";
  render(
    () => (
      <TerminalApp lockedSessionId={lockedSessionId} detached={isDetached} />
    ),
    root
  );
} else if (windowType === "guide") {
  render(() => <GuideApp />, root);
} else if (windowType === "sidebar") {
  render(() => <SidebarApp />, root);
} else if (!isTauri) {
  // Browser without ?window param: show combined split layout
  render(() => <BrowserApp />, root);
} else {
  // Tauri default: sidebar
  render(() => <SidebarApp />, root);
}
