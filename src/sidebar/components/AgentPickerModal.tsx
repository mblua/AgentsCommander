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
  effectiveEnvProjection,
  expandAcRootPreview,
  hasAcRootPlaceholder,
  isAcAgentPath,
  isCodexAgent,
  isWgReplicaPath,
  normalizeProfileLetter,
  profileBadgeKind,
  type ProfileBadgeKind,
  profileCellCommandText,
  profileDisplayLabel,
  resolveProfilePreview,
  sortedProfileLetters,
  targetProfileFqn,
} from "../../shared/profile-utils";

/** Scope/replica context supplied by ProjectPanel for WG replica launches. When
 *  absent (OpenAgentModal / normal repo / RootAgentBanner) only the safe
 *  "this replica" scope is offered. (#384 Frontend §4) */
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

// #527: per-card status pill labels for the Selection profile cards. Same
// taxonomy/colors as the Config rails (profileBadgeKind), so a given (agent,
// letter) reads identically on both screens. `invalid` is Config-only (live
// command edits), so it never surfaces here.
const SELECTION_PILL_LABEL: Record<Exclude<ProfileBadgeKind, "invalid">, string> = {
  match: "MATCH",
  configured: "CONFIGURED",
  fallback: "FALLBACK",
  missing: "MISSING",
};

const AgentPickerModal: Component<{
  sessionName: string;
  agentPath?: string | null;
  currentAgentId?: string | null;
  currentRequestedProfile?: string | null;
  scopeContext?: AgentPickerScopeContext;
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

  // ── V2 scope picker state ──
  const [selectedScope, setSelectedScope] = createSignal<ProfileAssignmentScope>("replica");
  const [restartSessions, setRestartSessions] = createSignal(false);
  const [dangerArmed, setDangerArmed] = createSignal(false);
  const [kindConfirmationText, setKindConfirmationText] = createSignal("");
  const [scopePreview, setScopePreview] = createSignal<PreviewCodingAgentProfileSelectionResult | null>(null);
  const [scopePreviewBusy, setScopePreviewBusy] = createSignal(false);
  const [scopePreviewError, setScopePreviewError] = createSignal("");
  const [applyErrors, setApplyErrors] = createSignal<ProfileAssignmentError[]>([]);
  // #537: transient toast so an assign failure is loud and unmissable (the
  // persistent banner below keeps the actionable detail once the toast fades).
  const [toastMsg, setToastMsg] = createSignal<string | null>(null);

  let overlayRef!: HTMLDivElement;
  let profileResolveSeq = 0;
  let previewSeq = 0;
  let toastTimer: ReturnType<typeof setTimeout> | null = null;

  // #537: mirror the SidebarApp/SettingsModal toast pattern (.toast-error,
  // auto-dismiss after 3s). Used to surface a failed Coding Agent assignment.
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
  // Prefer the explicit scope-context replica path; fall back to the legacy agentPath.
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
  // Broad scope (kind/workgroup) is only offered for a real WG replica with
  // workgroup context. Everything else collapses to "assign to this replica".
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
  const profileLabel = (letter: string) =>
    settings() ? profileDisplayLabel(settings()!.codingAgentProfiles, letter) : letter;
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
  const projectedCell = createMemo(() => {
    const agent = selectedAgent();
    if (!agent) return EMPTY_DISPLAY_CELL;
    return enabledLaunchCellFor(agent, effectivePreview().effectiveProfile);
  });
  // The cell command is the full invocation; an empty cell command falls back to
  // the agent's base command. Display-only %AC_ROOT% expansion uses the replica root.
  const projectedCommand = createMemo(() => {
    const cmd = profileCellCommandText(projectedCell()) || selectedAgent()?.command || "";
    return expandAcRootPreview(cmd, acRoot());
  });
  // #527 Effective Projection: merged effective env (agent env + profile env) with
  // per-value origin badges, plus the chosen-pair resolution descriptors.
  const effectiveEnv = createMemo(() =>
    effectiveEnvProjection(selectedAgent()?.envs, projectedCell().env, acRoot()),
  );
  const projectionFallback = createMemo(() => effectivePreview().fallbackApplied);
  const projectionBadgeKind = createMemo<"match" | "fallback">(() =>
    projectionFallback() ? "fallback" : "match",
  );
  const resolutionText = createMemo(() =>
    projectionFallback()
      ? `${profileLabel(effectivePreview().requestedProfile)} → ${profileLabel(effectivePreview().effectiveProfile)} (fallback)`
      : "Direct match",
  );
  const commandUsesAcRoot = createMemo(() =>
    hasAcRootPlaceholder(profileCellCommandText(projectedCell()) || selectedAgent()?.command || ""),
  );
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
    const acDefault = isAcAgentPath(targetReplicaPath())
      ? normalizeProfileLetter(loaded.codingAgentProfiles.defaultProfileByAgent[targetName()])
      : null;
    const requested = currentRequested ?? acDefault ?? "A";
    setSelectedProfile(requested);
    setInitialProfileShouldLaunch(Boolean(currentRequested) || Boolean(acDefault));
  });

  // Backend profile resolution for the projected preview (effective letter, fallback).
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

  // Backend scope preview — authoritative target/live-session enumeration and the
  // fingerprint that apply re-validates. Re-runs (and clears the typed confirmation,
  // arm checkbox, preview, fingerprint) whenever scope, coding agent, profile,
  // restart toggle, or target replica changes (#384 Frontend §4).
  const runScopePreview = (scope: ProfileAssignmentScope, agentId: string, profile: string) => {
    const target = targetReplicaPath();
    if (!target || !isWgReplica()) return;
    const seq = ++previewSeq;
    setScopePreviewBusy(true);
    setScopePreviewError("");
    SettingsAPI.previewCodingAgentProfileSelection({
      targetReplicaPath: target,
      codingAgentId: agentId,
      profile,
      scope,
      restartSessions: restartSessions(),
    })
      .then((result) => {
        if (seq !== previewSeq) return;
        setScopePreview(result);
      })
      .catch((err: unknown) => {
        if (seq !== previewSeq) return;
        setScopePreview(null);
        setScopePreviewError(err instanceof Error ? err.message : String(err));
      })
      .finally(() => {
        if (seq === previewSeq) setScopePreviewBusy(false);
      });
  };

  createEffect(() => {
    const scope = selectedScope();
    const agent = selectedAgent();
    const profile = selectedProfile();
    // Track these so any change resets the confirmation gate and re-previews.
    restartSessions();
    targetReplicaPath();

    // Reset every transient confirmation/preview artifact on any input change.
    previewSeq += 1;
    setDangerArmed(false);
    setKindConfirmationText("");
    setScopePreview(null);
    setScopePreviewError("");
    setApplyErrors([]);
    setScopePreviewBusy(false);

    if (!agent || !isWgReplica()) return;
    runScopePreview(scope, agent.id, profile);
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

  // Typed confirmation phrase for the cross-workgroup `kind` scope. Must match the
  // backend phrase exactly, including count and restart wording (#384 §7).
  const kindPhrase = createMemo(() => {
    const preview = scopePreview();
    const agent = selectedAgent();
    if (!preview || !agent) return "";
    const restart = restartSessions() ? "WITH RESTART" : "WITHOUT RESTART";
    return `APPLY ${agent.id}:${selectedProfile()} TO ${preview.targetCount} REPLICAS ${restart}`;
  });

  const scopeCount = (scope: ProfileAssignmentScope): number => {
    const preview = scopePreview();
    if (preview && selectedScope() === scope) return preview.targetCount;
    return scope === "replica" ? 1 : 0;
  };

  const distinctWorkgroupCount = createMemo(() => {
    const targets = scopePreview()?.targets ?? [];
    return new Set(targets.map((t) => t.workgroupName)).size;
  });

  const applyLabel = createMemo(() => {
    const scope = selectedScope();
    if (scope === "replica") return "Assign to this replica";
    const count = scopePreview()?.targetCount ?? 0;
    if (scope === "kind") return `Overwrite ${count} of this kind`;
    const wg = props.scopeContext?.workgroupName;
    return `Overwrite ${count}${wg ? ` in ${wg}` : " in this workgroup"}`;
  });

  const applyEnabled = createMemo(() => {
    if (busy() || profileResolving() || !selectedAgent()) return false;
    const scope = selectedScope();
    if (scope === "replica") return true;
    if (!isWgReplica()) return false;
    if (scopePreviewBusy() || !scopePreview()) return false;
    if (scope === "workgroup") return dangerArmed();
    if (scope === "kind") return kindConfirmationText().trim() === kindPhrase();
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
    // #537: replica scope no longer uses the pre-apply restart toggle; the post-assign
    // "Restart now?" modal (ProjectPanel) owns the restart. Force it off so a toggle
    // value carried over from kind/workgroup scope cannot trigger a backend restart.
    const restart = scope === "replica" ? false : restartSessions();
    try {
      let updatedCount: number | undefined;
      let restartedCount: number | undefined;
      const target = targetReplicaPath();
      // Broad-scope writes are backend-owned; only call apply for a real WG replica.
      if (target && isWgReplica()) {
        const result = await SettingsAPI.applyCodingAgentProfileSelection({
          targetReplicaPath: target,
          codingAgentId: agent.id,
          profile: selectedProfile(),
          scope,
          restartSessions: restart,
          confirmedTargetFingerprint:
            scope === "replica" ? null : scopePreview()?.targetFingerprint ?? null,
          typedConfirmation: scope === "kind" ? kindConfirmationText().trim() : null,
        });
        if (result.errors.length > 0) {
          // Stale fingerprint / failed targets: surface errors, keep the modal open,
          // reset the typed confirmation and re-run preview so the user re-confirms.
          setApplyErrors(result.errors);
          // #537: the assign no longer silently no-ops. Fire a loud toast with the
          // backend's human-readable message so the failure cannot be missed.
          const firstError = result.errors[0];
          const extra = result.errors.length - 1;
          showToast(extra > 0 ? `${firstError.message} (+${extra} more)` : firstError.message);
          setKindConfirmationText("");
          setDangerArmed(false);
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
      // #516 — surface the failure in the modal (keep it open) rather than
      // letting the rejection escape as a swallowed unhandled rejection. The
      // Resource Monitor cap gets a friendly, actionable message.
      const message = launchErrorMessage(err);
      setError(message);
      // #537: keep unexpected apply failures as loud as the structured ones.
      showToast(message);
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
                    // #527: per-card status pill — shared taxonomy with the Config rails.
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
                            <span>{profileCellCommandText(cell()) || selectedAgent()?.command || "none"}</span>
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
              class="agent-profile-panel agent-projection-panel"
              data-component="Effective projection panel"
              {...automationAttrs("agentPicker.projected", "status")}
            >
              <div class="agent-projection-head">
                <div class="agent-projection-heading">
                  <div class="agent-profile-panel-title">Effective Projection</div>
                  <div class="agent-profile-panel-kicker">Projected command and env for the chosen pair</div>
                </div>
                <span
                  class={`agent-projection-badge ${projectionBadgeKind()}`}
                  data-ac-role="status"
                  data-ac-state={projectionBadgeKind()}
                >
                  {projectionBadgeKind()}
                </span>
              </div>

              {/* Chosen pair — AGENT THEN PROFILE */}
              <div class="agent-projection-card">
                <div class="agent-projection-card-head">
                  <span class="agent-projection-card-title">Chosen pair</span>
                  <span class="agent-projection-tag cyan">agent then profile</span>
                </div>
                <div class="agent-projection-grid">
                  <div class="agent-projection-row" data-ac-role="row">
                    <span class="agent-projection-label">Coding agent</span>
                    <span class="agent-projection-value" data-component="Selected coding agent">
                      {selectedAgent()?.label ?? "none"}
                    </span>
                    <span class="agent-projection-pill green">selected</span>
                  </div>
                  <div class="agent-projection-row" data-ac-role="row">
                    <span class="agent-projection-label">Profile letter</span>
                    <span class="agent-projection-value">{effectivePreview().requestedProfile}</span>
                    <span class={`agent-projection-pill ${projectionBadgeKind() === "fallback" ? "yellow" : "green"}`}>
                      {projectionBadgeKind()}
                    </span>
                  </div>
                  <div class="agent-projection-row" data-ac-role="row">
                    <span class="agent-projection-label">Resolution</span>
                    <span class="agent-projection-value" data-component="Profile resolution">{resolutionText()}</span>
                    <span class={`agent-projection-pill ${projectionBadgeKind() === "fallback" ? "yellow" : "green"}`}>
                      {projectionBadgeKind()}
                    </span>
                  </div>
                </div>
              </div>

              {/* Command (RESOLVED) + Env (EFFECTIVE) */}
              <div class="agent-projection-pair">
                <div class="agent-projection-card">
                  <div class="agent-projection-card-head">
                    <span class="agent-projection-card-title">Command</span>
                    <span class="agent-projection-tag cyan">resolved</span>
                  </div>
                  <div class="agent-projection-command" data-component="Resolved invocation">
                    <span class="agent-projection-command-label">Invocation</span>
                    <span class="agent-projection-command-value">{projectedCommand() || "none"}</span>
                  </div>
                  <Show when={commandUsesAcRoot()}>
                    <div class="agent-projection-ph">
                      <span class="arrow">→</span>
                      <span>%AC_ROOT% expands at launch; the backend validates the path.</span>
                    </div>
                  </Show>
                  <Show when={selectedIsCodex() || selectedAgent()?.isolatedHome}>
                    <div class="agent-projection-note">
                      Home isolation {selectedAgent()?.isolatedHome ? "enabled" : "disabled"}
                    </div>
                  </Show>
                  <Show when={projectedCell().notes}>
                    <div class="agent-projection-note">Note: {projectedCell().notes}</div>
                  </Show>
                </div>

                <div class="agent-projection-card">
                  <div class="agent-projection-card-head">
                    <span class="agent-projection-card-title">Env</span>
                    <span class="agent-projection-tag cyan">effective</span>
                  </div>
                  <div class="agent-projection-grid" data-ac-testid="agentPicker.projectedEnv" data-ac-role="list">
                    <Show
                      when={effectiveEnv().length > 0}
                      fallback={
                        <div class="agent-projection-row">
                          <span class="agent-projection-label">env</span>
                          <span class="agent-projection-value">No additional env vars</span>
                          <span class="agent-projection-pill">none</span>
                        </div>
                      }
                    >
                      <For each={effectiveEnv()}>
                        {(entry) => (
                          <div class="agent-projection-row" data-ac-role="row" data-ac-env-origin={entry.origin}>
                            <span class="agent-projection-label">{entry.key}</span>
                            <span class="agent-projection-value">{entry.value}</span>
                            <span
                              class={`agent-projection-pill ${entry.origin === "profile" ? "" : "green"}`}
                            >
                              {entry.origin}
                            </span>
                          </div>
                        )}
                      </For>
                    </Show>
                  </div>
                </div>
              </div>

              <div
                class="agent-profile-warning-strip agent-projection-status"
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

            <Show when={selectedScope() === "workgroup"}>
              <label class="agent-scope-arm">
                <input
                  type="checkbox"
                  checked={dangerArmed()}
                  disabled={!scopePreview()}
                  onChange={(e) => setDangerArmed(e.currentTarget.checked)}
                  {...automationAttrs("agentPicker.armToggle", "checkbox", dangerArmed() ? "checked" : "unchecked")}
                />
                <span>I understand this overwrites {scopeCount("workgroup")} replicas</span>
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

          <Show when={selectedScope() === "kind"}>
            <div class="agent-scope-confirm">
              <label class="agent-scope-confirm-label">
                Type to confirm: <code>{kindPhrase()}</code>
              </label>
              <input
                class="agent-scope-confirm-input"
                value={kindConfirmationText()}
                onInput={(e) => setKindConfirmationText(e.currentTarget.value)}
                placeholder={kindPhrase()}
                spellcheck={false}
                {...automationAttrs("agentPicker.kindConfirm", "textbox", applyEnabled() ? "matched" : "unmatched")}
              />
            </div>
          </Show>
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
