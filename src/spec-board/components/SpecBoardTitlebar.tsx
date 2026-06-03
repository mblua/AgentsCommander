
import { Component } from "solid-js";
import { getCurrentWindow } from "@tauri-apps/api/window";

const SpecBoardTitlebar: Component = () => {
  const appWindow = getCurrentWindow();
  
  return (
    <div data-tauri-drag-region class="spec-board-titlebar">
      <div data-tauri-drag-region class="spec-board-titlebar-title">Spec Board</div>
      <div class="spec-board-titlebar-controls">
        <button onClick={(e) => { e.stopPropagation(); appWindow.minimize(); }}>&#x2014;</button>
        <button onClick={(e) => { e.stopPropagation(); appWindow.toggleMaximize(); }}>&#x25A1;</button>
        <button onClick={(e) => { e.stopPropagation(); appWindow.close(); }}>&#x2715;</button>
      </div>
    </div>
  );
};

export default SpecBoardTitlebar;
