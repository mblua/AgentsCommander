import { Component, createEffect, createSignal, For, onMount, Show } from "solid-js";
import type { AgentConfig, AppSettings, CodingAgentDefinition } from "../../shared/types";
import { SettingsAPI } from "../../shared/ipc";
import { settingsStore } from "../../shared/stores/settings";
import { newAgentId, definitionToSeed } from "../../shared/agent-presets";
import { codingAgentsStore } from "../stores/coding-agents";

const CUSTOM_PRESET: CodingAgentDefinition = {
  key: "custom",
  label: "Custom Agent",
  description: "Configure your own Coding Agent",
  color: "#6366f1",
  command: "",
  envs: [],
  isolatedHome: false,
  removable: false,
  updateCommands: [],
  autoUpdate: false,
};

export interface CodingAgentQuickConfigurationProps {
  title: string;
  message: string;
  onClose: () => void;
  onCancel?: () => void;
  onBeforeSave?: (settings: AppSettings) => AppSettings;
  ariaLabel?: string;
}

/** #1965 — private detail renderer shared by the catalog error and warning
 *  blocks: the path renders only when present, the reason always. It keeps the
 *  exact elements, order and test ids of the inline markup it replaces and adds
 *  no wrapper element. Props are read reactively, never destructured. */
const DiagnosticDetails: Component<{
  path: string;
  reason: string;
  testIdPrefix: string;
}> = (props) => (
  <>
    <Show when={props.path}>
      <div data-ac-testid={`${props.testIdPrefix}.path`}>{props.path}</div>
    </Show>
    <div data-ac-testid={`${props.testIdPrefix}.reason`}>{props.reason}</div>
  </>
);

const CodingAgentQuickConfiguration: Component<CodingAgentQuickConfigurationProps> = (props) => {
  const [selectedPreset, setSelectedPreset] = createSignal<string | null>(null);
  const [selectionGeneration, setSelectionGeneration] = createSignal<number | null>(null);
  const [saving, setSaving] = createSignal(false);
  const [done, setDone] = createSignal(false);
  const [addedLabel, setAddedLabel] = createSignal("");

  const allPresets = () => [...codingAgentsStore.catalog(), CUSTOM_PRESET];

  const [customLabel, setCustomLabel] = createSignal("");
  const [customCommand, setCustomCommand] = createSignal("");

  // #1965 — a preset selection belongs to the catalog generation it was made
  // in. A project switch or reload invalidates it: drop the catalog selection
  // and keep the manually typed custom fields.
  let observedGeneration = codingAgentsStore.generation();
  createEffect(() => {
    const current = codingAgentsStore.generation();
    if (current === observedGeneration) return;
    observedGeneration = current;
    setSelectedPreset(null);
    setSelectionGeneration(null);
  });

  const isCustom = () => selectedPreset() === "custom";

  /** A catalog preset is current only in the generation that offered it. */
  const isCatalogSelectionCurrent = (): boolean => {
    const key = selectedPreset();
    if (!key || key === "custom") return false;
    if (selectionGeneration() !== codingAgentsStore.generation()) return false;
    return codingAgentsStore.catalog().some((def) => def.key === key);
  };

  const canConfirm = () => {
    if (!selectedPreset()) return false;
    if (isCustom()) return customLabel().trim() !== "" && customCommand().trim() !== "";
    return isCatalogSelectionCurrent();
  };

  const handleSelect = (key: string) => {
    if (key === selectedPreset()) {
      setSelectedPreset(null);
      setSelectionGeneration(null);
      return;
    }
    setSelectedPreset(key);
    setSelectionGeneration(codingAgentsStore.generation());
  };

  const handleConfirm = async () => {
    const key = selectedPreset();
    if (!key) return;

    const preset = allPresets().find((p) => p.key === key);
    if (!preset) return;

    // #1965 — validate the definition and its generation immediately before
    // starting, then again after the asynchronous settings read below: a preset
    // selected from an old generation must not register after switch/reload.
    const presetGeneration = selectionGeneration();
    if (key !== "custom" && !isCatalogSelectionCurrent()) {
      setSelectedPreset(null);
      setSelectionGeneration(null);
      return;
    }

    setSaving(true);
    try {
      const settings = await SettingsAPI.get();

      if (key !== "custom") {
        const staleSelection =
          presetGeneration !== codingAgentsStore.generation() ||
          !codingAgentsStore.catalog().some((def) => def.key === key);
        if (staleSelection) {
          setSelectedPreset(null);
          setSelectionGeneration(null);
          return;
        }
      }

      let agent: AgentConfig;
      if (key === "custom") {
        agent = {
          id: newAgentId(),
          label: customLabel().trim(),
          command: customCommand().trim(),
          color: preset.color,
          envs: [],
          isolatedHome: false,
        };
      } else {
        const current = codingAgentsStore.catalog().find((def) => def.key === key) ?? preset;
        agent = { id: newAgentId(), ...definitionToSeed(current) };
      }

      const withAgent: AppSettings = {
        ...settings,
        agents: [...settings.agents, agent],
      };
      const updated = props.onBeforeSave ? props.onBeforeSave(withAgent) : withAgent;
      await SettingsAPI.update(updated);
      settingsStore.refresh();

      setAddedLabel(agent.label);
      setDone(true);
    } catch (e) {
      console.error("Coding Agent quick configuration save failed:", e);
    } finally {
      setSaving(false);
    }
  };

  const handleKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      if (done()) {
        props.onClose();
        return;
      }
      props.onCancel?.();
      return;
    }
    if (e.key === "Tab" && modalRef) {
      const focusable = modalRef.querySelectorAll<HTMLElement>(
        'button:not(:disabled), input:not(:disabled), [tabindex]:not([tabindex="-1"])'
      );
      if (focusable.length === 0) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    }
  };

  let overlayRef!: HTMLDivElement;
  let modalRef!: HTMLDivElement;
  onMount(() => {
    overlayRef.focus();
    void codingAgentsStore.ensureLoaded();
  });

  return (
    <div
      class="modal-overlay"
      ref={overlayRef}
      onKeyDown={handleKeyDown}
      tabIndex={0}
      data-ac-testid="onboarding.overlay"
      data-ac-role="overlay"
    >
      <div
        class="agent-modal onboarding-modal"
        ref={modalRef}
        role="dialog"
        aria-modal="true"
        aria-label={props.ariaLabel ?? props.title}
        data-ac-testid="onboarding.modal"
        data-ac-role="dialog"
        data-ac-state={done() ? "done" : "selecting"}
      >
        <div class="agent-modal-header">
          <span class="agent-modal-title">{props.title}</span>
        </div>

        <div class="wizard-body onboarding-body">
          <Show when={!done()} fallback={
            <div
              class="onboarding-done"
              data-ac-testid="onboarding.done"
              data-ac-role="status"
            >
              <div class="onboarding-done-icon">&#x2713;</div>
              <div class="onboarding-done-text">
                <strong>{addedLabel()}</strong> configured!
              </div>
              <div class="onboarding-done-hint">
                You can add more Coding Agents later in Settings.
              </div>
            </div>
          }>
            <p class="onboarding-welcome">{props.message}</p>

            {/* #1965 — catalog status: never a silent fallback to embedded
                presets. Paths and reasons render as text. */}
            <Show when={codingAgentsStore.loading()}>
              <div
                class="settings-hint"
                data-ac-testid="onboarding.catalog.loading"
                data-ac-role="status"
              >
                Loading catalog…
              </div>
            </Show>
            <Show when={codingAgentsStore.error()}>
              {(diagnostic) => (
                <div
                  class="settings-hint settings-hint-warning"
                  data-ac-testid="onboarding.catalog.error"
                  data-ac-role="status"
                  data-ac-state={
                    diagnostic().code === "primary-project-changed"
                      ? "source-changed"
                      : "unavailable"
                  }
                >
                  <div>
                    {diagnostic().code === "primary-project-changed"
                      ? "Catalog source changed"
                      : "Catalog unavailable"}
                  </div>
                  <DiagnosticDetails
                    path={diagnostic().path}
                    reason={diagnostic().reason}
                    testIdPrefix="onboarding.catalog.error"
                  />
                </div>
              )}
            </Show>
            <For each={codingAgentsStore.warnings()}>
              {(warning, index) => (
                <div
                  class="settings-hint settings-hint-warning"
                  data-ac-testid={`onboarding.catalog.warning.${index()}`}
                  data-ac-role="status"
                >
                  <DiagnosticDetails
                    path={warning.path}
                    reason={warning.reason}
                    testIdPrefix={`onboarding.catalog.warning.${index()}`}
                  />
                </div>
              )}
            </For>
            <Show
              when={
                codingAgentsStore.error() ||
                codingAgentsStore.warnings().length > 0 ||
                (codingAgentsStore.loaded() && codingAgentsStore.catalog().length === 0)
              }
            >
              <button
                class="new-agent-cancel-btn"
                onClick={() => void codingAgentsStore.refresh()}
                data-ac-testid="onboarding.catalog.reload"
                data-ac-role="button"
              >
                Reload catalog
              </button>
            </Show>
            <Show
              when={
                codingAgentsStore.loaded() &&
                !codingAgentsStore.error() &&
                codingAgentsStore.catalog().length === 0
              }
            >
              <div
                class="settings-hint"
                data-ac-testid="onboarding.catalog.empty"
                data-ac-role="status"
              >
                No catalog agents available
              </div>
            </Show>

            <div class="onboarding-cards">
              <For each={allPresets()}>
                {(preset) => (
                  <button
                    class={`onboarding-card ${selectedPreset() === preset.key ? "selected" : ""}`}
                    onClick={() => handleSelect(preset.key)}
                    style={{ "--card-accent": preset.color }}
                    aria-pressed={selectedPreset() === preset.key}
                    aria-label={`Select ${preset.label}`}
                    data-ac-testid={`onboarding.agentPreset.${preset.key}`}
                    data-ac-role="agent-preset"
                    data-ac-state={selectedPreset() === preset.key ? "selected" : "idle"}
                    data-ac-agent-key={preset.key}
                  >
                    <div
                      class="onboarding-card-icon"
                      style={{ background: preset.color }}
                    >
                      {preset.label[0]}
                    </div>
                    <div class="onboarding-card-info">
                      <div class="onboarding-card-name">{preset.label}</div>
                      <div class="onboarding-card-desc">{preset.description}</div>
                    </div>
                  </button>
                )}
              </For>
            </div>

            <Show when={isCustom()}>
              <div class="onboarding-custom-fields">
                <label class="onboarding-field-label">
                  Agent name
                  <input
                    class="onboarding-field-input"
                    type="text"
                    placeholder="My Agent"
                    value={customLabel()}
                    onInput={(e) => setCustomLabel(e.currentTarget.value)}
                    data-ac-testid="onboarding.custom.label"
                    data-ac-role="textbox"
                  />
                </label>
                <label class="onboarding-field-label">
                  Command
                  <input
                    class="onboarding-field-input"
                    type="text"
                    placeholder="my-agent --flag"
                    value={customCommand()}
                    onInput={(e) => setCustomCommand(e.currentTarget.value)}
                    data-ac-testid="onboarding.custom.command"
                    data-ac-role="textbox"
                  />
                </label>
              </div>
            </Show>
          </Show>
        </div>

        <div class="new-agent-footer">
          <Show when={done()} fallback={
            <>
              <Show when={!!props.onCancel}>
                <button
                  class="new-agent-cancel-btn"
                  onClick={() => props.onCancel?.()}
                  data-ac-testid="onboarding.cancel"
                  data-ac-role="button"
                >
                  Cancel
                </button>
              </Show>
              <button
                class="new-agent-create-btn"
                disabled={!canConfirm() || saving()}
                onClick={handleConfirm}
                data-ac-testid="onboarding.confirm"
                data-ac-role="button"
                data-ac-state={saving() ? "saving" : canConfirm() ? "ready" : "disabled"}
              >
                {saving() ? "Setting up..." : "Set up Coding Agent"}
              </button>
            </>
          }>
            <button
              class="new-agent-create-btn"
              onClick={props.onClose}
              data-ac-testid="onboarding.done.close"
              data-ac-role="button"
            >
              Get started
            </button>
          </Show>
        </div>
      </div>
    </div>
  );
};

export default CodingAgentQuickConfiguration;
