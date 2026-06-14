import { Component, createSignal, createMemo, For, Show, onMount, createEffect } from "solid-js";
import type { AgentConfig, AppSettings, CodingAgentProfileResolution, ProfileCellConfig } from "../../shared/types";
import { SettingsAPI } from "../../shared/ipc";
import { automationAttrs } from "../../shared/automation-hooks";
import {
  agentNameFromPathOrSession,
  isAcAgentPath,
  isCodexAgent,
  normalizeProfileLetter,
  profileDisplayLabel,
  resolveProfilePreview,
  sortedProfileLetters,
  stringifyArgv,
  targetProfileFqn,
} from "../../shared/profile-utils";

export interface AgentPickerSelection {
  agent: AgentConfig;
  requestedProfile: string | null;
  effectiveProfile: string;
  scope: "default" | "instance";
}

const EMPTY_DISPLAY_CELL: ProfileCellConfig = {
  enabled: true,
  argv: [],
  env: {},
  notes: "",
};

const AgentPickerModal: Component<{
  sessionName: string;
  agentPath?: string | null;
  currentAgentId?: string | null;
  currentRequestedProfile?: string | null;
  onSelect: (selection: AgentPickerSelection) => void | Promise<void>;
  onClose: () => void;
}> = (props) => {
  const [settings, setSettings] = createSignal<AppSettings | null>(null);
  const [agents, setAgents] = createSignal<AgentConfig[]>([]);
  const [highlightIndex, setHighlightIndex] = createSignal(0);
  const [selectedProfile, setSelectedProfile] = createSignal("A");
  const [profileTouched, setProfileTouched] = createSignal(false);
  const [initialProfileShouldLaunch, setInitialProfileShouldLaunch] = createSignal(false);
  const [backendPreview, setBackendPreview] = createSignal<CodingAgentProfileResolution | null>(null);
  const [profileResolving, setProfileResolving] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  let overlayRef!: HTMLDivElement;
  let profileResolveSeq = 0;

  const sortedAgents = createMemo(() =>
    [...agents()].sort((a, b) =>
      a.label.localeCompare(b.label, "en", { sensitivity: "base", numeric: true })
    )
  );

  const selectedAgent = createMemo(() => sortedAgents()[highlightIndex()] ?? null);
  const profileLetters = createMemo(() =>
    settings() ? sortedProfileLetters(settings()!.codingAgentProfiles) : ["A"]
  );
  const targetName = createMemo(() =>
    agentNameFromPathOrSession(props.agentPath, props.sessionName)
  );
  const targetFqn = createMemo(() =>
    targetProfileFqn(props.agentPath, props.sessionName)
  );
  const canPersistProfileSelection = createMemo(() => isAcAgentPath(props.agentPath));
  const canUseBackendProfileResolution = createMemo(() => isAcAgentPath(props.agentPath));
  const configuredDefault = createMemo(() => {
    const resolved = backendPreview();
    const backendDefault = resolved?.originDefaultProfile ?? resolved?.agentDefaultProfile;
    if (backendDefault) return backendDefault;
    if (!canPersistProfileSelection()) return "A";
    const current = settings();
    if (!current) return "A";
    return normalizeProfileLetter(current.codingAgentProfiles.agentDefaults[targetName()]) ?? "A";
  });
  const profileLabel = (letter: string) =>
    settings() ? profileDisplayLabel(settings()!.codingAgentProfiles, letter) : letter;
  const profileCellFor = (agent: AgentConfig | null, letter: string) => {
    const current = settings();
    if (!current || !agent) return null;
    return current.codingAgentProfiles.matrix[agent.id]?.[letter] ?? null;
  };
  const enabledLaunchCellFor = (agent: AgentConfig | null, letter: string): ProfileCellConfig => {
    const cell = profileCellFor(agent, letter);
    return cell?.enabled ? cell : EMPTY_DISPLAY_CELL;
  };
  const isProfileConfiguredFor = (agent: AgentConfig | null, letter: string) => {
    if (letter === "A") return true;
    return Boolean(profileCellFor(agent, letter)?.enabled);
  };
  const localSelectionPreview = createMemo(() => {
    const current = settings();
    const agent = selectedAgent();
    if (!current || !agent) {
      return {
        requestedProfile: selectedProfile(),
        effectiveProfile: selectedProfile(),
        fallbackChain: [selectedProfile()],
        fallbackApplied: false,
      };
    }
    return resolveProfilePreview(
      current.codingAgentProfiles,
      agent.id,
      selectedProfile()
    );
  });
  const effectivePreview = createMemo(() => {
    if (profileTouched()) return localSelectionPreview();
    const resolved = backendPreview();
    if (resolved) {
      return {
        requestedProfile: resolved.requestedProfile,
        effectiveProfile: resolved.effectiveProfile,
        fallbackChain: resolved.fallbackChain,
        fallbackApplied: resolved.fallbackApplied,
      };
    }
    return localSelectionPreview();
  });
  const projectedCell = createMemo(() => {
    const agent = selectedAgent();
    if (!agent) return EMPTY_DISPLAY_CELL;
    return enabledLaunchCellFor(agent, effectivePreview().effectiveProfile);
  });
  const formattedArgv = (argv: string[]) => stringifyArgv(argv) || "none";
  const formattedEnv = (env: Record<string, string>) => {
    const enabled = Object.keys(env).sort((a, b) => a.localeCompare(b));
    return enabled.length ? enabled.join(", ") : "none";
  };
  const formattedAgentEnv = (agent: AgentConfig | null) => {
    const keys = (agent?.envs ?? [])
      .filter((row) => row.enabled)
      .map((row) => row.key.trim())
      .filter(Boolean)
      .sort((a, b) => a.localeCompare(b));
    return keys.length ? keys.join(", ") : "none";
  };
  const providerDefaultPreview = (agent: AgentConfig) => {
    const current = settings();
    if (!current) {
      return {
        requestedProfile: configuredDefault(),
        effectiveProfile: configuredDefault(),
        fallbackChain: [configuredDefault()],
        fallbackApplied: false,
      };
    }
    return resolveProfilePreview(
      current.codingAgentProfiles,
      agent.id,
      configuredDefault()
    );
  };
  const backendWarnings = createMemo(() => backendPreview()?.warnings ?? []);
  const hasBackendWarnings = createMemo(() => backendWarnings().length > 0);
  const selectedIsCodex = createMemo(() => {
    const agent = selectedAgent();
    return agent ? isCodexAgent(agent) : false;
  });

  onMount(async () => {
    overlayRef?.focus();
    const loaded = await SettingsAPI.get();
    const agentIndex = loaded.agents
      .slice()
      .sort((a, b) => a.label.localeCompare(b.label, "en", { sensitivity: "base", numeric: true }))
      .findIndex((agent) => agent.id === props.currentAgentId);
    if (agentIndex >= 0) setHighlightIndex(agentIndex);
    setSettings(loaded);
    setAgents(loaded.agents);
    const currentRequested = normalizeProfileLetter(props.currentRequestedProfile);
    const acDefault = isAcAgentPath(props.agentPath)
      ? normalizeProfileLetter(loaded.codingAgentProfiles.agentDefaults[targetName()])
      : null;
    const requested = currentRequested ?? acDefault ?? "A";
    setSelectedProfile(requested);
    setInitialProfileShouldLaunch(Boolean(currentRequested) || Boolean(acDefault));
  });

  createEffect(() => {
    const current = settings();
    const agent = selectedAgent();
    const agentPath = props.agentPath;
    if (!current || !agent || !agentPath || !canUseBackendProfileResolution()) {
      profileResolveSeq += 1;
      setBackendPreview(null);
      setProfileResolving(false);
      return;
    }

    const requested = profileTouched()
      ? selectedProfile()
      : normalizeProfileLetter(props.currentRequestedProfile);
    const seq = ++profileResolveSeq;
    setBackendPreview(null);
    setProfileResolving(true);
    SettingsAPI.resolveCodingAgentProfile(agentPath, agent.id, requested)
      .then((resolution) => {
        if (seq !== profileResolveSeq) return;
        setError("");
        setBackendPreview(resolution);
        if (!profileTouched()) {
          setSelectedProfile(resolution.requestedProfile);
        }
      })
      .catch((err: unknown) => {
        if (seq !== profileResolveSeq) return;
        setBackendPreview(null);
        setError(err instanceof Error ? err.message : String(err));
      })
      .finally(() => {
        if (seq === profileResolveSeq) setProfileResolving(false);
      });
  });

  const moveProfile = (delta: number) => {
    const letters = profileLetters();
    const current = Math.max(0, letters.indexOf(selectedProfile()));
    const next = Math.min(Math.max(current + delta, 0), letters.length - 1);
    setProfileTouched(true);
    setSelectedProfile(letters[next]);
  };

  const chooseProfile = (letter: string) => {
    setProfileTouched(true);
    setSelectedProfile(letter);
  };

  const requestedProfileForSelection = (): string | null => {
    if (canUseBackendProfileResolution()) return selectedProfile();
    return profileTouched() || initialProfileShouldLaunch() ? selectedProfile() : null;
  };

  const commit = async (scope: "default" | "instance") => {
    const agent = selectedAgent();
    if (!agent || busy() || profileResolving()) return;
    setBusy(true);
    setError("");
    try {
      if (props.agentPath && canPersistProfileSelection()) {
        if (scope === "default") {
          await SettingsAPI.setAgentDefaultProfile(props.agentPath, selectedProfile());
          await SettingsAPI.setInstanceProfileOverride(props.agentPath, null);
        } else {
          await SettingsAPI.setInstanceProfileOverride(props.agentPath, selectedProfile());
        }
      }
      await props.onSelect({
        agent,
        requestedProfile: requestedProfileForSelection(),
        effectiveProfile: effectivePreview().effectiveProfile,
        scope,
      });
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
      setBusy(false);
    }
  };

  const isInteractiveTarget = (target: EventTarget | null): boolean => {
    if (!(target instanceof HTMLElement)) return false;
    const interactive = target.closest(
      'button,input,select,textarea,a[href],[role="button"],[role="link"],[role="menuitem"],[tabindex]:not([tabindex="-1"])'
    );
    return Boolean(interactive && interactive !== overlayRef);
  };

  const handleKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      props.onClose();
      return;
    }
    const list = sortedAgents();
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setHighlightIndex((i) => Math.min(i + 1, list.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setHighlightIndex((i) => Math.max(i - 1, 0));
    } else if (e.key === "ArrowLeft") {
      e.preventDefault();
      moveProfile(-1);
    } else if (e.key === "ArrowRight") {
      e.preventDefault();
      moveProfile(1);
    } else if (e.key === "Enter" && list.length > 0 && !isInteractiveTarget(e.target)) {
      e.preventDefault();
      void commit("instance");
    }
  };

  return (
    <div
      ref={overlayRef}
      class="modal-overlay"
      tabIndex={0}
      onKeyDown={handleKeyDown}
      data-component="Coding Agent profile assignment modal overlay"
      {...automationAttrs("agentPicker.overlay", "overlay")}
    >
      <div
        class="agent-modal agent-picker-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="agentPickerTitle"
        data-component="Coding Agent profile assignment modal"
        {...automationAttrs("agentPicker.modal", "dialog")}
      >
        <div class="agent-modal-header agent-picker-modal-header" data-component="Coding Agent profile modal header">
          <div>
            <div class="agent-picker-eyebrow">Coding Agent</div>
            <span id="agentPickerTitle" class="agent-modal-title">
              Assign profile for <strong>{targetFqn()}</strong>
            </span>
          </div>
          <div class="agent-picker-context">
            <strong>{targetName()}</strong>
            <span>{props.sessionName}</span>
          </div>
        </div>

        <div class="agent-profile-assignment-body" data-component="Coding Agent profile modal variant C layout">
          <aside class="agent-profile-panel agent-profile-provider-panel" data-component="Coding Agents selector panel">
            <div class="agent-profile-panel-title">Coding Agents</div>
            <div
              class="agent-profile-provider-list"
              aria-label="Coding agent choices"
              data-component="Coding agent selector"
              {...automationAttrs("agentPicker.providers", "list")}
            >
              <Show
                when={sortedAgents().length > 0}
                fallback={<div class="agent-modal-empty">No agents configured. Add agents in Settings.</div>}
              >
                <For each={sortedAgents()}>
                  {(agent, i) => {
                    const defaultPreview = () => providerDefaultPreview(agent);
                    const active = () => i() === highlightIndex();
                    return (
                      <button
                        type="button"
                        class="agent-profile-provider-card"
                        classList={{ active: active() }}
                        aria-pressed={active()}
                        onClick={() => setHighlightIndex(i())}
                        onMouseEnter={() => setHighlightIndex(i())}
                        data-component={`${agent.label} coding agent option`}
                        data-ac-agent-id={agent.id}
                        data-ac-agent-command={agent.command}
                        data-ac-effective-profile={defaultPreview().effectiveProfile}
                        data-ac-requested-profile={defaultPreview().requestedProfile}
                        style={{ "--agent-color": agent.color }}
                        {...automationAttrs(`agentPicker.provider.${agent.id}`, "button", active() ? "active" : "inactive")}
                      >
                        <span>
                          <span class="agent-profile-provider-name">{agent.label}</span>
                          <span class="agent-profile-provider-command">{agent.command}</span>
                        </span>
                        <span class="agent-profile-provider-chip">
                          {defaultPreview().fallbackApplied
                            ? `${defaultPreview().requestedProfile}->${defaultPreview().effectiveProfile}`
                            : profileLabel(defaultPreview().effectiveProfile)}
                        </span>
                      </button>
                    );
                  }}
                </For>
              </Show>
            </div>
          </aside>

          <div class="agent-profile-assignment-scroll" data-component="Coding Agent profile selector independent scroll area">
            <section class="agent-profile-panel" data-component="Coding Agent profile selector panel">
              <div>
                <div class="agent-profile-panel-title">Profiles</div>
                <div class="agent-profile-panel-kicker">Profiles resolve for the selected coding agent only.</div>
              </div>
              <div
                class="agent-profile-card-list"
                data-component="Selected Coding Agent available profile cards"
                {...automationAttrs("agentPicker.profiles", "list")}
              >
                <For each={profileLetters()}>
                  {(letter) => {
                    const configured = () => isProfileConfiguredFor(selectedAgent(), letter);
                    const selected = () => selectedProfile() === letter;
                    const preview = () =>
                      settings() && selectedAgent()
                        ? resolveProfilePreview(settings()!.codingAgentProfiles, selectedAgent()!.id, letter)
                        : {
                            requestedProfile: letter,
                            effectiveProfile: letter,
                            fallbackChain: [letter],
                            fallbackApplied: false,
                          };
                    const cell = () => enabledLaunchCellFor(selectedAgent(), preview().effectiveProfile);
                    return (
                      <button
                        type="button"
                        class="agent-profile-card"
                        classList={{
                          active: selected(),
                          missing: !configured(),
                          default: configuredDefault() === letter,
                        }}
                        aria-pressed={selected()}
                        onClick={() => chooseProfile(letter)}
                        data-component={`${selectedAgent()?.label ?? "Coding Agent"} ${profileLabel(letter)} profile selector card`}
                        data-ac-agent-id={selectedAgent()?.id}
                        data-ac-profile-letter={letter}
                        data-ac-effective-profile={preview().effectiveProfile}
                        data-ac-configured={configured()}
                        {...automationAttrs(
                          `agentPicker.profile.${letter}`,
                          "button",
                          selected()
                            ? "active"
                            : !configured()
                            ? "missing"
                            : configuredDefault() === letter
                            ? "default"
                            : "available"
                        )}
                      >
                        <span class="agent-profile-card-head">
                          <span>
                            <span class="agent-profile-card-title">{profileLabel(letter)}</span>
                            <span class="agent-profile-card-subtitle">
                              {configured()
                                ? "configured for selected coding agent"
                                : `missing; launches ${profileLabel(preview().effectiveProfile)}`}
                            </span>
                          </span>
                          <Show when={configuredDefault() === letter}>
                            <span class="agent-profile-default-marker">Default</span>
                          </Show>
                        </span>
                        <span class="agent-profile-param-list">
                          <span class="agent-profile-param">
                            <span>Profile args </span>
                            <span>{formattedArgv(cell().argv)}</span>
                          </span>
                          <Show when={!configured()}>
                            <span class="agent-profile-token warn">
                              Fallback {letter}-&gt;{preview().effectiveProfile}
                            </span>
                          </Show>
                        </span>
                      </button>
                    );
                  }}
                </For>
              </div>
            </section>

            <section
              class="agent-profile-panel"
              data-component="Selected profile projected parameters panel"
              {...automationAttrs("agentPicker.projected", "status")}
            >
              <div class="agent-profile-panel-title">Projected parameters</div>
              <div
                class="agent-profile-resolution-status"
                data-component="Coding Agent profile requested and resolved status"
              >
                <strong>Default: {profileLabel(configuredDefault())}</strong>
                <span>Selected coding agent: {selectedAgent()?.label ?? "none"}</span>
                <span>
                  Requested {profileLabel(effectivePreview().requestedProfile)} resolves to{" "}
                  {profileLabel(effectivePreview().effectiveProfile)}
                  {effectivePreview().fallbackApplied ? " via fallback" : " as configured"}.
                </span>
              </div>
              <div class="agent-profile-active-summary" data-component="Selected profile launch parameter summary">
                <div class="agent-profile-active-title">
                  {selectedAgent()?.label ?? "Coding Agent"} / {profileLabel(selectedProfile())}
                </div>
                <div class="agent-profile-param-list">
                  <div class="agent-profile-param"><span>Command </span><span>{selectedAgent()?.command ?? "none"}</span></div>
                  <div class="agent-profile-param"><span>Agent env </span><span>{formattedAgentEnv(selectedAgent())}</span></div>
                  <Show when={selectedIsCodex() || selectedAgent()?.isolateCodexHome}>
                    <div class="agent-profile-param"><span>Codex home isolation </span><span>{selectedAgent()?.isolateCodexHome ? "enabled" : "disabled"}</span></div>
                  </Show>
                  <div class="agent-profile-param"><span>Effective profile </span><span>{profileLabel(effectivePreview().effectiveProfile)}</span></div>
                  <div class="agent-profile-param"><span>Fallback chain </span><span>{effectivePreview().fallbackChain.join(" -> ") || "none"}</span></div>
                  <div class="agent-profile-param"><span>Profile args </span><span>{formattedArgv(projectedCell().argv)}</span></div>
                  <div class="agent-profile-param"><span>Profile env </span><span>{formattedEnv(projectedCell().env)}</span></div>
                  <Show when={projectedCell().notes}>
                    <div class="agent-profile-param"><span>Notes </span><span>{projectedCell().notes}</span></div>
                  </Show>
                </div>
              </div>
              <div
                class="agent-profile-warning-strip"
                classList={{ visible: effectivePreview().fallbackApplied || hasBackendWarnings() }}
                data-component="Coding Agent profile fallback explanation"
                {...automationAttrs(
                  "agentPicker.fallback",
                  "status",
                  effectivePreview().fallbackApplied || hasBackendWarnings() ? "warning" : "neutral"
                )}
              >
                <span>
                  {effectivePreview().fallbackApplied
                    ? `${profileLabel(effectivePreview().requestedProfile)} is not configured for ${selectedAgent()?.label ?? "the selected coding agent"}; launch resolves through ${profileLabel(effectivePreview().effectiveProfile)}. A remains the final fallback.`
                    : `${selectedAgent()?.label ?? "Selected coding agent"} launches with configured ${profileLabel(selectedProfile())} parameters.`}
                </span>
                <Show when={hasBackendWarnings()}>
                  <span>Profile warning: {backendWarnings().join(" ")}</span>
                </Show>
              </div>
            </section>
          </div>
        </div>

        <Show when={error()}>
          <div class="agent-picker-error">{error()}</div>
        </Show>

        <div class="agent-picker-actions">
          <button
            class="modal-btn modal-btn-cancel"
            disabled={busy()}
            onClick={props.onClose}
            {...automationAttrs("agentPicker.cancel", "button")}
          >
            Cancel
          </button>
          <button
            class="modal-btn modal-btn-save"
            disabled={busy() || profileResolving() || !selectedAgent() || !canPersistProfileSelection()}
            onClick={() => commit("default")}
            title={!canPersistProfileSelection() ? "Default profiles require an AgentsCommander agent path" : undefined}
            {...automationAttrs("agentPicker.setDefault", "button", canPersistProfileSelection() ? "enabled" : "disabled")}
          >
            Set selected profile as new default for {targetFqn()}
          </button>
          <button
            class="modal-btn modal-btn-save"
            disabled={busy() || profileResolving() || !selectedAgent()}
            onClick={() => commit("instance")}
            {...automationAttrs("agentPicker.setInstance", "button")}
          >
            Set just for instance
          </button>
        </div>
      </div>
    </div>
  );
};

export default AgentPickerModal;
