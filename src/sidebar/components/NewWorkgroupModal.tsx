import { Component, createSignal, createMemo, For, Show } from "solid-js";
import type { AcTeam } from "../../shared/types";
import { EntityAPI } from "../../shared/ipc";
import { projectStore } from "../stores/project";

const NewWorkgroupModal: Component<{
  projectPath: string;
  teams: AcTeam[];
  onClose: () => void;
}> = (props) => {
  const [selectedTeam, setSelectedTeam] = createSignal(
    props.teams.length === 1 ? props.teams[0].name : ""
  );
  const [taskTitle, setTaskTitle] = createSignal("");
  const [error, setError] = createSignal("");
  const [creating, setCreating] = createSignal(false);

  const canCreate = createMemo(() => selectedTeam() !== "" && taskTitle().trim() !== "");

  const handleCreate = async () => {
    if (!canCreate() || creating()) return;
    setCreating(true);
    setError("");
    try {
      await EntityAPI.createWorkgroup(
        props.projectPath,
        selectedTeam(),
        taskTitle().trim()
      );
      await projectStore.reloadProject(props.projectPath);
      props.onClose();
      // intentionally do NOT clear creating() — modal unmounts; any in-flight
      // keydown event in the close transition stays guarded.
    } catch (e: any) {
      console.error("create_workgroup failed:", e);
      setError(typeof e === "string" ? e : e.message || "Failed to create workgroup");
      setCreating(false);
    }
  };

  const handleKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") props.onClose();
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      handleCreate();
    }
  };

  const handleOverlayClick = (e: MouseEvent) => {
    if ((e.target as HTMLElement).classList.contains("modal-overlay")) props.onClose();
  };

  return (
    <div class="modal-overlay" onClick={handleOverlayClick} onKeyDown={handleKeyDown}>
      <div class="agent-modal new-agent-modal">
        <div class="agent-modal-header">
          <span class="agent-modal-title">New Workgroup</span>
        </div>

        <div class="new-agent-form">
          <div class="new-agent-field">
            <label class="new-agent-label">Team</label>
            <select
              class="entity-select"
              value={selectedTeam()}
              onChange={(e) => setSelectedTeam(e.currentTarget.value)}
            >
              <option value="" disabled>Select a team...</option>
              <For each={props.teams}>
                {(team) => (
                  <option value={team.name}>{team.name}</option>
                )}
              </For>
            </select>
          </div>

          <div class="new-agent-field">
            <label class="new-agent-label">Task Title</label>
            <input
              type="text"
              class="entity-input"
              value={taskTitle()}
              onInput={(e) => setTaskTitle(e.currentTarget.value)}
              placeholder="Task title (required)"
              autofocus
            />
            <div class="entity-textarea-meta">
              <span id="task-keyhint" class="entity-textarea-hint">Enter to create · Shift+Enter for newline</span>
            </div>
          </div>

          <Show when={creating()}>
            <div class="wizard-loading">Creating workgroup (cloning repos may take a moment)...</div>
          </Show>

          <Show when={error()}>
            <div class="new-agent-error">{error()}</div>
          </Show>
        </div>

        <div class="new-agent-footer">
          <button type="button" class="new-agent-cancel-btn" onClick={() => props.onClose()} disabled={creating()}>Cancel</button>
          <button
            class="new-agent-create-btn"
            disabled={!canCreate() || creating()}
            onClick={handleCreate}
          >
            {creating() ? "Creating..." : "Create"}
          </button>
        </div>
      </div>
    </div>
  );
};

export default NewWorkgroupModal;
