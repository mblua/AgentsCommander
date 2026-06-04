import { Component, createSignal, createEffect, onCleanup, Show, onMount } from "solid-js";
import { open } from "@tauri-apps/plugin-dialog";
import { projectStore } from "../stores/project";
import { sessionsStore } from "../stores/sessions";
import type { UnlistenFn } from "../../shared/transport";
import { ProjectAPI, GuideAPI, SettingsAPI, SpecBoardAPI, emitThemeChanged, onOpenSettings } from "../../shared/ipc";
import { settingsStore } from "../../shared/stores/settings";
import { setSoundsEnabled } from "../../shared/sound";
import { homeStore } from "../../main/stores/home";
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
  const [isPendingDialog, setIsPendingDialog] = createSignal(false);
  // #289 — local synchronous mirror of the persisted theme. Drives the button
  // glyph directly so rapid double-clicks each see the freshly-toggled value
  // (a getter that reads settingsStore.current?.themeLight would see the
  // pre-write value until fire-and-forget refresh() resolves, so clicks 2+ in
  // a quick burst would all flip from the same stale state). createEffect
  // syncs in when the store loads or changes externally (SettingsModal, the
  // peer window's theme_changed event, etc.). Default true mirrors
  // AppSettings::default for the brief window before load() resolves on mount.
  const [localThemeLight, setLocalThemeLight] = createSignal(
    settingsStore.current?.themeLight ?? true,
  );
  createEffect(() => {
    const t = settingsStore.current?.themeLight;
    if (t !== undefined) setLocalThemeLight(t);
  });
  const isLight = (): boolean => localThemeLight();
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
      const { picked, hasWorkspace } = await projectStore.pickAndCheck();
      if (!picked) return;
      if (!hasWorkspace) {
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
      const hasWorkspace = await ProjectAPI.checkPath(picked);
      if (hasWorkspace) {
        await projectStore.loadProject(picked);
      } else {
        showToast("No AC project found in this folder (.ac/ not found)");
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

  // #158 — global app-sound mute. Default true so old settings.json files
  // (no `soundsEnabled` field) stay audible. setSoundsEnabled is pushed
  // synchronously BEFORE the persist roundtrip so a beep that fires between
  // the click and the IPC reply (e.g. team-idle-watcher transitioning during
  // file IO) sees the new gate value — the user's intent in clicking mute is
  // exactly to suppress imminent beeps. On persist failure we rollback the
  // gate and toast.
  const isSoundsEnabled = (): boolean =>
    settingsStore.current?.soundsEnabled ?? true;
  const handleToggleMute = async () => {
    const previousValue = isSoundsEnabled();
    const newValue = !previousValue;
    setSoundsEnabled(newValue);
    try {
      await SettingsAPI.setSoundsEnabled(newValue);
      settingsStore.refresh();
    } catch (err) {
      setSoundsEnabled(previousValue);
      const msg = err instanceof Error ? err.message : String(err);
      showToast(`Failed to ${newValue ? "unmute" : "mute"} sounds: ${msg}`);
    }
  };

  // #289 — flip the DOM class optimistically (snappy UI), persist, and roll
  // back both DOM and toast on failure. Mirrors handleToggleMute: the user
  // intent in clicking is to *see* the new theme immediately, so the visual
  // swap precedes the IPC roundtrip. emitThemeChanged keeps the other window
  // in sync; on rollback we re-emit the previous value so it follows back.
  const applyThemeClass = (light: boolean) => {
    if (light) {
      document.documentElement.classList.add("light-theme");
    } else {
      document.documentElement.classList.remove("light-theme");
    }
  };
  const handleToggleTheme = async () => {
    const previousValue = isLight();
    const newValue = !previousValue;
    setLocalThemeLight(newValue);
    applyThemeClass(newValue);
    emitThemeChanged(newValue).catch(console.error);
    try {
      await SettingsAPI.setThemeLight(newValue);
      settingsStore.refresh();
    } catch (err) {
      setLocalThemeLight(previousValue);
      applyThemeClass(previousValue);
      emitThemeChanged(previousValue).catch(console.error);
      const msg = err instanceof Error ? err.message : String(err);
      showToast(`Failed to switch to ${newValue ? "light" : "dark"} theme: ${msg}`);
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
          <Show when={settingsStore.current?.specBoardEnabled === true}>
            <button class="toolbar-gear-btn" onClick={() => SpecBoardAPI.open()} title="Spec board">
              &#x25A7;
            </button>
          </Show>

          <button
            class={`toolbar-gear-btn home-toggle-btn ${homeStore.visible ? "active" : ""}`}
            onClick={() => homeStore.toggle()}
            title={homeStore.visible ? "Hide Home" : "Show Home"}
            aria-label={homeStore.visible ? "Hide Home" : "Show Home"}
            aria-pressed={homeStore.visible}
          >
            &#x1F3E0;
          </button>
          <button
            class={`toolbar-gear-btn coord-sort-activity-btn ${sessionsStore.coordSortByActivity ? "active" : ""}`}
            disabled={!sessionsStore.hydrated || sessionsStore.toggleInFlight}
            onClick={() => sessionsStore.toggleCoordSortByActivity()}
            title={sessionsStore.coordSortByActivity ? "Show recent coordinators first" : "Show coordinators in default order"}
          >
            &#x1F525;
          </button>
          <button
            class={`toolbar-gear-btn sounds-mute-btn ${isSoundsEnabled() ? "" : "active"}`}
            disabled={!settingsStore.current}
            onClick={handleToggleMute}
            title={isSoundsEnabled() ? "Mute all app sounds" : "Unmute app sounds"}
            aria-label={isSoundsEnabled() ? "Mute all app sounds" : "Unmute app sounds"}
          >
            {isSoundsEnabled() ? "🔊" : "🔇"}
          </button>
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
            disabled={!settingsStore.current}
            onClick={handleToggleTheme}
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
