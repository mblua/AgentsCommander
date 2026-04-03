import { Component, createSignal, createEffect, onCleanup, Show } from "solid-js";
import { open } from "@tauri-apps/plugin-dialog";
import { projectStore } from "../stores/project";
import { sessionsStore } from "../stores/sessions";
import { ProjectAPI, GuideAPI, DarkFactoryWindowAPI } from "../../shared/ipc";
import OpenAgentModal from "./OpenAgentModal";
import NewAgentModal from "./NewAgentModal";
import SettingsModal from "./SettingsModal";

const ActionBar: Component = () => {
  const [showDropdown, setShowDropdown] = createSignal(false);
  const [showOpenAgent, setShowOpenAgent] = createSignal(false);
  const [showNewAgent, setShowNewAgent] = createSignal(false);
  const [showSettings, setShowSettings] = createSignal(false);
  const [confirmPath, setConfirmPath] = createSignal<string | null>(null);
  const [toastMsg, setToastMsg] = createSignal<string | null>(null);
  const [isLight, setIsLight] = createSignal(true);
  let dropdownRef: HTMLDivElement | undefined;

  // Click-away to close dropdown
  const onClickAway = (e: MouseEvent) => {
    if (dropdownRef && !dropdownRef.contains(e.target as Node)) {
      setShowDropdown(false);
    }
  };

  createEffect(() => {
    if (showDropdown()) {
      document.addEventListener("mousedown", onClickAway);
    } else {
      document.removeEventListener("mousedown", onClickAway);
    }
  });

  onCleanup(() => document.removeEventListener("mousedown", onClickAway));

  const handleNewProject = async () => {
    setShowDropdown(false);
    const { picked, hasAcNew } = await projectStore.pickAndCheck();
    if (!picked) return;
    if (!hasAcNew) {
      await projectStore.createAndLoad(picked);
    }
  };

  const showToast = (msg: string) => {
    setToastMsg(msg);
    setTimeout(() => setToastMsg(null), 3000);
  };

  const handleOpenProject = async () => {
    setShowDropdown(false);
    const picked = await open({ directory: true, title: "Select AC Project Folder" });
    if (!picked) return;
    const hasAcNew = await ProjectAPI.checkPath(picked);
    if (hasAcNew) {
      await projectStore.loadProject(picked);
    } else {
      showToast("No AC project found in this folder (.ac-new/ not found)");
    }
  };

  const handleConfirmCreate = async () => {
    const path = confirmPath();
    if (path) {
      await projectStore.createAndLoad(path);
      setConfirmPath(null);
    }
  };

  return (
    <>
      <div class="action-bar">
        <div class="action-bar-dropdown" ref={dropdownRef}>
          <button
            class="action-bar-dropdown-btn"
            onClick={() => setShowDropdown(!showDropdown())}
          >
            New / Open
            <svg class="action-bar-chevron" width="10" height="6" viewBox="0 0 10 6" fill="none">
              <path d="M1 1l4 4 4-4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" />
            </svg>
          </button>
          <Show when={showDropdown()}>
            <div class="action-bar-menu">
              <button class="action-bar-menu-item" onClick={handleNewProject}>
                &#x1F4C1; New Project
              </button>
              <button class="action-bar-menu-item" onClick={handleOpenProject}>
                &#x1F4C2; Open Project
              </button>
              <button class="action-bar-menu-item" onClick={() => { setShowDropdown(false); setShowNewAgent(true); }}>
                &#x2795; New Agent
              </button>
              <button class="action-bar-menu-item" onClick={() => { setShowDropdown(false); setShowOpenAgent(true); }}>
                &#x25B6; Open Agent
              </button>
            </div>
          </Show>
        </div>
        <div class="action-bar-icons">
          <button
            class={`toolbar-gear-btn show-inactive-btn ${sessionsStore.showInactive ? "active" : ""}`}
            onClick={() => sessionsStore.toggleShowInactive()}
            title={sessionsStore.showInactive ? "Hide inactive agents" : "Show inactive agents"}
          >
            &#x1F441;
          </button>
          <button class="toolbar-gear-btn" onClick={() => GuideAPI.open()} title="Hints">
            &#x1F4A1;
          </button>
          <button
            class="toolbar-gear-btn"
            onClick={() => {
              const next = !isLight();
              setIsLight(next);
              if (next) {
                document.documentElement.classList.add("light-theme");
              } else {
                document.documentElement.classList.remove("light-theme");
              }
            }}
            title="Toggle theme"
          >
            {isLight() ? "\u2600\uFE0F" : "\uD83C\uDF19"}
          </button>
          <button class="toolbar-gear-btn" onClick={() => DarkFactoryWindowAPI.open()} title="Dark Factory">
            &#x1F3ED;
          </button>
          <button class="toolbar-gear-btn" onClick={() => setShowSettings(true)} title="Settings">
            &#x2699;
          </button>
        </div>
      </div>
      {showOpenAgent() && <OpenAgentModal onClose={() => setShowOpenAgent(false)} />}
      {showNewAgent() && <NewAgentModal onClose={() => setShowNewAgent(false)} />}
      {showSettings() && <SettingsModal onClose={() => setShowSettings(false)} />}
      <Show when={confirmPath()}>
        <div class="confirm-overlay" onClick={() => setConfirmPath(null)}>
          <div class="confirm-dialog" onClick={(e) => e.stopPropagation()}>
            <p class="confirm-text">
              This folder does not have an AC project. Do you want to create a new project here?
            </p>
            <p class="confirm-path">{confirmPath()}</p>
            <div class="confirm-actions">
              <button class="confirm-btn confirm-btn-yes" onClick={handleConfirmCreate}>
                Yes, create project
              </button>
              <button class="confirm-btn confirm-btn-no" onClick={() => setConfirmPath(null)}>
                Cancel
              </button>
            </div>
          </div>
        </div>
      </Show>
      <Show when={toastMsg()}>
        <div class="toast-error">{toastMsg()}</div>
      </Show>
    </>
  );
};

export default ActionBar;
