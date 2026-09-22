import { Component, createSignal, createMemo, For, Show, onMount, onCleanup, createEffect } from "solid-js";
import type {
  AgentConfig,
  AppSettings,
  ApplySelectionLockRemovalResult,
  AssignmentMode,
  CodingAgentProfileResolution,
  ConflictDecision,
  MoveCodingAgentDirection,
  ProfileCellConfig,
  ProfileAssignmentScope,
  ProfileAssignmentError,
  PreviewCodingAgentProfileSelectionResult,
  PreviewSelectionLockRemovalResult,
  ReplicaSelectionDefaultResult,
  SavedPair,
  SelectionError,
  SelectionState,
  SettingsSnapshot,
} from "../../shared/types";
import {
  SettingsAPI,
  onCodingAgentProfileSelectionUpdated,
  onCodingAgentSettingsUpdated,
} from "../../shared/ipc";
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

/** #1943 - the lock glyph. Exported so the sidebar lock chip draws the same
 *  shape without a second definition; ProjectPanel already imports this module,
 *  so it adds no dependency edge. */
export const LockIcon: Component<{ class?: string }> = (props) => (
  <svg class={props.class} viewBox="0 0 16 16" fill="none" aria-hidden="true">
    <rect x="3" y="7" width="10" height="7" rx="1.6" stroke="currentColor" stroke-width="1.5" />
    <path
      d="M5.5 7V5.4A2.5 2.5 0 0 1 8 3a2.5 2.5 0 0 1 2.5 2.4V7"
      stroke="currentColor"
      stroke-width="1.5"
      stroke-linecap="round"
    />
  </svg>
);

export type AgentPickerScopeContext = {
  workgroupPath?: string;
  workgroupName?: string;
  targetReplicaPath?: string;
  targetReplicaName?: string;
  currentCodingAgentId?: string | null;
  currentProfile?: string | null;
  /** #1943 - the persisted protected pair from discovery. `undefined` is an
   *  older backend: unsupported/unknown, never proven unlocked. */
  savedPair?: SavedPair | null;
  /** #1943 - strict protection state from discovery; absent is unknown. */
  selectionState?: SelectionState;
  /** #1943 - strict-read diagnostic carried beside an `invalid` state. */
  selectionError?: string | null;
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

/** #2306 - the one reason both surfaces disable moves when the local overlay owns
 *  top-level `agents`. Kept identical here and in SettingsModal. */
const MOVE_OVERLAY_REASON =
  "Agent order is controlled by the local settings overlay (settings.local.json).";

/** #1943 - the three scopes, in the order the lock radios and the independent
 *  "Remove lock from" group both render them. */
const LOCK_SCOPES: ProfileAssignmentScope[] = ["replica", "kind", "workgroup"];

const LOCK_SCOPE_LABEL: Record<ProfileAssignmentScope, string> = {
  replica: "This replica",
  kind: "All replicas of this kind",
  workgroup: "Entire room",
};

const LOCK_SCOPE_TEST_ID: Record<ProfileAssignmentScope, string> = {
  replica: "replica",
  kind: "kind",
  workgroup: "workgroup",
};

/** #1943 - the one sentence a complete removal reports. A partial run still
 *  states the count that actually landed; the error rows carry the rest. */
function removalOutcomeMessage(result: ApplySelectionLockRemovalResult): string {
  const noun = result.removedCount === 1 ? "replica" : "replicas";
  return `Lock removed from ${result.removedCount} ${noun} · Coding Agent + Profile kept · no restart`;
}

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
  // #2306 - snapshot metadata and the modal-local move state. Moves are
  // serialized per modal: one request in flight, every move control disabled
  // until the authoritative refetch lands.
  const [overlayOwnsAgents, setOverlayOwnsAgents] = createSignal(false);
  const [moveBusy, setMoveBusy] = createSignal(false);
  const [moveError, setMoveError] = createSignal("");
  const [moveAnnouncement, setMoveAnnouncement] = createSignal("");

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

  // #1943 - the operation is one mutually exclusive selection of (scope, mode):
  // the ordinary scope the picker always had, plus the same scope with `+ lock`.
  const [assignmentMode, setAssignmentMode] = createSignal<AssignmentMode>("ordinary");
  // #1943 - a reviewed conflict policy is only ever echoed with the fingerprint
  // the backend issued for that exact decision.
  const [conflictDecision, setConflictDecision] = createSignal<ConflictDecision | null>(null);
  const [conflictOpen, setConflictOpen] = createSignal(false);

  // #1943 - "Remove lock from" keeps its OWN scope, previews and counts: it is
  // deliberately independent of the `Apply to` selection above.
  const [removeScope, setRemoveScope] = createSignal<ProfileAssignmentScope>("replica");
  const emptyRemovePreviews: Record<ProfileAssignmentScope, PreviewSelectionLockRemovalResult | null> = {
    replica: null,
    kind: null,
    workgroup: null,
  };
  const [removePreviews, setRemovePreviews] = createSignal<
    Record<ProfileAssignmentScope, PreviewSelectionLockRemovalResult | null>
  >({ ...emptyRemovePreviews });
  const [removePreviewBusyMap, setRemovePreviewBusyMap] = createSignal<
    Record<ProfileAssignmentScope, boolean>
  >({ replica: false, kind: false, workgroup: false });
  const [removePreviewErrorMap, setRemovePreviewErrorMap] = createSignal<
    Record<ProfileAssignmentScope, string>
  >({ replica: "", kind: "", workgroup: "" });
  const [removeResult, setRemoveResult] = createSignal<ApplySelectionLockRemovalResult | null>(null);
  const [removeErrors, setRemoveErrors] = createSignal<SelectionError[]>([]);
  const [removeBusy, setRemoveBusy] = createSignal(false);

  // #1943 - the Matrix default for FUTURE replicas. The persisted snapshot and
  // the user's unsaved draft stay separate: nothing here saves on change.
  const [selectionDefault, setSelectionDefault] = createSignal<ReplicaSelectionDefaultResult | null>(null);
  const [defaultDraftLocked, setDefaultDraftLocked] = createSignal(false);
  const [defaultBusy, setDefaultBusy] = createSignal(false);
  const [defaultNotice, setDefaultNotice] = createSignal("");

  let overlayRef!: HTMLDivElement;
  let profileResolveSeq = 0;
  let previewSeqByScope: Record<ProfileAssignmentScope, number> = { replica: 0, kind: 0, workgroup: 0 };
  let removeSeqByScope: Record<ProfileAssignmentScope, number> = { replica: 0, kind: 0, workgroup: 0 };
  let defaultSeq = 0;
  // The draft is seeded ONCE from the stored default, so a later refresh cannot
  // silently discard what the user typed but did not save.
  let defaultDraftSeeded = false;
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

  // #2306 - the picker renders the backend's vector order verbatim: no
  // alphabetical copy and no local re-ordering. Explicit `order` ordinals
  // already won backend-side (P1/P2), so the received array IS the effective
  // order.
  const orderedAgents = createMemo(() => agents());

  const selectedAgent = createMemo(() => orderedAgents()[highlightIndex()] ?? null);
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
  const replicaRoot = createMemo(() => targetReplicaPath());

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
        value: expandAcPlaceholdersPreview(value, replicaRoot()),
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
    return orderedAgents().map((agent, index) => {
      const preview = resolveProfilePreview(current.codingAgentProfiles, agent.id, selectedProfile());
      const cell = enabledLaunchCellFor(agent, preview.effectiveProfile);
      const command = expandAcPlaceholdersPreview(
        composeEffectiveCommand(agent.command, profileCellCommandText(cell)),
        replicaRoot(),
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
        // #2014 - the line the panel shows: command + the EFFECTIVE profile
        // cell's arguments, never the final spawn argv (plan D1).
        launchLine: command,
        active: index === highlightIndex(),
      };
    });
  });
  const launchLineByAgentId = createMemo(
    () => new Map(comparisonRows().map((row) => [row.agent.id, row.launchLine]))
  );
  // #2014 - the agent filter is a LEFT-COLUMN view concern: it never
  // re-indexes orderedAgents(), so highlightIndex keeps addressing the full list.
  const [agentFilter, setAgentFilter] = createSignal("");
  const filterQuery = createMemo(() => agentFilter().trim().toLowerCase());
  const matchesFilter = (agent: AgentConfig) => {
    const q = filterQuery();
    return (
      q === "" ||
      `${agent.label} ${launchLineByAgentId().get(agent.id) ?? agent.command}`
        .toLowerCase()
        .includes(q)
    );
  };
  const visibleAgentCount = createMemo(() => orderedAgents().filter(matchesFilter).length);
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

  // #2306 - one modal-local coalesced refresh. Concurrent settings events (or an
  // event racing a move's own refetch) collapse into at most one extra fetch, so
  // this modal never opens a second refresh owner for the same state.
  let settingsRefreshInFlight: Promise<void> | null = null;
  let settingsRefreshQueued = false;

  const installLoadedSnapshot = (loaded: SettingsSnapshot, reconcileSelection: boolean) => {
    setSettings(loaded);
    setOverlayOwnsAgents(loaded.overlayOwnsAgents === true);
    const next = loaded.agents;
    if (!reconcileSelection) {
      setAgents(next);
      return;
    }
    const previous = agents();
    const previousIndex = highlightIndex();
    const previousSelectedId = previous[previousIndex]?.id ?? null;
    setAgents(next);
    if (next.length === 0) {
      setHighlightIndex(0);
      return;
    }
    const survivingIndex = previousSelectedId
      ? next.findIndex((agent) => agent.id === previousSelectedId)
      : -1;
    // Clamped-old-position rule: a disappeared selection falls to the survivor
    // shifted into the old numeric slot, or the new final item when the list
    // shrank past it (successor over predecessor, deterministic for coalesced removals).
    setHighlightIndex(
      survivingIndex >= 0 ? survivingIndex : Math.min(previousIndex, next.length - 1),
    );
  };

  const refreshFromSettings = (): Promise<void> => {
    if (settingsRefreshInFlight) {
      settingsRefreshQueued = true;
      return settingsRefreshInFlight;
    }
    settingsRefreshInFlight = (async () => {
      try {
        do {
          settingsRefreshQueued = false;
          const loaded = await SettingsAPI.get();
          installLoadedSnapshot(loaded, true);
        } while (settingsRefreshQueued);
      } finally {
        settingsRefreshInFlight = null;
        settingsRefreshQueued = false;
      }
    })();
    return settingsRefreshInFlight;
  };

  const moveControlTestId = (agentId: string, direction: MoveCodingAgentDirection) =>
    `agentPicker.provider.${agentId}.move${direction === "up" ? "Up" : "Down"}`;

  const moveControlsDisabled = () =>
    moveBusy() || overlayOwnsAgents() || filterQuery() !== "";
  const moveUpDisabled = (index: number) => moveControlsDisabled() || index <= 0;
  const moveDownDisabled = (index: number) =>
    moveControlsDisabled() || index >= orderedAgents().length - 1;
  /** Distinct tool-and-direction accessible name; when the overlay owns the
   *  order, the name carries the backend ownership reason it is disabled for. */
  const moveControlLabel = (agent: AgentConfig, direction: MoveCodingAgentDirection) => {
    const name = agent.label || agent.id;
    return overlayOwnsAgents()
      ? `Move ${name} ${direction} \u2014 ${MOVE_OVERLAY_REASON}`
      : `Move ${name} ${direction}`;
  };
  const moveControlTitle = (agent: AgentConfig, direction: MoveCodingAgentDirection) =>
    overlayOwnsAgents() ? MOVE_OVERLAY_REASON : `Move ${agent.label || agent.id} ${direction}`;

  /** Focus the corresponding moved-tool control, or its remaining direction at a
   *  new boundary, or the tool card when both directions are gone. */
  const focusMoveControl = (agentId: string, direction: MoveCodingAgentDirection) => {
    queueMicrotask(() => {
      const other: MoveCodingAgentDirection = direction === "up" ? "down" : "up";
      for (const candidate of [direction, other]) {
        const control = document.querySelector<HTMLButtonElement>(
          `[data-ac-testid="${moveControlTestId(agentId, candidate)}"]`,
        );
        if (control && !control.disabled) {
          control.focus();
          return;
        }
      }
      document
        .querySelector<HTMLButtonElement>(`[data-ac-testid="agentPicker.provider.${agentId}"]`)
        ?.focus();
    });
  };

  /** #2306 - one adjacent move through the narrow command. The returned id order
   *  is a consistency check only; the authoritative state always comes from the
   *  follow-up get_settings, and a mismatch never applies a speculative order. */
  const moveAgent = async (agent: AgentConfig, direction: MoveCodingAgentDirection) => {
    if (moveBusy() || overlayOwnsAgents() || filterQuery() !== "") return;
    const list = orderedAgents();
    const index = list.findIndex((candidate) => candidate.id === agent.id);
    const neighbor = direction === "up" ? list[index - 1] : list[index + 1];
    if (index < 0 || !neighbor) return;
    setMoveBusy(true);
    setMoveError("");
    setMoveAnnouncement("");
    try {
      const ids = await SettingsAPI.moveCodingAgent({
        id: agent.id,
        neighborId: neighbor.id,
        direction,
      });
      const expectedIds = list.map((candidate) => candidate.id);
      const consistent =
        ids.length === expectedIds.length &&
        new Set(ids).size === ids.length &&
        ids.every((id) => expectedIds.includes(id));
      if (!consistent) throw new Error("The backend returned an unexpected agent order.");
      await refreshFromSettings();
      const newIndex = agents().findIndex((candidate) => candidate.id === agent.id);
      if (newIndex >= 0) {
        setMoveAnnouncement(
          `Moved ${agent.label || agent.id} ${direction} to position ${newIndex + 1} of ${agents().length}.`,
        );
      }
    } catch (err: unknown) {
      setMoveError(launchErrorMessage(err));
      // Resolve possible external progress; never apply speculative order.
      try {
        await refreshFromSettings();
      } catch {
        // Keep the last authoritative order visible.
      }
    } finally {
      setMoveBusy(false);
      focusMoveControl(agent.id, direction);
    }
  };

  onMount(async () => {
    overlayRef?.focus();
    // #1943 - reload this modal's own previews and default on external updates.
    // App already owns the global project/settings refresh; these listeners are
    // modal-scoped and add no second refresh owner.
    let disposed = false;
    const unlisteners: Array<() => void> = [];
    const bind = (pending: Promise<() => void>) => {
      void pending.then((fn) => {
        if (disposed) {
          fn();
          return;
        }
        unlisteners.push(fn);
      });
    };
    bind(
      onCodingAgentProfileSelectionUpdated(() => {
        if (disposed) return;
        handleExternalSelectionUpdate();
      }),
    );
    // #2306 - a move or any external coding-agent mutation refetches this modal
    // through its own coalesced path; the app-lifetime store listener is not the
    // owner of this modal's draft state.
    bind(
      onCodingAgentSettingsUpdated(() => {
        if (disposed) return;
        void refreshFromSettings().catch(() => {});
      }),
    );
    onCleanup(() => {
      disposed = true;
      for (const fn of unlisteners) fn();
      unlisteners.length = 0;
    });

    const loaded = await SettingsAPI.get();
    // #2306 - vector order decides the initial selection: no alphabetical remap.
    const agentIndex = loaded.agents.findIndex((agent) => agent.id === props.currentAgentId);
    if (agentIndex >= 0) setHighlightIndex(agentIndex);
    installLoadedSnapshot(loaded, false);
    const currentRequested = normalizeProfileLetter(props.currentRequestedProfile);
    const acDefault = isAcAgentPath(targetReplicaPath())
      ? normalizeProfileLetter(loaded.codingAgentProfiles.defaultProfileByAgent[targetName()])
      : null;
    const requested = currentRequested ?? acDefault ?? "A";
    setSelectedProfile(requested);
    setInitialProfileShouldLaunch(Boolean(currentRequested) || Boolean(acDefault));
    if (isWgReplica()) {
      refreshRemovePreviews();
      void refreshSelectionDefault();
    }
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

  /** #2051 - a replica apply never restarts through this toggle (#537, the
   *  post-assign prompt owns it), so the replica preview must hash the same
   *  `false` the apply sends. Bulk scopes keep the toggle. */
  const selectionRestart = (scope: ProfileAssignmentScope) =>
    scope === "replica" ? false : restartSessions();

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
      restartSessions: selectionRestart(scope),
      assignmentMode: assignmentMode(),
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
    selectedScope();
    // The operation is (scope, mode): switching to or from `+ lock` is a new
    // operation, so it re-previews and drops any reviewed conflict policy.
    assignmentMode();
    const agent = selectedAgent();
    const profile = selectedProfile();
    restartSessions();
    targetReplicaPath();

    previewSeqByScope.replica += 1;
    previewSeqByScope.kind += 1;
    previewSeqByScope.workgroup += 1;
    setDangerArmed(false);
    setConflictOpen(false);
    setConflictDecision(null);
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
    const withLock = assignmentMode() === "assignAndLock";
    if (scope === "replica") {
      return withLock ? "Assign + lock this replica" : "Assign to this replica";
    }
    const count = scopePreview()?.targetCount ?? 0;
    const wg = props.scopeContext?.workgroupName;
    const base =
      scope === "kind"
        ? `Overwrite ${count} of this kind`
        : `Overwrite ${count}${wg ? ` in ${wg}` : " in this room"}`;
    // The label states the policy: never a silent upgrade to lock or downgrade.
    return withLock ? `${base} + lock` : base;
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
    // #1943 - writing the SAME pair while ALSO setting the lock is a real change,
    // so the #551 no-op guard must not swallow it.
    if (assignmentMode() === "assignAndLock") return false;
    if (props.targetProfileOutdated) return false;
    const agent = selectedAgent();
    const baselineAgentId = props.explicitCurrentAgentId;
    if (!agent || !baselineAgentId) return false;
    if (agent.id !== baselineAgentId) return false;
    if (!profileTouched()) return true;
    return selectedProfile() === currentProfileLetter();
  });

  /** A backend-owned mutation is in flight. Every mutating control waits for it;
   *  #1942 keeps the promise open until the operation really settles. */
  const mutating = createMemo(() => busy() || removeBusy() || defaultBusy());

  /** The persisted protection snapshot. `null` means this build did not report
   *  one: unknown, never downgraded to unlocked. */
  const persistedLockState = createMemo<SelectionState | null>(
    () => props.scopeContext?.selectionState ?? null
  );
  const lockStateUsable = createMemo(
    () => persistedLockState() === "locked" || persistedLockState() === "unlocked"
  );

  const applyEnabled = createMemo(() => {
    // ANY backend mutation in flight blocks a second one, not just this button's
    // own apply: a removal or a default save is an owned operation too.
    if (mutating() || profileResolving() || !selectedAgent()) return false;
    // A `+ lock` operation needs an established protection state. When the state
    // stops being usable the operation is BLOCKED here rather than silently
    // demoted to an ordinary assignment, which would change the chosen policy.
    if (assignmentMode() === "assignAndLock" && !lockStateUsable()) return false;
    const scope = selectedScope();
    if (scope === "replica") {
      // #2051 - the fingerprint IS that confirmation, so the lock button waits
      // for its own preview exactly like the bulk scopes below.
      if (assignmentMode() === "assignAndLock" && (scopePreviewBusy() || !scopePreview())) {
        return false;
      }
      return !isRedundantReplicaSelection();
    }
    if (!isWgReplica()) return false;
    if (scopePreviewBusy() || !scopePreview()) return false;
    if (scope === "workgroup" || scope === "kind") return dangerArmed();
    return false;
  });

  // ── #1943 selection lock: persisted state, removal scope, Matrix default ──

  const persistedPair = createMemo<SavedPair | null>(() => props.scopeContext?.savedPair ?? null);
  const lockStateDiagnostic = createMemo(() => {
    const state = persistedLockState();
    if (state === "invalid") {
      const detail = props.scopeContext?.selectionError;
      return detail
        ? `Protection state invalid: ${detail}`
        : "Protection state invalid for this replica. Lock operations stay disabled until it is readable.";
    }
    if (state === null) {
      return "Protection state was not reported for this replica. Lock operations stay disabled; assignment still works.";
    }
    return "";
  });

  /** SAVED provider label and requested profile, falling back to the raw id when
   *  the provider is no longer configured. */
  const savedPairLabel = (saved: SavedPair | null | undefined): string => {
    const provider = saved?.codingAgentId
      ? settings()?.agents.find((a) => a.id === saved.codingAgentId)?.label ?? saved.codingAgentId
      : null;
    const profile = saved?.requestedProfile
      ? profileLabel(saved.requestedProfile, saved.codingAgentId)
      : null;
    if (!provider && !profile) return "Pair unavailable";
    return [provider, profile ? `Profile ${profile}` : null].filter(Boolean).join(" · ");
  };
  const persistedPairLabel = createMemo(() => savedPairLabel(persistedPair()));

  // Removal: its own scope, its own previews and counts.
  const removePreview = createMemo(() => removePreviews()[removeScope()]);
  const removeProtectedCount = createMemo(() => removePreview()?.protectedCount ?? 0);
  const removeCountsComplete = createMemo(() => removePreview()?.countsComplete === true);
  const removeInvalidCount = createMemo(() => removePreview()?.invalidCount ?? 0);
  const removeScopeCountLabel = (scope: ProfileAssignmentScope): string => {
    const preview = removePreviews()[scope];
    if (!preview) return removePreviewBusyMap()[scope] ? "…" : "—";
    if (!preview.countsComplete) return "count unknown";
    return scope === "replica"
      ? `${preview.protectedCount} protected`
      : `${preview.protectedCount} of ${preview.candidateCount} protected`;
  };
  const removeStateChip = createMemo(() => {
    const preview = removePreview();
    const persisted = persistedLockState();
    if (persisted === "invalid" || removeInvalidCount() > 0) {
      return { state: "invalid", label: "Check state" };
    }
    if (!preview) {
      return { state: "unknown", label: removePreviewBusyMap()[removeScope()] ? "Checking…" : "Unknown" };
    }
    // An incomplete enumeration must never claim "0 of N".
    if (!preview.countsComplete) return { state: "unknown", label: "Protection unknown" };
    // A claim about the FOCUSED replica needs an ESTABLISHED persisted state: an
    // unreported state must never be drawn as the reassuring "Unlocked" while
    // the adjacent diagnostic says the state was not reported.
    if (removeScope() === "replica" && persisted === null) {
      return { state: "unknown", label: "State unknown" };
    }
    if (removeScope() === "replica") {
      return preview.protectedCount > 0
        ? { state: "locked", label: "Protected" }
        : { state: "open", label: "Unlocked" };
    }
    // A bulk count is not a claim about the focused replica, so it stays a count.
    return {
      state: preview.protectedCount > 0 ? "locked" : "open",
      label: `${preview.protectedCount} of ${preview.candidateCount} protected`,
    };
  });
  const removeLabel = createMemo(() => {
    if (!removeCountsComplete() || removeProtectedCount() === 0) return "Nothing to remove";
    if (removeScope() === "replica") return "Remove lock";
    return `Remove lock from ${removeProtectedCount()} ${
      removeProtectedCount() === 1 ? "replica" : "replicas"
    }`;
  });
  const removeNote = createMemo(() => {
    const failure = removePreviewErrorMap()[removeScope()];
    if (failure) return failure;
    if (!removeCountsComplete()) return "Scope totals could not be established here; nothing is offered for removal.";
    return removeProtectedCount() > 0
      ? "Keeps Coding Agent + Profile. No restart."
      : "No protected replicas in this scope — nothing to remove.";
  });
  /** Independent of the focused replica: an unlocked focus never disables a
   *  scope whose own preview found protected peers. */
  const canRemove = createMemo(
    () => !mutating() && lockStateUsable() && removeCountsComplete() && removeProtectedCount() > 0
  );
  const lockHint = createMemo(() => {
    if (removeScope() !== "replica") {
      return "Only protection changes here: pairs, sessions and the future default stay untouched.";
    }
    return removeProtectedCount() > 0
      ? "Bulk assignments and restarts skip it. Removing the lock keeps the pair and does not restart."
      : 'Bulk assignments may overwrite this pair. Use "+ lock" below to write and protect in one step.';
  });
  const removeDone = createMemo(() => {
    const result = removeResult();
    return result ? removalOutcomeMessage(result) : "";
  });

  // Conflict review for a bulk assign-and-lock that found protected replicas.
  /** The six radios are ONE selection: a scope either without or with the lock. */
  const isOrdinaryScope = (scope: ProfileAssignmentScope) =>
    selectedScope() === scope && assignmentMode() === "ordinary";
  const isLockScope = (scope: ProfileAssignmentScope) =>
    selectedScope() === scope && assignmentMode() === "assignAndLock";
  const conflictEffectText = (decision: ConflictDecision): string => {
    const projection = conflictProjection(decision);
    if (!projection) return "The backend re-checks the scope before writing anything.";
    if (decision === "unlockedOnly") {
      return `${projection.eligibleCount} updated + locked; ${projection.skippedLockedCount} protected stay untouched.`;
    }
    return `${projection.eligibleCount} updated + locked; the ${conflictCount()} protected keep their lock.`;
  };
  const lockedKindTargets = createMemo(() =>
    (scopePreview()?.targets ?? []).filter((t) => t.selectionState === "locked")
  );
  const conflictCount = createMemo(() => {
    if (assignmentMode() !== "assignAndLock" || selectedScope() === "replica") return 0;
    return scopePreview()?.conflictCount ?? 0;
  });
  const conflictProjection = (decision: ConflictDecision) => {
    const projections = scopePreview()?.decisions ?? null;
    if (!projections) return null;
    return decision === "unlockedOnly" ? projections.unlockedOnly : projections.forceReviewed;
  };

  // The Matrix default for FUTURE replicas: persisted snapshot vs. user draft.
  const persistedDefault = createMemo(() => selectionDefault()?.default ?? null);
  const persistedDefaultLabel = createMemo(() => {
    const stored = persistedDefault();
    if (!stored) return "No Matrix default stored yet — Save default writes the pair below.";
    const provider =
      settings()?.agents.find((a) => a.id === stored.codingAgentId)?.label ?? stored.codingAgentId;
    const profile = profileLabel(stored.requestedProfile, stored.codingAgentId);
    return `Pair used at creation: ${provider} · Profile ${profile}${
      stored.selectionLocked ? " · Start locked" : " · Start unlocked"
    }`;
  });
  const canSaveDefault = createMemo(
    () => !mutating() && lockStateUsable() && !!selectedAgent() && !!selectionDefault()
  );

  const refreshSelectionDefault = async () => {
    const target = targetReplicaPath();
    if (!target || !isWgReplica()) return;
    const seq = ++defaultSeq;
    setDefaultBusy(true);
    try {
      const result = await SettingsAPI.getReplicaSelectionDefault({ targetReplicaPath: target });
      if (seq !== defaultSeq) return;
      setSelectionDefault(result);
      // Deliberately does NOT clear `defaultNotice`: a failed save tells the user
      // to review the refreshed default, so the refresh must not erase the ask.
      if (!defaultDraftSeeded) {
        defaultDraftSeeded = true;
        setDefaultDraftLocked(result.default?.selectionLocked ?? false);
      }
    } catch (err: unknown) {
      if (seq !== defaultSeq) return;
      setSelectionDefault(null);
      setDefaultNotice(launchErrorMessage(err));
    } finally {
      if (seq === defaultSeq) setDefaultBusy(false);
    }
  };

  const runRemovePreview = (scope: ProfileAssignmentScope) => {
    const target = targetReplicaPath();
    if (!target || !isWgReplica()) return;
    const seq = ++removeSeqByScope[scope];
    setRemovePreviewBusyMap((prev) => ({ ...prev, [scope]: true }));
    setRemovePreviewErrorMap((prev) => ({ ...prev, [scope]: "" }));
    SettingsAPI.previewSelectionLockRemoval({ targetReplicaPath: target, scope })
      .then((result) => {
        if (seq !== removeSeqByScope[scope]) return;
        setRemovePreviews((prev) => ({ ...prev, [scope]: result }));
      })
      .catch((err: unknown) => {
        if (seq !== removeSeqByScope[scope]) return;
        setRemovePreviews((prev) => ({ ...prev, [scope]: null }));
        setRemovePreviewErrorMap((prev) => ({ ...prev, [scope]: launchErrorMessage(err) }));
      })
      .finally(() => {
        if (seq === removeSeqByScope[scope]) {
          setRemovePreviewBusyMap((prev) => ({ ...prev, [scope]: false }));
        }
      });
  };

  const refreshRemovePreviews = () => {
    for (const scope of LOCK_SCOPES) runRemovePreview(scope);
  };

  /** Removal carries no pair and no restart: it only clears protection. */
  const removeLock = async () => {
    const target = targetReplicaPath();
    const preview = removePreview();
    if (!canRemove() || !target || !preview) return;
    setRemoveBusy(true);
    setRemoveErrors([]);
    setRemoveResult(null);
    try {
      const result = await SettingsAPI.applySelectionLockRemoval({
        targetReplicaPath: target,
        scope: removeScope(),
        confirmedTargetFingerprint: preview.targetFingerprint,
      });
      setRemoveResult(result);
      setRemoveErrors(result.errors);
      const first = result.errors[0];
      if (first) {
        showToast(
          result.errors.length > 1 ? `${first.message} (+${result.errors.length - 1} more)` : first.message,
        );
      } else {
        showToast(removalOutcomeMessage(result));
      }
    } catch (err: unknown) {
      // A dropped reply must not be read as "nothing happened".
      const message = `Outcome unknown: ${launchErrorMessage(err)}. The lock may still have been removed; the scope will refresh.`;
      setRemoveErrors([
        { code: "removalOutcomeUnknown", message, sessionIds: [], replicaPaths: [] },
      ]);
      showToast(message);
    } finally {
      setRemoveBusy(false);
      // Re-read the authoritative state instead of mutating optimistically.
      refreshRemovePreviews();
      void refreshSelectionDefault();
    }
  };

  const saveSelectionDefault = async () => {
    const target = targetReplicaPath();
    const agent = selectedAgent();
    const current = selectionDefault();
    if (!canSaveDefault() || !target || !agent || !current) return;
    setDefaultBusy(true);
    setDefaultNotice("");
    try {
      const result = await SettingsAPI.setReplicaSelectionDefault({
        targetReplicaPath: target,
        codingAgentId: agent.id,
        requestedProfile: selectedProfile(),
        selectionLocked: defaultDraftLocked(),
        confirmedDefaultFingerprint: current.defaultFingerprint,
      });
      setSelectionDefault(result);
      defaultDraftSeeded = true;
      setDefaultDraftLocked(result.default?.selectionLocked ?? defaultDraftLocked());
      showToast("Default for new replicas saved.");
    } catch (err: unknown) {
      // Keep the stored view AND the draft; the retry needs a fresh review.
      const message = launchErrorMessage(err);
      setDefaultNotice(`${message} Review the refreshed default before retrying.`);
      showToast(message);
      void refreshSelectionDefault();
    } finally {
      setDefaultBusy(false);
    }
  };

  const openConflictReview = () => {
    setConflictDecision(null);
    setConflictOpen(true);
  };

  /** Cancel/Escape: the review closes and nothing at all is sent. */
  const cancelConflict = () => {
    setConflictOpen(false);
    setConflictDecision(null);
  };

  const resolveConflict = (decision: ConflictDecision) => {
    setConflictDecision(decision);
    setConflictOpen(false);
    // The reviewed policy is consumed by this one apply: a later force needs a
    // fresh review, so the decision never survives the request.
    void apply().finally(() => setConflictDecision(null));
  };

  /** An external selection update invalidates reviews and snapshots alike. */
  const handleExternalSelectionUpdate = () => {
    setConflictOpen(false);
    setConflictDecision(null);
    setDangerArmed(false);
    refreshRemovePreviews();
    void refreshSelectionDefault();
    const agent = selectedAgent();
    if (agent) {
      // Every scope's count is on screen at once, so all three are reloaded.
      for (const scope of LOCK_SCOPES) runScopePreview(scope, agent.id, selectedProfile());
    }
  };

  const apply = async () => {
    const agent = selectedAgent();
    if (!agent || !applyEnabled()) return;
    const scope = selectedScope();
    const mode = assignmentMode();
    // A bulk assign-and-lock that found protected replicas cannot proceed on a
    // guess: pick one of the two reviewed outcomes first.
    if (
      mode === "assignAndLock" &&
      scope !== "replica" &&
      conflictCount() > 0 &&
      !conflictDecision()
    ) {
      openConflictReview();
      return;
    }
    const decision = conflictDecision();
    setBusy(true);
    setError("");
    setApplyErrors([]);
    const requested = requestedProfileForSelection();
    const effective = effectivePreview().effectiveProfile;
    const target = targetReplicaPath();
    const restart = selectionRestart(scope);
    // Each reviewed policy carries its OWN backend-issued fingerprint; the
    // no-conflict path uses the preview's direct fingerprint.
    const reviewedFingerprint = decision
      ? conflictProjection(decision)?.fingerprint ?? null
      : null;
    // #2051 - "This replica + lock" is confirmed by the replica-scope preview's
    // own fingerprint; the legacy replica ordinary apply deliberately sends none
    // (the backend still accepts a missing fingerprint for replica + ordinary).
    const previewFingerprint = scopePreview()?.targetFingerprint ?? null;
    const confirmedFingerprint =
      scope === "replica"
        ? mode === "assignAndLock"
          ? previewFingerprint
          : null
        : reviewedFingerprint ?? previewFingerprint;
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
          confirmedTargetFingerprint: confirmedFingerprint,
          typedConfirmation: null,
          assignmentMode: mode,
          // Deliberately omitted unless a policy was actually reviewed: replica
          // scope and ordinary mode must never carry a decision.
          ...(decision ? { conflictDecision: decision } : {}),
        });
        if (result.errors.length > 0) {
          setApplyErrors(result.errors);
          const firstError = result.errors[0];
          const extra = result.errors.length - 1;
          showToast(extra > 0 ? `${firstError.message} (+${extra} more)` : firstError.message);
          setDangerArmed(false);
          setConflictDecision(null);
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
      setConflictDecision(null);
      // #2051 - a rejected replica + lock needs the same fresh review the bulk
      // scopes get; the old fingerprint must never be replayed. Replica ordinary
      // keeps today's behavior.
      const needsFreshReview = scope !== "replica" || mode === "assignAndLock";
      if (needsFreshReview && target && isWgReplica()) {
        setDangerArmed(false);
        setScopePreviews((prev) => ({ ...prev, [scope]: null }));
        runScopePreview(scope, agent.id, selectedProfile());
      }
    } finally {
      // The backend-owned mutation has settled. Releasing the gate here (instead
      // of only on the error paths) keeps "busy until the promise settles" true
      // for callers that leave the modal open after a successful apply.
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
      // Escape backs out of an open review first; either way nothing is sent.
      if (conflictOpen()) {
        cancelConflict();
        return;
      }
      props.onClose();
      return;
    }
    // #2014 - keys typed in the agent filter must never move the profile
    // (ArrowLeft/Right) or the selection (ArrowUp/Down). Escape still closes.
    if (
      e.target instanceof HTMLElement &&
      e.target.id === "agentPickerAgentFilter" &&
      e.key.startsWith("Arrow")
    ) {
      return;
    }
    const list = orderedAgents();
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
            {/* #2014 - the requested filter, immediately above the first Coding
                Agent card. It hides non-matching cards only; the comparison
                panel always keeps every row and nothing here assigns. */}
            <Show when={orderedAgents().length > 0}>
              <div class="agent-profile-provider-filter">
                <label for="agentPickerAgentFilter">Filter by name or start line</label>
                <input
                  id="agentPickerAgentFilter"
                  type="search"
                  autocomplete="off"
                  spellcheck={false}
                  placeholder="name or command + args"
                  aria-controls="agentPickerAgentList"
                  value={agentFilter()}
                  onInput={(e) => setAgentFilter(e.currentTarget.value)}
                  data-ac-testid="agentPicker.agentFilter"
                />
                <div
                  class="agent-profile-provider-filter-status"
                  role="status"
                  aria-live="polite"
                  data-ac-testid="agentPicker.agentFilterStatus"
                >
                  {filterQuery() === ""
                    ? `${orderedAgents().length} agents`
                    : visibleAgentCount() === 0
                    ? `No coding agent matches "${agentFilter().trim()}". Clear the filter to see all ${orderedAgents().length}.`
                    : `${visibleAgentCount()} of ${orderedAgents().length} agents match "${agentFilter().trim()}".`}
                </div>
              </div>
            </Show>
            <div
              id="agentPickerAgentList"
              class="agent-profile-provider-list"
              aria-label="Coding agent choices"
              data-component="Coding agent selector"
              {...automationAttrs("agentPicker.providers", "list")}
            >
              <Show
                when={orderedAgents().length > 0}
                fallback={<div class="agent-modal-empty">No agents configured. Add agents in Settings.</div>}
              >
                <For each={orderedAgents()}>
                  {(agent, i) => {
                    const defaultPreview = () => providerDefaultPreview(agent);
                    const active = () => i() === highlightIndex();
                    return (
                      <Show when={matchesFilter(agent)}>
                        <div
                          class="agent-profile-provider-card-wrap"
                          data-ac-testid={`agentPicker.providerWrap.${agent.id}`}
                        >
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
                          {/* #2306 - adjacent move controls are SEPARATE buttons beside
                              the card (no nested buttons); the filter disables both. */}
                          <div class="agent-profile-provider-moves">
                            <button
                              type="button"
                              class="settings-row-btn"
                              disabled={moveUpDisabled(i())}
                              onClick={() => void moveAgent(agent, "up")}
                              title={moveControlTitle(agent, "up")}
                              aria-label={moveControlLabel(agent, "up")}
                              data-ac-testid={moveControlTestId(agent.id, "up")}
                              data-ac-role="button"
                            >
                              {"\u2191"}
                            </button>
                            <button
                              type="button"
                              class="settings-row-btn"
                              disabled={moveDownDisabled(i())}
                              onClick={() => void moveAgent(agent, "down")}
                              title={moveControlTitle(agent, "down")}
                              aria-label={moveControlLabel(agent, "down")}
                              data-ac-testid={moveControlTestId(agent.id, "down")}
                              data-ac-role="button"
                            >
                              {"\u2193"}
                            </button>
                          </div>
                        </div>
                      </Show>
                    );
                  }}
                </For>
              </Show>
            </div>
            <Show when={moveError()}>
              <div
                class="agent-picker-error"
                role="status"
                aria-live="polite"
                data-ac-testid="agentPicker.moveError"
              >
                {moveError()}
              </div>
            </Show>
            <div
              role="status"
              aria-live="polite"
              data-ac-testid="agentPicker.moveStatus"
            >
              {moveAnnouncement()}
            </div>
            <Show when={overlayOwnsAgents()}>
              <div data-ac-testid="agentPicker.overlayReason">{MOVE_OVERLAY_REASON}</div>
            </Show>
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
                          <span
                            class="agent-comparison-agent-sub"
                            data-ac-testid={`agentPicker.comparison.row.${row.agent.id}.launchLine`}
                          >
                            {row.launchLine || "none"}
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

        {/* #1943 - the persisted selection lock. The pair shown is the STORED
            one, never the unsaved picker selection, and the `Remove lock from`
            scope keeps its own state, previews and counts. */}
        <Show when={showBroadScope()}>
          <div
            class="selection-lock-bar"
            data-state={removeProtectedCount() > 0 ? "locked" : "open"}
            data-ac-testid="agentPicker.lockBar"
          >
            <div class="selection-lock-icon">
              <LockIcon />
            </div>
            <div class="selection-lock-main">
              <div class="selection-lock-title">
                Selection lock
                <span
                  class="selection-lock-state"
                  data-state={removeStateChip().state}
                  data-ac-testid="agentPicker.lockState"
                  data-ac-role="status"
                >
                  {removeStateChip().label}
                </span>
              </div>
              <div class="selection-lock-pair" data-ac-testid="agentPicker.lockPair">
                {persistedPairLabel()}
              </div>
              <div class="selection-lock-hint">{lockHint()}</div>
            </div>
            <div class="selection-lock-right">
              <div class="selection-lock-remove-head">Remove lock from</div>
              <div
                class="selection-lock-remove-scopes"
                role="radiogroup"
                aria-label="Remove lock scope"
              >
                <For each={LOCK_SCOPES}>
                  {(scope) => (
                    <label
                      class="selection-lock-scope-opt"
                      classList={{
                        active: removeScope() === scope,
                        empty: (removePreviews()[scope]?.protectedCount ?? 0) === 0,
                      }}
                      {...automationAttrs(
                        `agentPicker.removeScope.${LOCK_SCOPE_TEST_ID[scope]}`,
                        "button",
                        removeScope() === scope ? "active" : "inactive",
                      )}
                    >
                      <input
                        type="radio"
                        name="agentPickerRemoveScope"
                        checked={removeScope() === scope}
                        onChange={() => setRemoveScope(scope)}
                      />
                      {LOCK_SCOPE_LABEL[scope]}{" "}
                      <span
                        class="selection-lock-scope-count"
                        data-ac-testid={`agentPicker.removeScopeCount.${LOCK_SCOPE_TEST_ID[scope]}`}
                      >
                        {removeScopeCountLabel(scope)}
                      </span>
                    </label>
                  )}
                </For>
              </div>
              <div class="selection-lock-remove-row">
                <span class="selection-lock-remove-note" data-ac-testid="agentPicker.removeNote">
                  {removeNote()}
                </span>
                <button
                  type="button"
                  class="selection-lock-remove"
                  disabled={!canRemove()}
                  onClick={() => void removeLock()}
                  {...automationAttrs(
                    "agentPicker.removeLock",
                    "button",
                    canRemove() ? "enabled" : "disabled",
                  )}
                >
                  {removeLabel()}
                </button>
              </div>
              <Show when={removeDone()}>
                <div
                  class="selection-lock-remove-done"
                  data-ac-testid="agentPicker.removeDone"
                  data-ac-role="status"
                >
                  {removeDone()}
                </div>
              </Show>
            </div>
          </div>

          {/* Unknown/invalid protection is a diagnostic, never a silent
              "unlocked", and it keeps the new lock controls disabled. */}
          <Show when={lockStateDiagnostic()}>
            <div
              class="agent-scope-warnings"
              data-ac-testid="agentPicker.lockDiagnostic"
              data-ac-role="status"
            >
              {lockStateDiagnostic()}
            </div>
          </Show>
          <Show when={removeErrors().length > 0}>
            <div
              class="agent-scope-error"
              data-ac-testid="agentPicker.removeErrors"
              data-ac-role="alert"
            >
              <For each={removeErrors()}>{(error) => <div>{error.message}</div>}</For>
            </div>
          </Show>

          {/* Informational only: protected rows, while the counts and the
              eligible set keep using the complete target list. */}
          <Show when={selectedScope() === "kind" && scopePreview()}>
            <div class="selection-lock-kind" data-ac-testid="agentPicker.lockKindList">
              <div class="selection-lock-kind-head">
                {scopePreview()!.targetCount} replica(s) of this kind ·{" "}
                {distinctWorkgroupCount()} room(s) · every row shows its own pair
              </div>
              <For each={lockedKindTargets()}>
                {(t) => (
                  <div
                    class="selection-lock-kind-row"
                    data-ac-role="row"
                    data-ac-replica-path={t.replicaPath}
                  >
                    <span class="wg">{t.workgroupName}</span>
                    <span class="name">{t.replicaName}</span>
                    <span class="pair">{savedPairLabel(t.savedPair)}</span>
                    <span class="selection-lock-pill locked">Protected</span>
                  </div>
                )}
              </For>
              <div class="selection-lock-kind-note">
                Apply to and + lock act on every eligible row; protected rows are resolved by the
                conflict dialog. New replicas inherit this Matrix default.
              </div>
            </div>
          </Show>

          {/* The Matrix default for FUTURE replicas: stored snapshot above, the
              user's unsaved draft beside it. Nothing saves on change. */}
          <Show when={selectionDefault() || defaultNotice()}>
            <div class="selection-lock-future" data-ac-testid="agentPicker.defaultSection">
              <div class="selection-lock-future-head">
                Default for new replicas of this Matrix
                <Show when={persistedDefault()}>
                  <span class="decision-tag">saved</span>
                </Show>
                <label class="selection-lock-scope-opt active" style={{ "margin-left": "auto" }}>
                  <input
                    type="checkbox"
                    checked={defaultDraftLocked()}
                    disabled={!lockStateUsable() || defaultBusy()}
                    onChange={(e) => setDefaultDraftLocked(e.currentTarget.checked)}
                    {...automationAttrs(
                      "agentPicker.defaultStartLocked",
                      "checkbox",
                      defaultDraftLocked() ? "checked" : "unchecked",
                    )}
                  />
                  Start locked
                </label>
              </div>
              <div class="selection-lock-kind-row">
                <span class="pair" data-ac-testid="agentPicker.defaultPersisted">
                  {persistedDefaultLabel()}
                </span>
              </div>
              <div class="selection-lock-future-note">
                Inherited by replicas created in future rooms of this team. Existing replicas keep
                their current lock state; changing this default never propagates, never assigns and
                never restarts.
              </div>
              <div class="selection-lock-remove-row">
                <Show when={defaultNotice()}>
                  <span
                    class="selection-lock-remove-note"
                    data-ac-testid="agentPicker.defaultNotice"
                    data-ac-role="alert"
                  >
                    {defaultNotice()}
                  </span>
                </Show>
                <button
                  type="button"
                  class="selection-lock-remove"
                  disabled={!canSaveDefault()}
                  onClick={() => void saveSelectionDefault()}
                  {...automationAttrs(
                    "agentPicker.defaultSave",
                    "button",
                    canSaveDefault() ? "enabled" : "disabled",
                  )}
                >
                  Save default
                </button>
              </div>
            </div>
          </Show>
        </Show>

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
            {/* Six radios, one mutually exclusive operation: the ordinary scopes
                the picker always had, then the same scopes with `+ lock`. */}
            <div
              class="agent-scope-stack"
              role="radiogroup"
              aria-label="Apply to, optionally with lock"
            >
              <div class="agent-scope-picker">
                <span class="agent-scope-label">Apply to</span>
                <label
                  class="agent-scope-opt"
                  classList={{ active: isOrdinaryScope("replica") }}
                  {...automationAttrs("agentPicker.scope.replica", "button", isOrdinaryScope("replica") ? "active" : "inactive")}
                >
                  <input
                    type="radio"
                    name="agentPickerScope"
                    checked={isOrdinaryScope("replica")}
                    onChange={() => {
                      setAssignmentMode("ordinary");
                      setSelectedScope("replica");
                    }}
                  />
                  This replica <span class="agent-scope-count">{scopeCount("replica")} replica</span>
                </label>
                <label
                  class="agent-scope-opt"
                  classList={{ active: isOrdinaryScope("kind"), dangerous: isOrdinaryScope("kind") }}
                  {...automationAttrs("agentPicker.scope.kind", "button", isOrdinaryScope("kind") ? "active" : "inactive")}
                >
                  <input
                    type="radio"
                    name="agentPickerScope"
                    checked={isOrdinaryScope("kind")}
                    onChange={() => {
                      setAssignmentMode("ordinary");
                      setSelectedScope("kind");
                    }}
                  />
                  All replicas of this kind <span class="agent-scope-count">{scopeCount("kind")} replicas</span>
                </label>
                <label
                  class="agent-scope-opt"
                  classList={{ active: isOrdinaryScope("workgroup"), dangerous: isOrdinaryScope("workgroup") }}
                  {...automationAttrs("agentPicker.scope.workgroup", "button", isOrdinaryScope("workgroup") ? "active" : "inactive")}
                >
                  <input
                    type="radio"
                    name="agentPickerScope"
                    checked={isOrdinaryScope("workgroup")}
                    onChange={() => {
                      setAssignmentMode("ordinary");
                      setSelectedScope("workgroup");
                    }}
                  />
                  Entire room <span class="agent-scope-count">{scopeCount("workgroup")} replicas</span>
                </label>
              </div>
              <div class="agent-scope-picker agent-scope-picker--lock">
                <span class="agent-scope-label">
                  Apply to{" "}
                  <span class="agent-scope-lock-badge">
                    <LockIcon /> + lock
                  </span>
                </span>
                <For each={LOCK_SCOPES}>
                  {(scope) => (
                    <label
                      class="agent-scope-opt"
                      classList={{
                        active: isLockScope(scope),
                        dangerous: isLockScope(scope),
                        "locked-choice": true,
                      }}
                      {...automationAttrs(
                        `agentPicker.scope.lock.${LOCK_SCOPE_TEST_ID[scope]}`,
                        "button",
                        isLockScope(scope) ? "active" : "inactive",
                      )}
                    >
                      <input
                        type="radio"
                        name="agentPickerScope"
                        checked={isLockScope(scope)}
                        disabled={!lockStateUsable()}
                        onChange={() => {
                          setAssignmentMode("assignAndLock");
                          setSelectedScope(scope);
                        }}
                      />
                      <LockIcon class="agent-scope-opt-lock" />
                      {LOCK_SCOPE_LABEL[scope]} + lock{" "}
                      <span class="agent-scope-count">
                        {scopeCount(scope)} {scopeCount(scope) === 1 ? "replica" : "replicas"}
                      </span>
                    </label>
                  )}
                </For>
              </div>
            </div>
            <div class="agent-scope-live-note">
              <span class="agent-scope-live-tag">live</span>
              Counts are read from the current room; the backend re-enumerates targets before applying.
            </div>
            <div class="agent-scope-lock-assumption">
              One step: <strong>+ lock</strong> writes the pair and sets the lock on the same eligible
              replicas. Replicas already locked are listed by the conflict dialog before applying;{" "}
              <strong>Cancel</strong> changes nothing.
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
                {scopePreview()!.targetCount} replica(s) across {distinctWorkgroupCount()} room(s) ·{" "}
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
      {/* #1943 conflict review. A bulk assign-and-lock over protected replicas
          is decided here, never silently downgraded or upgraded; Cancel and
          Escape both send nothing at all. */}
      <Show when={conflictOpen() && scopePreview()}>
        <div class="lock-conflict-overlay" data-ac-testid="agentPicker.conflict">
          {/* #1943 P1 - decorative scrim only, deliberately WITHOUT a click
              handler: the approved v3 prototype has no dismissal here, and a
              click-only div is what raised Sonar typescript:S1082 (Reliability
              New Code B) plus S6848. Cancel and Escape own dismissal. */}
          <div class="lock-conflict-scrim" />
          <div
            class="lock-conflict-card"
            role="alertdialog"
            aria-modal="true"
            aria-labelledby="agentPickerConflictTitle"
          >
            <div class="lock-conflict-head">
              <div class="lock-conflict-title" id="agentPickerConflictTitle">
                {lockedKindTargets().length}{" "}
                {lockedKindTargets().length === 1 ? "replica is" : "replicas are"} already locked
              </div>
              <div class="lock-conflict-sub">
                {scopePreview()!.targets.length} replica(s) in scope for <strong>+ lock</strong> with{" "}
                {selectedAgent()?.label} · Profile {profileLabel(selectedProfile())}. Choose how to treat
                them.
              </div>
            </div>
            <div class="lock-conflict-list">
              <For each={lockedKindTargets()}>
                {(t) => (
                  <div
                    class="lock-conflict-row"
                    data-ac-role="row"
                    data-ac-replica-path={t.replicaPath}
                  >
                    <span class="lock-conflict-who">
                      <span class="wg">{t.workgroupName}</span> ·{" "}
                      <span class="name">{t.replicaName}</span>
                    </span>
                    <span class="lock-conflict-now">
                      Now: {savedPairLabel(t.savedPair)}{" "}
                      <span class="selection-lock-pill locked">Protected</span>
                    </span>
                    <span class="lock-conflict-arrow">&#8594;</span>
                    <span class="lock-conflict-next">
                      Requested: {selectedAgent()?.label} · Profile {profileLabel(selectedProfile())}
                    </span>
                  </div>
                )}
              </For>
            </div>
            <div class="lock-conflict-effects">
              <div class="lock-conflict-effect">
                <strong>Cancel</strong>
                <span>Nothing changes: no pair, no lock and no restart.</span>
              </div>
              <div class="lock-conflict-effect">
                <strong>Apply only to unlocked</strong>
                <span>{conflictEffectText("unlockedOnly")}</span>
              </div>
              <div class="lock-conflict-effect danger">
                <strong>Force all, including locked</strong>
                <span>{conflictEffectText("forceReviewed")}</span>
              </div>
            </div>
            <div class="lock-conflict-actions">
              <button
                type="button"
                class="modal-btn modal-btn-cancel"
                onClick={cancelConflict}
                {...automationAttrs("agentPicker.conflict.cancel", "button")}
              >
                Cancel
              </button>
              <button
                type="button"
                class="modal-btn modal-btn-save"
                onClick={() => resolveConflict("unlockedOnly")}
                {...automationAttrs("agentPicker.conflict.unlockedOnly", "button")}
              >
                Apply only to unlocked
              </button>
              <button
                type="button"
                class="modal-btn modal-btn-save danger"
                onClick={() => resolveConflict("forceReviewed")}
                {...automationAttrs("agentPicker.conflict.forceAll", "button")}
              >
                Force all, including locked
              </button>
            </div>
          </div>
        </div>
      </Show>
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
