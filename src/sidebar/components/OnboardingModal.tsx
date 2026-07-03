import { Component, createSignal, For, onMount, Show } from "solid-js";
import type { AgentConfig, AppSettings, CodingAgentDefinition } from "../../shared/types";
import { SettingsAPI } from "../../shared/ipc";
import { settingsStore } from "../../shared/stores/settings";
import { newAgentId, definitionToSeed } from "../../shared/agent-presets";
import { codingAgentsStore } from "../stores/coding-agents";

// Custom Agent is not a catalog entry: it triggers the inline name/command
// fields and is always offered last, so it stays a client-side definition.
const CUSTOM_PRESET: CodingAgentDefinition = {
  key: "custom",
  label: "Custom Agent",
  description: "Configure your own Coding Agent",
  color: "#6366f1",
  command: "",
  envs: [],
  isolatedHome: false,
  removable: false,
};

const OnboardingModal: Component<{ onClose: () => void }> = (props) => {
  const [selectedPreset, setSelectedPreset] = createSignal<string | null>(null);
  const [saving, setSaving] = createSignal(false);
  const [done, setDone] = createSignal(false);
  const [addedLabel, setAddedLabel] = createSignal("");

  // #769 — the catalog is backend-owned; the store is pre-seeded with the
  // fallback so the card list (and the #768 no-scroll fit) renders immediately,
  // with no wrong-length flash before the async fetch resolves.
  const allPresets = () => [...codingAgentsStore.catalog(), CUSTOM_PRESET];

  const dismissAndClose = async () => {
    try {
      const settings = await SettingsAPI.get();
      await SettingsAPI.update({ ...settings, onboardingDismissed: true });
      settingsStore.refresh();
    } catch {}
    props.onClose();
  };

  // Custom agent fields
  const [customLabel, setCustomLabel] = createSignal("");
  const [customCommand, setCustomCommand] = createSignal("");

  const isCustom = () => selectedPreset() === "custom";
  const canConfirm = () => {
    if (!selectedPreset()) return false;
    if (isCustom()) return customLabel().trim() !== "" && customCommand().trim() !== "";
    return true;
  };

  const handleSelect = (key: string) => {
    setSelectedPreset(key === selectedPreset() ? null : key);
  };

  const handleConfirm = async () => {
    const key = selectedPreset();
    if (!key) return;

    const preset = allPresets().find((p) => p.key === key);
    if (!preset) return;

    setSaving(true);
    try {
      const settings = await SettingsAPI.get();

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
        agent = { id: newAgentId(), ...definitionToSeed(preset) };
      }

      const updated: AppSettings = {
        ...settings,
        agents: [...settings.agents, agent],
        onboardingDismissed: true,
      };
      await SettingsAPI.update(updated);
      settingsStore.refresh();

      setAddedLabel(agent.label);
      setDone(true);
    } catch (e) {
      console.error("Onboarding save failed:", e);
    } finally {
      setSaving(false);
    }
  };

  const handleKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      if (done()) props.onClose();
      else void dismissAndClose();
      return;
    }
    // Focus trap: keep Tab cycling within the modal
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
        aria-label="Set up your first Coding Agent"
        data-ac-testid="onboarding.modal"
        data-ac-role="dialog"
        data-ac-state={done() ? "done" : "selecting"}
      >
        <div class="agent-modal-header">
          <span class="agent-modal-title">Welcome</span>
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
            <p class="onboarding-welcome">
              Welcome to AgentsCommander! Let's set up your first Coding Agent.
            </p>

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
              <button
                class="new-agent-cancel-btn"
                onClick={() => void dismissAndClose()}
                data-ac-testid="onboarding.skip"
                data-ac-role="button"
              >
                Skip
              </button>
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

export default OnboardingModal;
