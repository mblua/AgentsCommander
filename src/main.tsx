import { render } from "solid-js/web";
import SidebarApp from "./sidebar/App";
import TerminalApp from "./terminal/App";

const params = new URLSearchParams(window.location.search);
const windowType = params.get("window") || "sidebar";

const root = document.getElementById("root");
if (!root) throw new Error("Root element not found");

if (windowType === "terminal") {
  const lockedSessionId = params.get("sessionId") || undefined;
  const isDetached = params.get("detached") === "true";
  render(
    () => (
      <TerminalApp lockedSessionId={lockedSessionId} detached={isDetached} />
    ),
    root
  );
} else {
  render(() => <SidebarApp />, root);
}
