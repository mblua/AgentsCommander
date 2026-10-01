import { Component, createSignal, createMemo, createEffect, on, For, Show, onMount, onCleanup } from "solid-js";
import type { AcTeam } from "../../shared/types";
import { EntityAPI } from "../../shared/ipc";
import { projectStore } from "../stores/project";

const NewWorkgroupModal: Component<{
  projectPath: string;
  teams: AcTeam[];
  onClose: () => void;
}> = (props) => {
  const initiallyUnique = props.teams.length === 1;
  const initialTeam = initiallyUnique ? props.teams[0].name : "";
  const [selectedTeam, setSelectedTeam] = createSignal(initialTeam);
  const [taskTitle, setTaskTitle] = createSignal("");
  const [teamSearch, setTeamSearch] = createSignal(initialTeam);
  const [listOpen, setListOpen] = createSignal(false);
  const [activeIndex, setActiveIndex] = createSignal(-1);
  let searchInput!: HTMLInputElement;
  let listElement!: HTMLDivElement;
  let compositionActive = false;
  let suppressFocus = false;
  const [error, setError] = createSignal("");
  const [creating, setCreating] = createSignal(false);

  const filteredTeams = createMemo(() => {
    const query = teamSearch().trim().toLowerCase();
    return props.teams.filter((team) => team.name.toLowerCase().includes(query));
  });
  const canCreate = createMemo(() =>
    selectedTeam() !== "" && filteredTeams().some((team) => team.name === selectedTeam())
  );
  const resetActive = () => setActiveIndex(-1);
  const closeTeamList = () => {
    setListOpen(false);
    resetActive();
  };
  const confirmTeam = (name: string) => {
    if (!filteredTeams().some((team) => team.name === name)) return;
    setSelectedTeam(name);
    setTeamSearch(name);
    closeTeamList();
  };
  const handleTeamInput = (e: InputEvent & { currentTarget: HTMLInputElement }) => {
    setSelectedTeam("");
    setTeamSearch(e.currentTarget.value);
    setListOpen(true);
    resetActive();
  };
  const handleTeamKeyDown = (e: KeyboardEvent) => {
    if (e.isComposing || compositionActive) return;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      setListOpen(true);
      const count = filteredTeams().length;
      const next = activeIndex() < 0 ? 0 : activeIndex() + (e.key === "ArrowDown" ? 1 : -1);
      const index = count === 0 ? -1 : Math.max(0, Math.min(count - 1, next));
      setActiveIndex(index);
      if (index >= 0) listElement.children[index]?.scrollIntoView({ block: "nearest" });
    } else if (e.key === "Enter" && listOpen()) {
      e.preventDefault();
      e.stopPropagation();
      const team = filteredTeams()[activeIndex()];
      if (team) confirmTeam(team.name);
    } else if (e.key === "Escape" && listOpen()) {
      e.preventDefault();
      e.stopPropagation();
      closeTeamList();
    } else if (e.key === "Tab") {
      closeTeamList();
    }
  };
  const handleTeamBlur = () => {
    closeTeamList();
    compositionActive = false;
  };
  const handleTeamsRefresh = () => {
    resetActive();
    if (selectedTeam() && !props.teams.some((team) => team.name === selectedTeam())) {
      setSelectedTeam("");
      setTeamSearch("");
    }
  };
  createEffect(on(() => props.teams, handleTeamsRefresh));
  onMount(() => {
    suppressFocus = initiallyUnique;
    try {
      searchInput.focus();
    } finally {
      suppressFocus = false;
    }
  });

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
      setError(typeof e === "string" ? e : e.message || "Failed to create room");
      setCreating(false);
    }
  };

  const handleKeyDown = (e: KeyboardEvent) => {
    if (e.defaultPrevented || e.isComposing || compositionActive) return;
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      handleCreate();
    }
  };

  const handleDocumentKeyDown = (e: KeyboardEvent) => {
    if (!e.defaultPrevented && !e.isComposing && !compositionActive && e.key === "Escape") props.onClose();
  };

  document.addEventListener("keydown", handleDocumentKeyDown);
  onCleanup(() => document.removeEventListener("keydown", handleDocumentKeyDown));

  return (
    <div class="modal-overlay" onKeyDown={handleKeyDown}>
      <div class="agent-modal new-room-modal" data-ac-testid="newRoom.modal" data-ac-role="dialog">
        <div class="agent-modal-header">
          <span class="agent-modal-title">New Room</span>
        </div>

        <div class="new-agent-form">
          <div class="new-agent-field">
            <label class="new-agent-label" for="new-room-team-search">Team</label>
            <input
              ref={searchInput}
              id="new-room-team-search"
              data-ac-testid="newRoom.teamSearch"
              data-ac-detail="Search teams..."
              data-ac-state={selectedTeam() ? "confirmed" : "unconfirmed"}
              type="text"
              class="entity-input"
              role="combobox"
              aria-autocomplete="list"
              aria-expanded={listOpen()}
              aria-controls="new-room-team-list"
              aria-activedescendant={activeIndex() >= 0 ? `new-room-team-option-${activeIndex()}` : undefined}
              placeholder="Search teams..."
              value={teamSearch()}
              onInput={handleTeamInput}
              onFocus={() => {
                if (suppressFocus) return;
                if (!listOpen()) resetActive();
                setListOpen(true);
              }}
              onBlur={handleTeamBlur}
              on:keydown={handleTeamKeyDown}
              onCompositionStart={() => { compositionActive = true; }}
              onCompositionEnd={() => { compositionActive = false; }}
            />
            <Show when={filteredTeams().length === 0}>
              <div role="status" class="new-room-hint" data-ac-testid="newRoom.team.empty">{props.teams.length === 0 ? "No teams available." : "No teams match your search."}</div>
            </Show>
            <div
              ref={listElement}
              id="new-room-team-list"
              class="new-room-team-list"
              role="listbox"
              aria-label="Team"
              hidden={!listOpen()}
              data-ac-testid="newRoom.team.list"
              data-ac-detail={JSON.stringify({ options: filteredTeams().map(team => team.name), active: activeIndex() })}
            >
              <For each={filteredTeams()}>
                {(team, index) => (
                  <div
                    id={`new-room-team-option-${index()}`}
                    class="new-room-team-option"
                    role="option"
                    aria-selected={index() === activeIndex()}
                    data-ac-testid={`newRoom.team.option.${index()}`}
                    data-ac-role="text"
                    data-ac-detail={team.name}
                    data-ac-state={index() === activeIndex() ? "active" : "inactive"}
                    onMouseDown={(e) => e.preventDefault()}
                    onClick={() => confirmTeam(team.name)}
                  >{team.name}</div>
                )}
              </For>
            </div>
            <span
              class="new-room-hint"
              data-ac-testid="newRoom.team.confirmed"
              data-ac-role="text"
              data-ac-state={selectedTeam() ? "confirmed" : "unconfirmed"}
              data-ac-detail={JSON.stringify({ selected: selectedTeam() })}
            >{selectedTeam() ? `Selected team: ${selectedTeam()}` : "No team selected."}</span>
          </div>

          <div class="new-agent-field">
            <label class="new-agent-label" for="new-room-task-title">Task Title</label>
            <input
              id="new-room-task-title"
              type="text"
              class="entity-input"
              value={taskTitle()}
              onInput={(e) => setTaskTitle(e.currentTarget.value)}
              placeholder="Task title (optional)"
              data-ac-testid="newRoom.taskTitle"
              data-ac-detail="Task title (optional)"
            />
            <span class="new-room-hint" data-ac-testid="newRoom.taskTitle.hint" data-ac-role="text">Leave empty to start with Clean.</span>
            <span id="task-keyhint" class="new-room-hint">Enter to create</span>
          </div>

          <Show when={creating()}>
            <div class="wizard-loading">Creating room (cloning repos may take a moment)...</div>
          </Show>

          <Show when={error()}>
            <div class="new-agent-error">{error()}</div>
          </Show>
        </div>

        <div class="new-agent-footer">
          <button data-ac-testid="newRoom.cancel" type="button" class="new-agent-cancel-btn" onClick={() => props.onClose()} disabled={creating()}>Cancel</button>
          <button
            class="new-agent-create-btn"
            data-ac-testid="newRoom.create"
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
