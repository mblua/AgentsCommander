import { Component, For, Show, createSignal } from "solid-js";
import { NO_TEAM } from "../../shared/constants";
import { GuideAPI } from "../../shared/ipc";
import { sessionsStore } from "../stores/sessions";
import SettingsModal from "./SettingsModal";

const TeamFilter: Component = () => {
  const [showSettings, setShowSettings] = createSignal(false);

  const handleChange = (e: Event) => {
    const value = (e.target as HTMLSelectElement).value;
    sessionsStore.setTeamFilter(value === "" ? null : value);
  };

  return (
    <>
      <div class="team-filter">
        <Show when={sessionsStore.teams.length > 0}>
          <div class="team-filter-wrapper">
            <select
              class="team-filter-select"
              value={sessionsStore.teamFilter ?? ""}
              onChange={handleChange}
            >
              <option value="">All</option>
              <option value={NO_TEAM}>No team</option>
              <For each={sessionsStore.teams}>
                {(team) => <option value={team.id}>{team.name}</option>}
              </For>
            </select>
            <svg class="team-filter-chevron" width="10" height="6" viewBox="0 0 10 6" fill="none">
              <path d="M1 1l4 4 4-4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" />
            </svg>
          </div>
        </Show>
        <div class="team-filter-actions">
          <button
            class={`toolbar-gear-btn show-inactive-btn ${sessionsStore.showInactive ? "active" : ""}`}
            onClick={() => sessionsStore.toggleShowInactive()}
            title={sessionsStore.showInactive ? "Hide inactive agents" : "Show inactive agents"}
          >
            &#x1F441;
          </button>
          <button
            class="toolbar-gear-btn"
            onClick={() => GuideAPI.open()}
            title="Hints"
          >
            &#x1F4A1;
          </button>
          <button
            class="toolbar-gear-btn"
            onClick={() => setShowSettings(true)}
            title="Settings"
          >
            &#x2699;
          </button>
        </div>
      </div>
      {showSettings() && (
        <SettingsModal onClose={() => setShowSettings(false)} />
      )}
    </>
  );
};

export default TeamFilter;
