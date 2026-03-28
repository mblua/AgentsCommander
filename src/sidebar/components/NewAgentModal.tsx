import { Component, createSignal, createMemo, For, Show, onMount } from "solid-js";
import type { AgentConfig } from "../../shared/types";
import { AgentCreatorAPI, SessionAPI, SettingsAPI } from "../../shared/ipc";

type Stage = "form" | "launch";

const NewAgentModal: Component<{ onClose: () => void }> = (props) => {
  const [stage, setStage] = createSignal<Stage>("form");
  const [parentPath, setParentPath] = createSignal("");
  const [agentName, setAgentName] = createSignal("");
  const [createdPath, setCreatedPath] = createSignal("");
  const [error, setError] = createSignal("");
  const [creating, setCreating] = createSignal(false);
  const [agents, setAgents] = createSignal<AgentConfig[]>([]);
  const [highlightIndex, setHighlightIndex] = createSignal(0);
  let nameInputRef!: HTMLInputElement;

  const sortedAgents = createMemo(() =>
    [...agents()].sort((a, b) => a.label.localeCompare(b.label, "en", { sensitivity: "base", numeric: true }))
  );

  // Derive display prefix: last folder component of parentPath + "/"
  const parentDisplay = createMemo(() => {
    const p = parentPath();
    if (!p) return "";
    const normalized = p.replace(/\\/g, "/").replace(/\/+$/, "");
    const last = normalized.split("/").pop() || normalized;
    return last + "/";
  });

  const canCreate = createMemo(() => {
    const name = agentName().trim();
    return parentPath() !== "" && name !== "" && !name.includes("/") && !name.includes("\\");
  });

  onMount(async () => {
    const settings = await SettingsAPI.get();
    setAgents(settings.agents);
  });

  const handleBrowse = async () => {
    const selected = await AgentCreatorAPI.pickFolder(parentPath() || undefined);
    if (selected) {
      setParentPath(selected);
      setError("");
      requestAnimationFrame(() => nameInputRef?.focus());
    }
  };

  const handleCreate = async () => {
    if (!canCreate()) return;
    setCreating(true);
    setError("");
    try {
      const path = await AgentCreatorAPI.createFolder(parentPath(), agentName().trim());
      setCreatedPath(path);
      setHighlightIndex(0);
      setStage("launch");
    } catch (e: any) {
      setError(typeof e === "string" ? e : e.message || "Failed to create agent folder");
    } finally {
      setCreating(false);
    }
  };

  const handleLaunch = (agent: AgentConfig) => {
    const parts = agent.command.trim().split(/\s+/);
    const executable = parts[0];
    const cmdArgs = parts.slice(1);

    let shell: string;
    let shellArgs: string[];

    if (agent.gitPullBefore) {
      shell = "cmd.exe";
      shellArgs = ["/K", `git pull && ${agent.command}`];
    } else {
      shell = executable;
      shellArgs = cmdArgs;
    }

    // Session name: parentFolder/agentName
    const normalized = parentPath().replace(/\\/g, "/").replace(/\/+$/, "");
    const parentName = normalized.split("/").pop() || normalized;
    const sessionName = `${parentName}/${agentName().trim()}`;

    SessionAPI.create({
      shell,
      shellArgs,
      cwd: createdPath(),
      sessionName,
      agentId: agent.id,
    });

    props.onClose();
  };

  const handleKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      if (stage() === "launch") {
        // Can't go back — folder is already created
        props.onClose();
      } else {
        props.onClose();
      }
      return;
    }

    if (stage() === "form" && e.key === "Enter") {
      e.preventDefault();
      handleCreate();
      return;
    }

    if (stage() === "launch") {
      const list = sortedAgents();
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setHighlightIndex((i) => Math.min(i + 1, list.length - 1));
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        setHighlightIndex((i) => Math.max(i - 1, 0));
      } else if (e.key === "Enter" && list.length > 0) {
        e.preventDefault();
        handleLaunch(list[highlightIndex()]);
      }
    }
  };

  const handleOverlayClick = (e: MouseEvent) => {
    if ((e.target as HTMLElement).classList.contains("modal-overlay")) {
      props.onClose();
    }
  };

  return (
    <div class="modal-overlay" onClick={handleOverlayClick} onKeyDown={handleKeyDown}>
      <div class="agent-modal new-agent-modal">
        <Show
          when={stage() === "form"}
          fallback={
            <>
              {/* Stage 2: Launch agent selection */}
              <div class="agent-modal-header">
                <span class="agent-modal-title">
                  Launch <strong>{parentDisplay()}{agentName().trim()}</strong>
                </span>
              </div>
              <div class="agent-modal-list">
                <For each={sortedAgents()}>
                  {(agent, i) => (
                    <div
                      class={`agent-modal-item agent-choice ${i() === highlightIndex() ? "highlighted" : ""}`}
                      onClick={() => handleLaunch(agent)}
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
              </div>
              <div class="agent-modal-footer">
                <span>&#x2191;&#x2193; navigate</span>
                <span>&#x23CE; launch</span>
                <span>esc close</span>
              </div>
            </>
          }
        >
          {/* Stage 1: Form */}
          <div class="agent-modal-header">
            <span class="agent-modal-title">New Agent</span>
          </div>

          <div class="new-agent-form">
            {/* Folder picker */}
            <div class="new-agent-field">
              <label class="new-agent-label">Parent folder</label>
              <div class="new-agent-path-row">
                <input
                  class="agent-search-input new-agent-path-input"
                  value={parentPath()}
                  onInput={(e) => {
                    setParentPath(e.currentTarget.value);
                    setError("");
                  }}
                  placeholder="Select a folder..."
                  readOnly
                />
                <button class="new-agent-browse-btn" onClick={handleBrowse}>
                  Browse
                </button>
              </div>
            </div>

            {/* Agent name */}
            <div class="new-agent-field">
              <label class="new-agent-label">Agent name</label>
              <div class="new-agent-name-row">
                <Show when={parentDisplay()}>
                  <span class="new-agent-prefix">{parentDisplay()}</span>
                </Show>
                <input
                  ref={nameInputRef!}
                  class="agent-search-input new-agent-name-input"
                  value={agentName()}
                  onInput={(e) => {
                    setAgentName(e.currentTarget.value);
                    setError("");
                  }}
                  placeholder="my-agent"
                />
              </div>
            </div>

            {/* Error message */}
            <Show when={error()}>
              <div class="new-agent-error">{error()}</div>
            </Show>
          </div>

          {/* Footer with create button */}
          <div class="new-agent-footer">
            <button class="new-agent-cancel-btn" onClick={() => props.onClose()}>
              Cancel
            </button>
            <button
              class="new-agent-create-btn"
              disabled={!canCreate() || creating()}
              onClick={handleCreate}
            >
              {creating() ? "Creating..." : "Create Agent"}
            </button>
          </div>
        </Show>
      </div>
    </div>
  );
};

export default NewAgentModal;
