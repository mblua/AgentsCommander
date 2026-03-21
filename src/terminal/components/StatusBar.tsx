import { Component, Show } from "solid-js";
import { terminalStore } from "../stores/terminal";

const StatusBar: Component<{ detached?: boolean }> = (props) => {
  return (
    <div class="status-bar">
      <Show when={props.detached}>
        <div class="status-bar-item">
          <span class="status-bar-detached">DETACHED</span>
        </div>
      </Show>
      <Show when={terminalStore.activeShell}>
        <div class="status-bar-item">
          <span class="status-bar-accent">{terminalStore.activeShell}</span>
        </div>
      </Show>
      <Show when={terminalStore.termSize.cols > 0}>
        <div class="status-bar-item">
          {terminalStore.termSize.cols}x{terminalStore.termSize.rows}
        </div>
      </Show>
    </div>
  );
};

export default StatusBar;
