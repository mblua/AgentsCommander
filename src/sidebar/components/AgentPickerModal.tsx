import { Component, createSignal, createMemo, For, Show, onMount, onCleanup, createEffect } from "solid-js";
import type {
  AgentConfig,
  AppSettings,
  CodingAgentProfileResolution,
  ProfileCellConfig,
  ProfileAssignmentScope,
  ProfileAssignmentError,
  PreviewCodingAgentProfileSelectionResult,
} from "../../shared/types";
import { SettingsAPI } from "../../shared/ipc";
import { launchErrorMessage } from "../../shared/launch-errors";
import { automationAttrs } from "../../shared/automation-hooks";
import {
  agentNameFromPathOrSession,
  composeEffectiveCommand,
  expandAcPlaceholdersPreview,
  isAcAgentPath,
  isWgReplicaPath,
  normalizeProfileLetter,
  profileBadgeKind,
  type ProfileBadgeKind,
  profileCellCommandText,
  profileDisplayLabel,
  profileEnvOrigin,
  resolveProfilePreview,
  sortedProfileLetters,
  targetProfileFqn,
} from "../../shared/profile-utils";

export type AgentPickerScopeContext = {
  workgroupPath?: string;
  workgroupName?: string;
  targetReplicaPath?: string;
  targetReplicaName?: string;
  currentCodingAgentId?: string | null;
  currentProfile?: string | null;
};

export interface AgentPickerSelection {
  agent: AgentConfig;
  requestedProfile: string | null;
  effectiveProfile: string;
  scope: ProfileAssignmentScope;
  restartSessions: boolean;
  updatedCount?: number;
  restartedCount?: number;
}

const EMPTY_DISPLAY_CELL: ProfileCellConfig = {
  enabled: true,
  command: "",
  env: {},
  notes: "",
};

const SELECTION_PILL_LABEL: Record<Exclude<ProfileBadgeKind, "invalid">, string> = {
  match: "MATCH",
  configured: "CONFIGURED",
  fallback: "FALLBACK",
  missing: "MISSING",
};

const REDUNDANT_REPLICA_ASSIGN_TOOLTIP =
  "This replica already uses this Coding Agent + Profile.";

const AgentPickerModal: Component<{
  sessionName: string;
  agentPath?: string | null;
  currentAgentId?: string | null;
  explicitCurrentAgentId?: string | null;
  currentRequestedProfile?: string | null;
  scopeContext?: AgentPickerScopeContext;
  disableRedundantReplicaAssign?: boolean;
  targetProfileOutdated?: boolean;
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

  const [selectedScope, setSelectedScope] = createSignal<ProfileAssignmentScope>("replica");
  const [restartSessions, setRestartSessions] = createSignal(false);
  const [dangerArmed, setDangerArmed] = createSignal(false);
  const emptyScopePreviews: Record<ProfileAssignmentScope, PreviewCodingAgentProfileSelectionResult | null> = {
    replica: null,
    kind: null,
    workgroup: null,
  };
  const emptyScopeBusy: Record<ProfileAssignmentScope, boolean> = {
    replica: false,
    kind: false,
    workgroup: false,
  };
  const emptyScopeErrors: Record<ProfileAssignmentScope, string> = {
    replica: "",
    kind: "",
    workgroup: "",
  };
  const [scopePreviews, setScopePreviews] = createSignal<Record<ProfileAssignmentScope, PreviewCodingAgentProfileSelectionResult | null>>({ ...emptyScopePreviews });
  const [scopePreviewBusyMap, setScopePreviewBusyMap] = createSignal<Record<ProfileAssignmentScope, boolean>>({ ...emptyScopeBusy });
  const [scopePreviewErrorMap, setScopePreviewErrorMap] = createSignal<Record<ProfileAssignmentScope, string>>({ ...emptyScopeErrors });
  const scopePreview = createMemo(() => scopePreviews()[selectedScope()]);
  const scopePreviewBusy = createMemo(() => scopePreviewBusyMap()[selectedScope()]);
  const scopePreviewError = createMemo(() => scopePreviewErrorMap()[selectedScope()]);
  const [applyErrors, setApplyErrors] = createSignal<ProfileAssignmentError[]>([]);
  const [toastMsg, setToastMsg] = createSignal<string | null>(null);

  let overlayRef!: HTMLDivElement;
  let profileResolveSeq = 0;
  let previewSeqByScope: Record<ProfileAssignmentScope, number> = { replica: 0, kind: 0, workgroup: 0 };
  let toastTimer: ReturnType<typeof setTimeout> | null = null;

  const showToast = (message: string) => {
    if (toastTimer) clearTimeout(toastTimer);
    setToastMsg(message);
    toastTimer = setTimeout(() => {
      setToastMsg(null);
      toastTimer = null;
    }, 3000);
  };

  onCleanup(() => {
    if (toastTimer) clearTimeout(toastTimer);
  });

  const sortedAgents = createMemo(() =>
    [...agents()].sort((a, b) =>
      a.label.localeCompare(b.label, "en", { sensitivity: "base", numeric: true })
    )
  );

  const selectedAgent = createMemo(() => sortedAgents()[highlightIndex()] ?? null);
  const profileLetters = createMemo(() =>
    settings() ? sortedProfileLetters(settings()!.codingAgentProfiles) : ["A"]
  );
  const targetReplicaPath = createMemo(
    () => props.scopeContext?.targetReplicaPath ?? props.agentPath ?? null
  );
  const targetName = createMemo(() =>
    agentNameFromPathOrSession(targetReplicaPath(), props.sessionName)
  );
  const targetFqn = createMemo(() =>
    targetProfileFqn(targetReplicaPath(), props.sessionName)
  );
  const isWgReplica = createMemo(() => isWgReplicaPath(targetReplicaPath()));
  const showBroadScope = createMemo(
    () => Boolean(props.scopeContext?.workgroupPath) && isWgReplica()
  );
  const canPersistProfileSelection = createMemo(() => isAcAgentPath(targetReplicaPath()));
  const canUseBackendProfileResolution = createMemo(() => isAcAgentPath(targetReplicaPath()));
  const acRoot = createMemo(() => targetReplicaPath());

  const configuredDefault = createMemo(() => {
    const resolved = backendPreview();
    const backendDefault = resolved?.originDefaultProfile ?? resolved?.agentDefaultProfile;
    if (backendDefault) return backendDefault;
    if (!canPersistProfileSelection()) return "A";
    const current = settings();
    if (!current) return "A";
    return normalizeProfileLetter(current.codingAgentProfiles.defaultProfileByAgent[targetName()]) ?? "A";
  });
  const profileLabel = (letter: string, agentId: string | null | undefined = selectedAgent()?.id) => {
    const current = settings();
    return current
      ? profileDisplayLabel(current.codingAgentProfiles, current.agents, agentId, letter)
      : letter;
  };
  const profileCellFor = (agent: AgentConfig | null, letter: string) => {
    const current = settings();
    if (!current || !agent) return null;
    return current.codingAgentProfiles.profilesByAgent[agent.id]?.[letter] ?? null;
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
  const profileEnvEntries = (cell: ProfileCellConfig | null) =>
    Object.entries(cell?.enabled ? cell.env : {})
      .filter(([key]) => key.trim().length > 0)
      .sort(([a], [b]) => a.localeCompare(b, "en", { sensitivity: "base" }))
      .map(([key, value]) => ({
        key,
        value: expandAcPlaceholdersPreview(value, acRoot()),
        origin: profileEnvOrigin(key, value),
      }));
  const declaredProfileEnv = (agent: AgentConfig | null, letter: string) =>
    profileEnvEntries(profileCellFor(agent, letter));
  const comparisonResolutionText = (
    agentId: string,
    preview: ReturnType<typeof resolveProfilePreview>,
  ) =>
    preview.fallbackApplied
      ? `${profileLabel(preview.requestedProfile, agentId)} → ${profileLabel(preview.effectiveProfile, agentId)} (fallback)`
      : `${profileLabel(preview.requestedProfile, agentId)} direct`;
  const comparisonStatusLabel = (status: string) =>
    status === "direct" ? "direct" : status === "fallback" ? "fallback" : "missing";
  const comparisonRows = createMemo(() => {
    const current = settings();
    if (!current) return [];
    return sortedAgents().map((agent, index) => {
      const preview = resolveProfilePreview(current.codingAgentProfiles, agent.id, selectedProfile());
      const cell = enabledLaunchCellFor(agent, preview.effectiveProfile);
      const command = expandAcPlaceholdersPreview(
        composeEffectiveCommand(agent.command, profileCellCommandText(cell)),
        acRoot(),
      );
      const status = command.trim().length === 0
        ? "missing"
        : preview.fallbackApplied
        ? "fallback"
        : "direct";
      return {
        agent,
        index,
        preview,
        status,
        active: index === highlightIndex(),
      };
    });
  });
  const comparisonSummary = createMemo(() => ({
    direct: comparisonRows().filter((row) => row.status === "direct").length,
    fallback: comparisonRows().filter((row) => row.status === "fallback").length,
    missing: comparisonRows().filter((row) => row.status === "missing").length,
  }));
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
    const acDefault = isAcAgentPath(targetReplicaPath())
      ? normalizeProfileLetter(loaded.codingAgentProfiles.defaultProfileByAgent[targetName()])
      : null;
    const requested = currentRequested ?? acDefault ?? "A";
    setSelectedProfile(requested);
    setInitialProfileShouldLaunch(Boolean(currentRequested) || Boolean(acDefault));
  });

  createEffect(() => {
    const current = settings();
    const agent = selectedAgent();
    const agentPath = targetReplicaPath();
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

  const runScopePreview = (scope: ProfileAssignmentScope, agentId: string, profile: string) => {
    const target = targetReplicaPath();
    if (!target || !isWgReplica()) return;
    const seq = ++previewSeqByScope[scope];
    setScopePreviewBusyMap((prev) => ({ ...prev, [scope]: true }));
    setScopePreviewErrorMap((prev) => ({ ...prev, [scope]: "" }));
    SettingsAPI.previewCodingAgentProfileSelection({
      targetReplicaPath: target,
      codingAgentId: agentId,
      profile,
      scope,
      restartSessions: restartSessions(),
    })
      .then((result) => {
        if (seq !== previewSeqByScope[scope]) return;
        setScopePreviews((prev) => ({ ...prev, [scope]: result }));
      })
      .catch((err: unknown) => {
        if (seq !== previewSeqByScope[scope]) return;
        setScopePreviews((prev) => ({ ...prev, [scope]: null }));
        setScopePreviewErrorMap((prev) => ({ ...prev, [scope]: err instanceof Error ? err.message : String(err) }));
      })
      .finally(() => {
        if (seq === previewSeqByScope[scope]) setScopePreviewBusyMap((prev) => ({ ...prev, [scope]: false }));
      });
  };

  createEffect(() => {
    const scope = selectedScope();
    const agent = selectedAgent();
    const profile = selectedProfile();
    restartSessions();
    targetReplicaPath();

    previewSeqByScope.replica += 1;
    previewSeqByScope.kind += 1;
    previewSeqByScope.workgroup += 1;
    setDangerArmed(false);
    setScopePreviews({ ...emptyScopePreviews });
    setScopePreviewErrorMap({ ...emptyScopeErrors });
    setApplyErrors([]);
    setScopePreviewBusyMap({ ...emptyScopeBusy });

    if (!agent || !isWgReplica()) return;
    runScopePreview("replica", agent.id, profile);
    runScopePreview("kind", agent.id, profile);
    runScopePreview("workgroup", agent.id, profile);
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

  const scopeCount = (scope: ProfileAssignmentScope): number => {
    if (scope === "replica") return 1;
    return scopePreviews()[scope]?.targetCount ?? 0;
  };

  const scopeReplicaNoun = (count: number) => count === 1 ? "replica" : "replicas";

  const distinctWorkgroupCount = createMemo(() => {
    const targets = scopePreview()?.targets ?? [];
    return new Set(targets.map((t) => t.workgroupName)).size;
  });

  const confirmationLabel = createMemo(() => {
    const scope = selectedScope();
    const count = scopeCount(scope);
    const noun = scopeReplicaNoun(count);
    if (scope === "kind") return `I understand this overwrites ${count} ${noun} of this kind`;
    return `I understand this overwrites ${count} ${noun}`;
  });

  const applyLabel = createMemo(() => {
    const scope = selectedScope();
    if (scope === "replica") return "Assign to this replica";
    const count = scopePreview()?.targetCount ?? 0;
    if (scope === "kind") return `Overwrite ${count} of this kind`;
    const wg = props.scopeContext?.workgroupName;
    return `Overwrite ${count}${wg ? ` in ${wg}` : " in this workgroup"}`;
  });

  const currentProfileLetter = createMemo(() => {
    const preview = backendPreview();
    const override = normalizeProfileLetter(preview?.instanceProfileOverride);
    if (override) return override;
    const explicit = normalizeProfileLetter(props.currentRequestedProfile);
    if (explicit) return explicit;
    const originDefault = normalizeProfileLetter(preview?.originDefaultProfile);
    if (originDefault) return originDefault;
    const agentDefault = normalizeProfileLetter(preview?.agentDefaultProfile);
    if (agentDefault) return agentDefault;
    const current = settings();
    if (current && isAcAgentPath(targetReplicaPath())) {
      const acDefault = normalizeProfileLetter(
        current.codingAgentProfiles.defaultProfileByAgent[targetName()],
      );
      if (acDefault) return acDefault;
    }
    return "A";
  });

  const isRedundantReplicaSelection = createMemo(() => {
    if (!props.disableRedundantReplicaAssign) return false;
    if (selectedScope() !== "replica") return false;
    if (props.targetProfileOutdated) return false;
    const agent = selectedAgent();
    const baselineAgentId = props.explicitCurrentAgentId;
    if (!agent || !baselineAgentId) return false;
    if (agent.id !== baselineAgentId) return false;
    if (!profileTouched()) return true;
    return selectedProfile() === currentProfileLetter();
  });

  const applyEnabled = createMemo(() => {
    if (busy() || profileResolving() || !selectedAgent()) return false;
    const scope = selectedScope();
    if (scope === "replica") return !isRedundantReplicaSelection();
    if (!isWgReplica()) return false;
    if (scopePreviewBusy() || !scopePreview()) return false;
    if (scope === "workgroup" || scope === "kind") return dangerArmed();
    return false;
  });

  const apply = async () => {
    const agent = selectedAgent();
    if (!agent || !applyEnabled()) return;
    setBusy(true);
    setError("");
    setApplyErrors([]);
    const scope = selectedScope();
    const requested = requestedProfileForSelection();
    const effective = effectivePreview().effectiveProfile;
    const target = targetReplicaPath();
    const restart = scope === "replica" ? false : restartSessions();
    try {
      let updatedCount: number | undefined;
      let restartedCount: number | undefined;
      if (target && isWgReplica()) {
        const result = await SettingsAPI.applyCodingAgentProfileSelection({
          targetReplicaPath: target,
          codingAgentId: agent.id,
          profile: selectedProfile(),
          scope,
          restartSessions: restart,
          confirmedTargetFingerprint:
            scope === "replica" ? null : scopePreview()?.targetFingerprint ?? null,
          typedConfirmation: null,
        });
        if (result.errors.length > 0) {
          setApplyErrors(result.errors);
          const firstError = result.errors[0];
          const extra = result.errors.length - 1;
          showToast(extra > 0 ? `${firstError.message} (+${extra} more)` : firstError.message);
          setDangerArmed(false);
          if (scope !== "replica") setScopePreviews((prev) => ({ ...prev, [scope]: null }));
          setBusy(false);
          runScopePreview(scope, agent.id, selectedProfile());
          return;
        }
        updatedCount = result.updatedCount;
        restartedCount = result.restartedCount;
      }
      await props.onSelect({
        agent,
        requestedProfile: requested,
        effectiveProfile: effective,
        scope,
        restartSessions: restart,
        updatedCount,
        restartedCount,
      });
    } catch (err: unknown) {
      const message = launchErrorMessage(err);
      setError(message);
      showToast(message);
      if (scope !== "replica" && target && isWgReplica()) {
        setDangerArmed(false);
        setScopePreviews((prev) => ({ ...prev, [scope]: null }));
        runScopePreview(scope, agent.id, selectedProfile());
      }
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
      void apply();
    }
  };

  return (
    <>
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
            <div class="agent-profile-panel-head">
              <div class="agent-profile-panel-heading">
                <div class="agent-profile-panel-title">Coding Agent</div>
                <div class="agent-profile-panel-kicker">Choose the tool first</div>
              </div>
              <span class="agent-profile-step cyan" data-ac-role="status">step 1</span>
            </div>
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
                            : profileLabel(defaultPreview().effectiveProfile, agent.id)}
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
              <div class="agent-profile-panel-head">
                <div class="agent-profile-panel-heading">
                  <div class="agent-profile-panel-title">Profile</div>
                  <div class="agent-profile-panel-kicker">Choose the profile letter second</div>
                </div>
                <span class="agent-profile-step yellow" data-ac-role="status">step 2</span>
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
                    const pillKind = (): Exclude<ProfileBadgeKind, "invalid"> => {
                      const current = settings();
                      const agent = selectedAgent();
                      if (!current || !agent) return letter === "A" ? "match" : "fallback";
                      return profileBadgeKind(current.codingAgentProfiles, agent.id, letter);
                    };
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
                          <span class="agent-profile-card-tags">
                            <span
                              class={`agent-profile-card-pill ${pillKind()}`}
                              data-ac-role="status"
                              data-ac-state={pillKind()}
                              data-ac-testid={`agentPicker.profile.${letter}.pill`}
                            >
                              {SELECTION_PILL_LABEL[pillKind()]}
                            </span>
                            <Show when={configuredDefault() === letter}>
                              <span class="agent-profile-default-marker">Default</span>
                            </Show>
                          </span>
                        </span>
                        <span class="agent-profile-param-list">
                          <span class="agent-profile-param">
                            <span>Command </span>
                            <span>{composeEffectiveCommand(selectedAgent()?.command ?? "", profileCellCommandText(cell())) || "none"}</span>
                          </span>
                          <Show when={selected()}>
                            <span
                              class="agent-profile-declared-env"
                              data-ac-testid={`agentPicker.profile.${letter}.env`}
                              data-ac-role="list"
                            >
                              <span class="agent-profile-declared-env-head">Declared env</span>
                              <Show
                                when={declaredProfileEnv(selectedAgent(), letter).length > 0}
                                fallback={
                                  <span class="agent-profile-declared-env-empty">
                                    No declared env vars for {profileLabel(letter)}
                                  </span>
                                }
                              >
                                <span class="agent-profile-declared-env-grid">
                                  <For each={declaredProfileEnv(selectedAgent(), letter)}>
                                    {(entry) => (
                                      <span
                                        class="agent-profile-declared-env-row"
                                        data-ac-role="row"
                                        data-ac-env-origin={entry.origin}
                                      >
                                        <span class="agent-profile-declared-env-key">{entry.key}</span>
                                        <span class="agent-profile-declared-env-value">{entry.value}</span>
                                        <span class="agent-profile-declared-env-origin">{entry.origin}</span>
                                      </span>
                                    )}
                                  </For>
                                </span>
                              </Show>
                            </span>
                          </Show>
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
              class="agent-profile-panel agent-projection-panel"
              data-component="Same profile comparison panel"
              {...automationAttrs("agentPicker.comparison", "status")}
            >
              <div class="agent-projection-head">
                <div class="agent-projection-heading">
                  <div class="agent-profile-panel-title">Same Profile In Other Agents</div>
                  <div class="agent-profile-panel-kicker">
                    {profileLabel(selectedProfile())} compared across configured Coding Agents
                  </div>
                </div>
              </div>

              <div class="agent-comparison-summary" aria-label="Profile status summary">
                <div class="agent-comparison-summary-tile">
                  <span class="agent-comparison-summary-value direct">{comparisonSummary().direct}</span>
                  <span class="agent-comparison-summary-label">Direct</span>
                </div>
                <div class="agent-comparison-summary-tile">
                  <span class="agent-comparison-summary-value fallback">{comparisonSummary().fallback}</span>
                  <span class="agent-comparison-summary-label">Fallback</span>
                </div>
                <div class="agent-comparison-summary-tile">
                  <span class="agent-comparison-summary-value missing">{comparisonSummary().missing}</span>
                  <span class="agent-comparison-summary-label">Missing</span>
                </div>
              </div>

              <div class="agent-comparison-table" role="table" aria-label="Same profile comparison">
                <div class="agent-comparison-table-head" role="row">
                  <span>Coding Agent</span>
                  <span>Resolution</span>
                </div>
                <div class="agent-comparison-table-body" role="rowgroup">
                  <For each={comparisonRows()}>
                    {(row) => (
                      <button
                        type="button"
                        class="agent-comparison-row"
                        classList={{ active: row.active }}
                        role="row"
                        onClick={() => setHighlightIndex(row.index)}
                        data-ac-agent-id={row.agent.id}
                        data-ac-profile-status={row.status}
                        data-ac-effective-profile={row.preview.effectiveProfile}
                        data-ac-requested-profile={row.preview.requestedProfile}
                        {...automationAttrs(
                          `agentPicker.comparison.row.${row.agent.id}`,
                          "button",
                          row.active ? "active" : "inactive",
                        )}
                      >
                        <span class="agent-comparison-agent-cell">
                          <span class="agent-comparison-agent-name">{row.agent.label}</span>
                          <span class="agent-comparison-agent-sub">
                            {row.active ? "selected coding agent" : "configured peer"}
                          </span>
                        </span>
                        <span class="agent-comparison-resolution-cell">
                          <span
                            class={`agent-comparison-status ${row.status}`}
                            data-ac-role="status"
                            data-ac-state={row.status}
                          >
                            {comparisonStatusLabel(row.status)}
                          </span>
                          <span class="agent-comparison-resolution">
                            {comparisonResolutionText(row.agent.id, row.preview)}
                          </span>
                        </span>
                      </button>
                    )}
                  </For>
                </div>
              </div>

              <Show when={effectivePreview().fallbackApplied || hasBackendWarnings()}>
                <div
                  class="agent-profile-warning-strip agent-projection-status"
                  classList={{ visible: true }}
                  data-component="Coding Agent profile fallback explanation"
                  {...automationAttrs("agentPicker.fallback", "status", "warning")}
                >
                  <Show when={effectivePreview().fallbackApplied}>
                    <span>
                      {`${profileLabel(effectivePreview().requestedProfile)} is not configured for ${selectedAgent()?.label ?? "the selected coding agent"}; launch resolves through ${profileLabel(effectivePreview().effectiveProfile)}. A remains the final fallback.`}
                    </span>
                  </Show>
                  <Show when={hasBackendWarnings()}>
                    <span>Profile warning: {backendWarnings().join(" ")}</span>
                  </Show>
                </div>
              </Show>
            </section>
          </div>
        </div>

        <Show when={error()}>
          <div class="agent-picker-error">{error()}</div>
        </Show>

        {/* ── V2 scope picker botonera ── */}
        <div
          class="agent-picker-botonera"
          data-component="Coding Agent assignment scope picker"
          {...automationAttrs("agentPicker.scope", "surface", selectedScope())}
        >
          <Show when={showBroadScope()}>
            <div class="agent-scope-picker" role="radiogroup" aria-label="Apply scope">
              <span class="agent-scope-label">Apply to</span>
              <label
                class="agent-scope-opt"
                classList={{ active: selectedScope() === "replica" }}
                {...automationAttrs("agentPicker.scope.replica", "button", selectedScope() === "replica" ? "active" : "inactive")}
              >
                <input
                  type="radio"
                  name="agentPickerScope"
                  checked={selectedScope() === "replica"}
                  onChange={() => setSelectedScope("replica")}
                />
                This replica <span class="agent-scope-count">{scopeCount("replica")} replica</span>
              </label>
              <label
                class="agent-scope-opt"
                classList={{ active: selectedScope() === "kind", dangerous: selectedScope() === "kind" }}
                {...automationAttrs("agentPicker.scope.kind", "button", selectedScope() === "kind" ? "active" : "inactive")}
              >
                <input
                  type="radio"
                  name="agentPickerScope"
                  checked={selectedScope() === "kind"}
                  onChange={() => setSelectedScope("kind")}
                />
                All replicas of this kind <span class="agent-scope-count">{scopeCount("kind")} replicas</span>
              </label>
              <label
                class="agent-scope-opt"
                classList={{ active: selectedScope() === "workgroup", dangerous: selectedScope() === "workgroup" }}
                {...automationAttrs("agentPicker.scope.workgroup", "button", selectedScope() === "workgroup" ? "active" : "inactive")}
              >
                <input
                  type="radio"
                  name="agentPickerScope"
                  checked={selectedScope() === "workgroup"}
                  onChange={() => setSelectedScope("workgroup")}
                />
                Entire workgroup <span class="agent-scope-count">{scopeCount("workgroup")} replicas</span>
              </label>
            </div>
            <div class="agent-scope-live-note">
              <span class="agent-scope-live-tag">live</span>
              Counts are read from the current workgroup; the backend re-enumerates targets before applying.
            </div>
          </Show>

          <Show when={scopePreviewBusy()}>
            <div class="agent-scope-status" data-ac-testid="agentPicker.previewBusy" data-ac-role="status">
              Loading targets…
            </div>
          </Show>
          <Show when={scopePreviewError()}>
            <div class="agent-scope-error" data-ac-testid="agentPicker.previewError" data-ac-role="status">
              {scopePreviewError()}
            </div>
          </Show>

          {/* Cross-workgroup target review for `kind` */}
          <Show when={selectedScope() === "kind" && scopePreview()}>
            <div
              class="agent-scope-targets"
              data-ac-testid="agentPicker.targets"
              data-ac-role="list"
            >
              <div class="agent-scope-targets-head">
                {scopePreview()!.targetCount} replica(s) across {distinctWorkgroupCount()} workgroup(s) ·{" "}
                {scopePreview()!.liveSessionCount} live session(s)
              </div>
              <For each={scopePreview()!.targets}>
                {(t) => (
                  <div
                    class="agent-scope-target-row"
                    data-ac-role="row"
                    data-ac-replica-path={t.replicaPath}
                    data-ac-live-sessions={t.liveSessionIds.length}
                  >
                    <span class="agent-scope-target-wg">{t.workgroupName}</span>
                    <span class="agent-scope-target-name">{t.replicaName}</span>
                    <span class="agent-scope-target-path">{t.replicaPath}</span>
                    <Show when={t.liveSessionIds.length > 0}>
                      <span class="agent-scope-target-live">{t.liveSessionIds.length} live</span>
                    </Show>
                  </div>
                )}
              </For>
            </div>
          </Show>

          {/* #537: prominent, persistent failure banner. The toast (loud, fades
              after 3s) flags the failure; this banner keeps the backend's
              human-readable message and a replica-scope retry on screen. */}
          <Show when={applyErrors().length > 0}>
            <div
              class="agent-scope-error-banner"
              role="alert"
              data-ac-testid="agentPicker.errors"
              data-ac-role="alert"
            >
              <div class="agent-scope-error-banner-head">
                <span class="agent-scope-error-banner-title">Assignment failed</span>
                <Show when={selectedScope() === "replica"}>
                  <button
                    type="button"
                    class="agent-scope-error-retry"
                    disabled={!applyEnabled()}
                    onClick={() => void apply()}
                    {...automationAttrs("agentPicker.retry", "button", applyEnabled() ? "enabled" : "disabled")}
                  >
                    Retry
                  </button>
                </Show>
              </div>
              <For each={applyErrors()}>
                {(e) => (
                  <div class="agent-scope-error-row">
                    {e.message}
                    <Show when={e.sessionIds.length > 0}>
                      <span class="agent-scope-error-ids"> ({e.sessionIds.join(", ")})</span>
                    </Show>
                  </div>
                )}
              </For>
            </div>
          </Show>
          <Show when={(scopePreview()?.warnings.length ?? 0) > 0}>
            <div class="agent-scope-warnings" data-ac-testid="agentPicker.warnings" data-ac-role="status">
              <For each={scopePreview()!.warnings}>{(w) => <div>{w}</div>}</For>
            </div>
          </Show>

          <div class="agent-picker-bar">
            {/* #537: replica scope is restarted via the post-assign "Restart now?"
                modal, not this toggle. The toggle stays for kind/workgroup scope
                (no per-replica prompt makes sense for a multi-target apply). */}
            <Show when={isWgReplica() && selectedScope() !== "replica"}>
              <label class="agent-scope-switch" title="Restart matching sessions after writing the selection">
                <input
                  type="checkbox"
                  checked={restartSessions()}
                  onChange={(e) => setRestartSessions(e.currentTarget.checked)}
                  {...automationAttrs("agentPicker.restartToggle", "checkbox", restartSessions() ? "checked" : "unchecked")}
                />
                <span>Restart sessions after apply</span>
              </label>
            </Show>

            <div class="agent-picker-bar-spacer" />

            <Show when={selectedScope() === "workgroup" || selectedScope() === "kind"}>
              <label class="agent-scope-arm">
                <input
                  type="checkbox"
                  checked={dangerArmed()}
                  disabled={!scopePreview()}
                  onChange={(e) => setDangerArmed(e.currentTarget.checked)}
                  {...automationAttrs("agentPicker.armToggle", "checkbox", dangerArmed() ? "checked" : "unchecked")}
                />
                <span>{confirmationLabel()}</span>
              </label>
            </Show>

            <button
              class="modal-btn modal-btn-cancel"
              disabled={busy()}
              onClick={props.onClose}
              {...automationAttrs("agentPicker.cancel", "button")}
            >
              Cancel
            </button>
            <button
              class="modal-btn modal-btn-save agent-picker-apply"
              classList={{ danger: selectedScope() !== "replica" }}
              disabled={!applyEnabled()}
              title={isRedundantReplicaSelection() ? REDUNDANT_REPLICA_ASSIGN_TOOLTIP : undefined}
              onClick={() => void apply()}
              {...automationAttrs(
                "agentPicker.apply",
                "button",
                applyEnabled() ? "enabled" : "disabled",
              )}
            >
              {applyLabel()}
            </button>
          </div>

        </div>
      </div>
    </div>
    {/* #537: viewport-level toast so the failure is unmissable even with the
        modal scrolled. Mirrors SidebarApp/SettingsModal `.toast-error`. */}
    <Show when={toastMsg()}>
      <div class="toast-error" data-ac-testid="agentPicker.toast" data-ac-role="alert">
        {toastMsg()}
      </div>
    </Show>
    </>
  );
};

export default AgentPickerModal;
