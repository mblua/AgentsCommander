import "./shared/console-capture";
import { render } from "solid-js/web";
import SidebarApp from "./sidebar/App";
import TerminalApp from "./terminal/App";
import GuideApp from "./guide/App";
import DarkFactoryApp from "./darkfactory/App";

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
} else if (windowType === "guide") {
  render(() => <GuideApp />, root);
} else if (windowType === "darkfactory") {
  render(() => <DarkFactoryApp />, root);
} else {
  render(() => <SidebarApp />, root);
}
