import { Component } from "solid-js";
import SidebarApp from "../sidebar/App";
import TerminalApp from "../terminal/App";
import "../sidebar/styles/sidebar.css";
import "../terminal/styles/terminal.css";
import "./styles/browser.css";

/**
 * Combined browser layout: sidebar (30%) + terminal (70%) in a single page.
 * Used when accessing AgentsCommander via web browser instead of Tauri.
 */
const BrowserApp: Component = () => {
  return (
    <div class="browser-layout">
      <div class="browser-sidebar">
        <SidebarApp />
      </div>
      <div class="browser-terminal">
        <TerminalApp />
      </div>
    </div>
  );
};

export default BrowserApp;
