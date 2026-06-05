import { Component, createSignal, createMemo, For, Show, onMount } from "solid-js";
import type { AgentConfig } from "../../shared/types";
import { SettingsAPI } from "../../shared/ipc";

const AgentPickerModal: Component<{
  sessionName: string;
  onSelect: (agent: AgentConfig) => void;
  onClose: () => void;
}> = (props) => {
  const [agents, setAgents] = createSignal<AgentConfig[]>([]);
  const [highlightIndex, setHighlightIndex] = createSignal(0);
  let overlayRef!: HTMLDivElement;

  const sortedAgents = createMemo(() =>
    [...agents()].sort((a, b) =>
      a.label.localeCompare(b.label, "en", { sensitivity: "base", numeric: true })
    )
  );

  onMount(async () => {
    overlayRef?.focus();
    const settings = await SettingsAPI.get();
    setAgents(settings.agents);
  });

  const handleKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      props.onClose();
      return;
    }
    const list = sortedAgents();
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setHighlightIndex((i) => Math.min(i + 1, list.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setHighlightIndex((i) => Math.max(i - 1, 0));
    } else if (e.key === "Enter" && list.length > 0) {
      e.preventDefault();
      props.onSelect(list[highlightIndex()]);
    }
  };

  return (
    <div ref={overlayRef} class="modal-overlay" tabIndex={0} onKeyDown={handleKeyDown}>
      <div class="agent-modal new-agent-modal">
        <div class="agent-modal-header">
          <span class="agent-modal-title">
            Launch <strong>{props.sessionName}</strong>
          </span>
        </div>
        <div class="agent-modal-list">
          <Show
            when={sortedAgents().length > 0}
            fallback={<div class="agent-modal-empty">No agents configured. Add agents in Settings.</div>}
          >
            <For each={sortedAgents()}>
              {(agent, i) => (
                <div
                  class={`agent-modal-item agent-choice ${i() === highlightIndex() ? "highlighted" : ""}`}
                  onClick={() => props.onSelect(agent)}
                  onMouseEnter={() => setHighlightIndex(i())}
                >
                  <div
                    class="agent-color-badge"
                    style={{ background: agent.color }}
                  />
                  <div class="agent-modal-item-info">
                    <div class="agent-modal-item-name">{agent.label}</div>
                    <div class="agent-modal-item-detail">{agent.command}</div>
                  </div>
                </div>
              )}
            </For>
          </Show>
        </div>
        <div class="agent-modal-footer">
          <span>&#x2191;&#x2193; navigate</span>
          <span>&#x23CE; launch</span>
          <span>esc close</span>
        </div>
      </div>
    </div>
  );
};

export default AgentPickerModal;
