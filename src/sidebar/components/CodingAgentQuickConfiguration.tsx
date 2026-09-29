import { Component, createEffect, createSignal, For, onCleanup, onMount, Show } from "solid-js";
import type {
  AgentConfig,
  AppSettings,
  CodingAgentDefinition,
  CodingAgentInstallFinished,
  CodingAgentTestedLevel,
  CodingAgentWelcomeStatus,
} from "../../shared/types";
import { CodingAgentsAPI, onCodingAgentInstallFinished, SettingsAPI } from "../../shared/ipc";
import type { UnlistenFn } from "../../shared/transport";
import { settingsStore } from "../../shared/stores/settings";
import { toastStore } from "../../shared/stores/toasts";
import { newAgentId, definitionToSeed, sortWelcomeAgents } from "../../shared/agent-presets";
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
  /** #2736 - Welcome screen only: status and tested chips plus Welcome order. */
  showInstallStatus?: boolean;
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

  const [welcomeStatus, setWelcomeStatus] = createSignal<CodingAgentWelcomeStatus[]>([]);

  const allPresets = () =>
    props.showInstallStatus
      ? [...sortWelcomeAgents(codingAgentsStore.catalog(), welcomeStatus()), CUSTOM_PRESET]
      : [...codingAgentsStore.catalog(), CUSTOM_PRESET];

  // #2736 - the status rows belong to the catalog generation they were asked
  // in; a response for a superseded generation is discarded. A failure is not
  // fatal: no chips beyond "Not installed" and catalog order.
  const loadWelcomeStatus = async (
    beforePublish?: (rows: CodingAgentWelcomeStatus[]) => (() => void) | void
  ) => {
    const generation = codingAgentsStore.generation();
    try {
      const rows = await CodingAgentsAPI.welcomeStatus();
      if (generation !== codingAgentsStore.generation()) return;
      const afterPublish = beforePublish?.(rows);
      setWelcomeStatus(rows);
      afterPublish?.();
    } catch (e) {
      if (generation !== codingAgentsStore.generation()) return;
      console.error("Coding Agent welcome status failed:", e);
      setWelcomeStatus([]);
    }
  };

  const statusRowOf = (key: string) => welcomeStatus().find((row) => row.key === key);
  const statusLabel = (key: string): string => {
    if (key === CUSTOM_PRESET.key) return "Not needed";
    return statusRowOf(key)?.installed ? "Installed" : "Not installed";
  };
  const statusState = (key: string): string => {
    if (key === CUSTOM_PRESET.key) return "not-needed";
    return statusRowOf(key)?.installed ? "installed" : "missing";
  };
  const testedLevelOf = (key: string): CodingAgentTestedLevel | null => {
    if (key === CUSTOM_PRESET.key) return null;
    return statusRowOf(key)?.testedLevel ?? null;
  };
  const testedLabel = (level: CodingAgentTestedLevel): string =>
    level === "high" ? "High" : level === "medium" ? "Medium" : "Low";
  const presetAriaLabel = (preset: CodingAgentDefinition): string => {
    if (!props.showInstallStatus) return `Select ${preset.label}`;
    const level = testedLevelOf(preset.key);
    const tested = level ? `, Tested: ${testedLabel(level)}` : "";
    return `Select ${preset.label}, ${statusLabel(preset.key)}${tested}`;
  };

  // #2736 - silent install. Each update publishes a NEW Set so Solid reacts.
  // Install is gated on the install-finished listener being registered, so a
  // fast completion event can never be lost and strand a row in Installing.
  const [installing, setInstalling] = createSignal<Set<string>>(new Set());
  const [installFailed, setInstallFailed] = createSignal<Set<string>>(new Set());
  const [installReady, setInstallReady] = createSignal(false);
  const [installBlocked, setInstallBlocked] = createSignal(false);
  const withKey = (set: Set<string>, key: string) => new Set(set).add(key);
  const withoutKey = (set: Set<string>, key: string) => {
    const next = new Set(set);
    next.delete(key);
    return next;
  };

  const installCommandOf = (key: string): string | null =>
    statusRowOf(key)?.installCommand ?? null;
  const showInstallRow = (key: string): boolean =>
    !!props.showInstallStatus &&
    !statusRowOf(key)?.installed &&
    installCommandOf(key) !== null &&
    key !== CUSTOM_PRESET.key;

  const copyInstallCommand = async (key: string) => {
    const command = installCommandOf(key);
    if (command === null) return;
    try {
      // The accessor itself throws in a non-secure context (#2135).
      await navigator.clipboard.writeText(command);
      toastStore.success("Command copied", { tag: "coding-agent-install-copy" });
    } catch (e) {
      console.error("Coding Agent install command copy failed:", e);
    }
  };

  const runInstall = async (key: string) => {
    if (!installReady() || installing().has(key)) return;
    setInstallFailed((set) => withoutKey(set, key));
    setInstalling((set) => withKey(set, key));
    try {
      // Fire-and-forget: the install-finished event ends the run.
      await CodingAgentsAPI.install(key);
    } catch (e) {
      console.error("Coding Agent install failed to start:", e);
      setInstallFailed((set) => withKey(set, key));
      setInstalling((set) => withoutKey(set, key));
    }
  };

  const handleInstallFinished = (payload: CodingAgentInstallFinished) => {
    setInstalling((set) => withoutKey(set, payload.key));
    if (!payload.ok) setInstallFailed((set) => withKey(set, payload.key));
    // Presence, not `ok`, turns the row Installed. When the focused install row
    // is about to unmount, refocus its card AFTER the publish: the re-sort moves
    // the row node, and a move drops focus set before it.
    void loadWelcomeStatus((rows) => {
      if (!rows.find((row) => row.key === payload.key)?.installed) return;
      const card = modalRef?.querySelector<HTMLElement>(
        `[data-ac-testid="onboarding.agentPreset.${payload.key}"]`
      );
      const installRow = card?.parentElement?.querySelector(".onboarding-card-install");
      if (card && installRow?.contains(document.activeElement)) return () => card.focus();
    });
  };

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
    if (props.showInstallStatus) void loadWelcomeStatus();
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
    if (props.showInstallStatus) void loadWelcomeStatus();
  });

  // #2736 - the listen promise may resolve after unmount; the disposed flag
  // makes sure that late unlisten still runs.
  let disposed = false;
  let unlisten: UnlistenFn | null = null;
  onMount(() => {
    if (!props.showInstallStatus) return;
    onCodingAgentInstallFinished(handleInstallFinished)
      .then((fn) => {
        unlisten = fn;
        if (disposed) {
          fn();
          return;
        }
        setInstallReady(true);
      })
      .catch((e) => {
        console.error("Coding Agent install listener registration failed:", e);
        setInstallBlocked(true);
      });
  });
  onCleanup(() => {
    disposed = true;
    unlisten?.();
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
                  <div class="onboarding-card-row">
                    <button
                      class={`onboarding-card ${selectedPreset() === preset.key ? "selected" : ""}`}
                      onClick={() => handleSelect(preset.key)}
                      style={{ "--card-accent": preset.color }}
                      aria-pressed={selectedPreset() === preset.key}
                      aria-label={presetAriaLabel(preset)}
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
                        <Show when={props.showInstallStatus}>
                          <div class="onboarding-card-chips">
                            <span
                              class="onboarding-chip onboarding-chip-status"
                              data-ac-testid={`onboarding.agentPreset.${preset.key}.status`}
                              data-ac-role="status"
                              data-ac-state={statusState(preset.key)}
                            >
                              {statusLabel(preset.key)}
                            </span>
                            <Show when={testedLevelOf(preset.key)}>
                              {(level) => (
                                <span
                                  class="onboarding-chip onboarding-chip-tested"
                                  data-ac-testid={`onboarding.agentPreset.${preset.key}.tested`}
                                  data-ac-role="status"
                                  data-ac-state={level()}
                                >
                                  {`Tested: ${testedLabel(level())}`}
                                </span>
                              )}
                            </Show>
                          </div>
                        </Show>
                        <div class="onboarding-card-desc">{preset.description}</div>
                      </div>
                    </button>
                    <Show when={showInstallRow(preset.key)}>
                      <div class="onboarding-card-install">
                        <code
                          class="onboarding-install-command"
                          data-ac-testid={`onboarding.agentPreset.${preset.key}.installCommand`}
                        >
                          {installCommandOf(preset.key)}
                        </code>
                        <button
                          class="onboarding-install-copy"
                          type="button"
                          title="Copy"
                          aria-label={`Copy the install command for ${preset.label}`}
                          data-ac-testid={`onboarding.agentPreset.${preset.key}.copy`}
                          data-ac-role="button"
                          onClick={() => void copyInstallCommand(preset.key)}
                        >
                          <svg
                            class="onboarding-install-copy-icon"
                            viewBox="0 0 16 16"
                            aria-hidden="true"
                            fill="none"
                            stroke="currentColor"
                            stroke-width="1.3"
                          >
                            <rect x="5.5" y="5.5" width="8" height="8" rx="1.5" />
                            <path d="M10.5 3.5v-.5a1.5 1.5 0 0 0-1.5-1.5H4A1.5 1.5 0 0 0 2.5 3v5A1.5 1.5 0 0 0 4 9.5h.5" />
                          </svg>
                        </button>
                        <button
                          class="onboarding-install-run"
                          type="button"
                          aria-label={`Install ${preset.label}`}
                          aria-disabled={installing().has(preset.key) || !installReady()}
                          data-ac-testid={`onboarding.agentPreset.${preset.key}.install`}
                          data-ac-role="button"
                          data-ac-state={
                            installing().has(preset.key)
                              ? "installing"
                              : installBlocked()
                                ? "blocked"
                                : installReady()
                                  ? "idle"
                                  : "pending"
                          }
                          onClick={() => void runInstall(preset.key)}
                        >
                          {installing().has(preset.key) ? "Installing..." : "Install"}
                        </button>
                        <Show when={installFailed().has(preset.key) || installBlocked()}>
                          <span
                            class="settings-hint settings-hint-warning"
                            data-ac-testid={`onboarding.agentPreset.${preset.key}.installFailed`}
                            data-ac-role="status"
                          >
                            {installBlocked()
                              ? "Install is unavailable; see the app log."
                              : "Install failed; see the app log."}
                          </span>
                        </Show>
                      </div>
                    </Show>
                  </div>
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
