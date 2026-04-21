import { Component, createSignal, createEffect, onCleanup, Show, onMount } from "solid-js";
import { open } from "@tauri-apps/plugin-dialog";
import { projectStore } from "../stores/project";
import { sessionsStore } from "../stores/sessions";
import type { UnlistenFn } from "../../shared/transport";
import { ProjectAPI, GuideAPI, emitThemeChanged, onOpenSettings } from "../../shared/ipc";
import SettingsModal from "./SettingsModal";

const ActionBar: Component = () => {
  const [showDropdown, setShowDropdown] = createSignal(false);
  const [showSettings, setShowSettings] = createSignal(false);
  // `equals: false` → each write notifies even if the value is the same. Lets
  // a second disabled-mic click re-snap the modal back to Integrations if the
  // user manually navigated away to another tab between clicks.
  const [pendingSection, setPendingSection] = createSignal<string | undefined>(undefined, { equals: false });
  const [confirmPath, setConfirmPath] = createSignal<string | null>(null);
  const [toastMsg, setToastMsg] = createSignal<string | null>(null);
  const [isLight, setIsLight] = createSignal(true);
  const [isPendingDialog, setIsPendingDialog] = createSignal(false);
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

  // Cross-window / same-window trigger to open the Settings modal (e.g. from a
  // disabled mic button prompting the user to configure voice). The optional
  // `section` argument targets a specific tab — SettingsModal picks it up via
  // the `section` prop and its createEffect re-targets the tab if the modal is
  // already open.
  let unlistenOpenSettings: UnlistenFn | null = null;
  onMount(async () => {
    unlistenOpenSettings = await onOpenSettings((section) => {
      setPendingSection(section);
      setShowSettings(true);
    });
  });
  onCleanup(() => {
    if (unlistenOpenSettings) unlistenOpenSettings();
  });

  const handleNewProject = async () => {
    if (isPendingDialog()) return;
    setShowDropdown(false);
    setIsPendingDialog(true);
    try {
      const { picked, hasAcNew } = await projectStore.pickAndCheck();
      if (!picked) return;
      if (!hasAcNew) {
        await projectStore.createAndLoad(picked);
      }
    } finally {
      setIsPendingDialog(false);
    }
  };

  const showToast = (msg: string) => {
    setToastMsg(msg);
    setTimeout(() => setToastMsg(null), 3000);
  };

  const handleOpenProject = async () => {
    if (isPendingDialog()) return;
    setShowDropdown(false);
    setIsPendingDialog(true);
    try {
      const picked = await open({ directory: true, title: "Select AC Project Folder" });
      if (!picked) return;
      const hasAcNew = await ProjectAPI.checkPath(picked);
      if (hasAcNew) {
        await projectStore.loadProject(picked);
      } else {
        showToast("No AC project found in this folder (.ac-new/ not found)");
      }
    } finally {
      setIsPendingDialog(false);
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
            disabled={isPendingDialog()}
            onClick={() => setShowDropdown(!showDropdown())}
          >
            New / Open
            <svg class="action-bar-chevron" width="10" height="6" viewBox="0 0 10 6" fill="none">
              <path d="M1 1l4 4 4-4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" />
            </svg>
          </button>
          <Show when={showDropdown()}>
            <div class="action-bar-menu">
              <button class="action-bar-menu-item" disabled={isPendingDialog()} onClick={handleNewProject}>
                &#x1F4C1; New Project
              </button>
              <button class="action-bar-menu-item" disabled={isPendingDialog()} onClick={handleOpenProject}>
                &#x1F4C2; Open Project
              </button>
            </div>
          </Show>
        </div>
        <div class="action-bar-icons">
          <button
            class={`toolbar-gear-btn show-categories-btn ${sessionsStore.showCategories ? "active" : ""}`}
            onClick={() => sessionsStore.toggleShowCategories()}
            title={sessionsStore.showCategories ? "Hide category sections" : "Show category sections"}
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
              emitThemeChanged(next).catch(console.error);
            }}
            title="Toggle theme"
          >
            {isLight() ? "\u2600\uFE0F" : "\uD83C\uDF19"}
          </button>
          <button
            class="toolbar-gear-btn"
            onClick={() => { setPendingSection(undefined); setShowSettings(true); }}
            title="Settings"
          >
            &#x2699;
          </button>
        </div>
      </div>

      {showSettings() && (
        <SettingsModal
          onClose={() => setShowSettings(false)}
          section={pendingSection()}
        />
      )}
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
