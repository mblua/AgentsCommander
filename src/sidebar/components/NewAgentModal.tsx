import { Component, createSignal, createMemo, Show, onCleanup } from "solid-js";
import { AgentCreatorAPI, SessionAPI } from "../../shared/ipc";
import { homeStore } from "../../main/stores/home";
import AgentPickerModal, { type AgentPickerSelection } from "./AgentPickerModal";

type Stage = "form" | "launch";

const NewAgentModal: Component<{ onClose: () => void }> = (props) => {
  const [stage, setStage] = createSignal<Stage>("form");
  const [parentPath, setParentPath] = createSignal("");
  const [agentName, setAgentName] = createSignal("");
  const [createdPath, setCreatedPath] = createSignal("");
  const [error, setError] = createSignal("");
  const [creating, setCreating] = createSignal(false);
  const [isPicking, setIsPicking] = createSignal(false);
  let nameInputRef!: HTMLInputElement;

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

  const handleBrowse = async () => {
    if (isPicking()) return;
    setIsPicking(true);
    try {
      const selected = await AgentCreatorAPI.pickFolder(parentPath() || undefined);
      if (selected) {
        setParentPath(selected);
        setError("");
        requestAnimationFrame(() => nameInputRef?.focus());
      }
    } finally {
      setIsPicking(false);
    }
  };

  const handleCreate = async () => {
    if (!canCreate()) return;
    setCreating(true);
    setError("");
    try {
      const path = await AgentCreatorAPI.createFolder(parentPath(), agentName().trim());
      setCreatedPath(path);
      setStage("launch");
    } catch (e: any) {
      setError(typeof e === "string" ? e : e.message || "Failed to create agent folder");
    } finally {
      setCreating(false);
    }
  };

  const launchSessionName = () => {
    const normalized = parentPath().replace(/\\/g, "/").replace(/\/+$/, "");
    const parentName = normalized.split("/").pop() || normalized;
    return `${parentName}/${agentName().trim()}`;
  };

  const handleLaunch = async (selection: AgentPickerSelection) => {
    const agent = selection.agent;
    // Auto-generate .claude/settings.local.json if the agent has the flag
    if (agent.excludeGlobalClaudeMd && createdPath()) {
      try {
        await AgentCreatorAPI.writeClaudeSettingsLocal(createdPath());
      } catch (e) {
        console.warn("Failed to write .claude/settings.local.json:", e);
      }
    }

    homeStore.hide();
    await SessionAPI.create({
      cwd: createdPath(),
      sessionName: launchSessionName(),
      agentId: agent.id,
      requestedProfile: selection.requestedProfile,
    });

    props.onClose();
  };

  const handleKeyDown = (e: KeyboardEvent) => {
    if (stage() === "form" && e.key === "Enter") {
      e.preventDefault();
      handleCreate();
      return;
    }

  };

  const handleDocumentKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") props.onClose();
  };

  document.addEventListener("keydown", handleDocumentKeyDown);
  onCleanup(() => document.removeEventListener("keydown", handleDocumentKeyDown));

  if (stage() === "launch") {
    return (
      <AgentPickerModal
        sessionName={launchSessionName()}
        agentPath={createdPath()}
        onSelect={handleLaunch}
        onClose={props.onClose}
      />
    );
  }

  return (
    <div class="modal-overlay" onKeyDown={handleKeyDown}>
      <div class="agent-modal new-agent-modal">
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
                <button class="new-agent-browse-btn" disabled={isPicking()} onClick={handleBrowse}>
                  {isPicking() ? "Picking..." : "Browse"}
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
      </div>
    </div>
  );
};

export default NewAgentModal;
