import { Component, createSignal, createMemo, For, Show, onMount, createEffect } from "solid-js";
import type { AgentConfig, AppSettings, CodingAgentProfileResolution } from "../../shared/types";
import { SettingsAPI } from "../../shared/ipc";
import {
  agentNameFromPathOrSession,
  isAcAgentPath,
  normalizeProfileLetter,
  profileDisplayLabel,
  resolveProfilePreview,
  sortedProfileLetters,
  targetProfileFqn,
} from "../../shared/profile-utils";

export interface AgentPickerSelection {
  agent: AgentConfig;
  requestedProfile: string | null;
  effectiveProfile: string;
  scope: "default" | "instance";
}

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
    const current = settings();
    if (!current) return "A";
    return current.codingAgentProfiles.agentDefaults[targetName()] ?? "A";
  });
  const effectivePreview = createMemo(() => {
    const resolved = backendPreview();
    if (resolved) {
      return {
        requestedProfile: resolved.requestedProfile,
        effectiveProfile: resolved.effectiveProfile,
        fallbackChain: resolved.fallbackChain,
        fallbackApplied: resolved.fallbackApplied,
      };
    }
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

  onMount(async () => {
    overlayRef?.focus();
    const loaded = await SettingsAPI.get();
    setSettings(loaded);
    setAgents(loaded.agents);
    const agentIndex = loaded.agents
      .slice()
      .sort((a, b) => a.label.localeCompare(b.label, "en", { sensitivity: "base", numeric: true }))
      .findIndex((agent) => agent.id === props.currentAgentId);
    if (agentIndex >= 0) setHighlightIndex(agentIndex);
    const requested =
      normalizeProfileLetter(props.currentRequestedProfile) ??
      normalizeProfileLetter(loaded.codingAgentProfiles.agentDefaults[targetName()]) ??
      "A";
    setSelectedProfile(requested);
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
    return profileTouched() ? selectedProfile() : null;
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
    } else if (e.key === "Enter" && list.length > 0) {
      e.preventDefault();
      void commit("instance");
    }
  };

  return (
    <div ref={overlayRef} class="modal-overlay" tabIndex={0} onKeyDown={handleKeyDown}>
      <div class="agent-modal new-agent-modal">
        <div class="agent-modal-header">
          <span class="agent-modal-title">
            Launch <strong>{props.sessionName}</strong>
          </span>
        </div>
        <div class="agent-picker-summary">
          <div>
            Current default for <strong>{targetFqn()}</strong>:{" "}
            {settings()
              ? profileDisplayLabel(settings()!.codingAgentProfiles, configuredDefault())
              : configuredDefault()}
          </div>
          <Show when={selectedAgent()}>
            <div>
              Effective:{" "}
              <strong>
                {profileResolving()
                  ? "Resolving..."
                  : effectivePreview().fallbackApplied
                  ? `${effectivePreview().requestedProfile} -> ${effectivePreview().effectiveProfile}`
                  : effectivePreview().effectiveProfile}
              </strong>
            </div>
          </Show>
        </div>
        <div class="agent-modal-list agent-picker-list">
          <Show
            when={sortedAgents().length > 0}
            fallback={<div class="agent-modal-empty">No agents configured. Add agents in Settings.</div>}
          >
            <For each={sortedAgents()}>
              {(agent, i) => (
                <div
                  class={`agent-modal-item agent-choice ${i() === highlightIndex() ? "highlighted" : ""}`}
                  onClick={() => setHighlightIndex(i())}
                  onMouseEnter={() => setHighlightIndex(i())}
                >
                  <div
                    class="agent-color-badge"
                    style={{ background: agent.color }}
                  />
                  <div class="agent-modal-item-info">
                    <div class="agent-modal-item-name">{agent.label}</div>
                    <div class="agent-modal-item-detail">{agent.command}</div>
                  </div>
                </div>
              )}
            </For>
          </Show>
        </div>
        <div class="agent-profile-strip">
          <For each={profileLetters()}>
            {(letter) => (
              <button
                class="agent-profile-chip"
                classList={{ active: selectedProfile() === letter }}
                onClick={() => chooseProfile(letter)}
              >
                {settings()
                  ? profileDisplayLabel(settings()!.codingAgentProfiles, letter)
                  : letter}
              </button>
            )}
          </For>
        </div>
        <Show when={error()}>
          <div class="agent-picker-error">{error()}</div>
        </Show>
        <div class="agent-picker-actions">
          <button class="modal-btn modal-btn-cancel" disabled={busy()} onClick={props.onClose}>
            Cancel
          </button>
          <button
            class="modal-btn modal-btn-save"
            disabled={busy() || profileResolving() || !selectedAgent() || !canPersistProfileSelection()}
            onClick={() => commit("default")}
            title={!canPersistProfileSelection() ? "Default profiles require an AgentsCommander agent path" : undefined}
          >
            Set selected profile as new default for {targetFqn()}
          </button>
          <button
            class="modal-btn modal-btn-save"
            disabled={busy() || profileResolving() || !selectedAgent()}
            onClick={() => commit("instance")}
          >
            Set just for instance
          </button>
        </div>
        <div class="agent-modal-footer">
          <span>&#x2191;&#x2193; agent</span>
          <span>&#x2190;&#x2192; profile</span>
          <span>&#x23CE; instance</span>
          <span>esc close</span>
        </div>
      </div>
    </div>
  );
};

export default AgentPickerModal;
