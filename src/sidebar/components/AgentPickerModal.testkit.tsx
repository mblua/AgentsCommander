// Shared mocks, fixtures and helpers for the AgentPickerModal test files.
import { afterEach, beforeEach, vi } from "vitest";
import { render } from "solid-js/web";
import AgentPickerModal, {
  type AgentPickerScopeContext,
  type AgentPickerSelection,
} from "./AgentPickerModal";
import type {
  AgentConfig,
  AppSettings,
  ApplyCodingAgentProfileSelectionResult,
  ApplySelectionLockRemovalResult,
  CodingAgentProfileResolution,
  PreviewCodingAgentProfileSelectionResult,
  PreviewSelectionLockRemovalResult,
  ProfileAssignmentScope,
  ProfileAssignmentTarget,
  ReplicaSelectionDefaultResult,
  SavedPair,
  SelectionState,
} from "../../shared/types";
import { resolveProfilePreview } from "../../shared/profile-utils";
import { baseSettings } from "../../shared/testing/base-settings";

const hoistedSettingsApi = vi.hoisted(() => ({
  get: vi.fn(),
  moveCodingAgent: vi.fn(),
  reorderCodingAgent: vi.fn(),
  resolveCodingAgentProfile: vi.fn(),
  previewCodingAgentProfileSelection: vi.fn(),
  applyCodingAgentProfileSelection: vi.fn(),
  previewSelectionLockRemoval: vi.fn(),
  applySelectionLockRemoval: vi.fn(),
  getReplicaSelectionDefault: vi.fn(),
  setReplicaSelectionDefault: vi.fn(),
  onCodingAgentProfileSelectionUpdated: vi.fn(),
  onCodingAgentSettingsUpdated: vi.fn(),
}));

vi.mock("../../shared/ipc", async () => {
  // The pure move-order contract helpers are the real shared code, not a mock.
  const { assertCodingAgentMoveOrder, expectedCodingAgentMoveOrder } = await vi.importActual<
    typeof import("../../shared/ipc")
  >("../../shared/ipc");
  return {
    SettingsAPI: {
      get: hoistedSettingsApi.get,
      moveCodingAgent: hoistedSettingsApi.moveCodingAgent,
      reorderCodingAgent: hoistedSettingsApi.reorderCodingAgent,
      resolveCodingAgentProfile: hoistedSettingsApi.resolveCodingAgentProfile,
      previewCodingAgentProfileSelection: hoistedSettingsApi.previewCodingAgentProfileSelection,
      applyCodingAgentProfileSelection: hoistedSettingsApi.applyCodingAgentProfileSelection,
      previewSelectionLockRemoval: hoistedSettingsApi.previewSelectionLockRemoval,
      applySelectionLockRemoval: hoistedSettingsApi.applySelectionLockRemoval,
      getReplicaSelectionDefault: hoistedSettingsApi.getReplicaSelectionDefault,
      setReplicaSelectionDefault: hoistedSettingsApi.setReplicaSelectionDefault,
    },
    onCodingAgentProfileSelectionUpdated: hoistedSettingsApi.onCodingAgentProfileSelectionUpdated,
    onCodingAgentSettingsUpdated: hoistedSettingsApi.onCodingAgentSettingsUpdated,
    assertCodingAgentMoveOrder,
    expectedCodingAgentMoveOrder,
  };
});

// vi.hoisted values cannot be exported directly; tests use this alias.
export const mockSettingsApi = hoistedSettingsApi;

export const ORIGIN_AGENT_PATH =
  "C:\\Users\\maria\\0_repos\\AgentsCommander_ac\\.ac\\_agent_architect";
export const REPO_PATH = "C:\\work\\repo";
export const WG_REPLICA_PATH = "C:\\repos\\proj\\.ac\\wg-7-dev-team\\__agent_dev-webpage-ui";

export const WG_SCOPE_CONTEXT: AgentPickerScopeContext = {
  workgroupPath: "C:\\repos\\proj\\.ac\\wg-7-dev-team",
  workgroupName: "wg-7-dev-team",
  targetReplicaPath: WG_REPLICA_PATH,
  targetReplicaName: "dev-webpage-ui",
  currentCodingAgentId: "codex",
  currentProfile: "A",
  // The gray/default focus is an UNLOCKED replica whose SCOPE may still
  // hold protected peers, which is exactly the case the removal bar must keep
  // separate from the picker selection.
  savedPair: null,
  selectionState: "unlocked",
};

export const state = {} as { currentSettings: AppSettings };

export function agent(overrides: Partial<AgentConfig>): AgentConfig {
  return {
    id: "codex",
    label: "Codex",
    command: "codex",
    color: "#10b981",
    envs: [],
    isolatedHome: false,
    ...overrides,
  };
}

export function settings(overrides: Partial<AppSettings> = {}): AppSettings {
  return baseSettings({
    themeLight: true,
    soundsEnabled: true,
    teamIdleBeepEnabled: true,
    agents: [
      agent({
        id: "codex",
        label: "Codex",
        command: "codex",
        envs: [
          { key: "OPENAI_API_KEY", value: "redacted", source: "user", enabled: true },
          { key: "DISABLED_KEY", value: "x", source: "user", enabled: false },
        ],
      }),
      agent({
        id: "claude",
        label: "Claude Code",
        command: "claude",
        color: "#d97706",
      }),
    ],
    codingAgentProfiles: {
      schemaVersion: 2,
      profileSlots: {
        A: { label: "" },
        B: { label: "fast" },
        C: { label: "review" },
      },
      defaultProfileByAgent: { architect: "B" },
      profilesByAgent: {
        codex: {
          A: {
            enabled: true,
            command: "codex --model gpt-5",
            env: { OPENAI_MODEL: "gpt-5" },
            notes: "baseline",
          },
          B: {
            enabled: true,
            command: "codex --profile fast",
            env: { CODEX_PROFILE: "fast" },
            notes: "fast lane",
          },
        },
        claude: {
          A: {
            enabled: true,
            command: "claude --dangerously-skip-permissions",
            env: {},
            notes: "",
          },
        },
      },
      profileLabelsByAgent: {},
    },
    ...overrides,
    archivedProjectPaths: overrides.archivedProjectPaths ?? [],
  });
}

export function resolution(
  overrides: Partial<CodingAgentProfileResolution> = {},
): CodingAgentProfileResolution {
  return {
    requestedProfile: "A",
    effectiveProfile: "A",
    fallbackChain: ["A"],
    fallbackApplied: false,
    requestedProfileInput: null,
    instanceProfileOverride: null,
    originDefaultProfile: null,
    agentDefaultProfile: null,
    warnings: [],
    ...overrides,
  };
}

export function defaultBackendResolve(
  _agentPath: string | null,
  agentId: string,
  requestedProfile?: string | null,
): Promise<CodingAgentProfileResolution> {
  const requested =
    requestedProfile ??
    state.currentSettings.codingAgentProfiles.defaultProfileByAgent.architect ??
    "A";
  const preview = resolveProfilePreview(
    state.currentSettings.codingAgentProfiles,
    agentId,
    requested,
  );
  return Promise.resolve(
    resolution({
      ...preview,
      requestedProfileInput: requestedProfile ?? null,
      agentDefaultProfile: state.currentSettings.codingAgentProfiles.defaultProfileByAgent.architect ?? null,
    }),
  );
}

export function makeTarget(
  name: string,
  wg: string,
  liveSessions: string[],
  lock?: { savedPair?: SavedPair | null; selectionState?: SelectionState },
): ProfileAssignmentTarget {
  return {
    workgroupName: wg,
    workgroupPath: `C:\\repos\\proj\\.ac\\${wg}`,
    replicaName: name,
    replicaPath: `C:\\repos\\proj\\.ac\\${wg}\\__agent_${name}`,
    identityPath: `C:\\repos\\proj\\.ac\\${wg}\\__agent_${name}\\identity.json`,
    originProject: "proj",
    liveSessionIds: liveSessions,
    savedPair: lock?.savedPair ?? null,
    selectionState: lock?.selectionState ?? "unlocked",
  };
}

export function previewResult(
  overrides: Partial<PreviewCodingAgentProfileSelectionResult> = {},
): PreviewCodingAgentProfileSelectionResult {
  return {
    scope: "replica",
    targetCount: 1,
    liveSessionCount: 1,
    targetFingerprint: "fp-replica",
    requiresExplicitConfirmation: false,
    targets: [makeTarget("dev-webpage-ui", "wg-7-dev-team", ["sess-1"])],
    warnings: [],
    ...overrides,
  };
}

export function scopeAwarePreview(): void {
  mockSettingsApi.previewCodingAgentProfileSelection.mockImplementation(
    (req: { scope: string; assignmentMode?: string }) => {
      if (req.scope === "kind") {
        // The SAME scope changes meaning with `+ lock`, so the two
        // modes get different previews exactly as the backend would emit them.
        if (req.assignmentMode === "assignAndLock") {
          return Promise.resolve(kindLockPreview());
        }
        return Promise.resolve(
          previewResult({
            scope: "kind",
            targetCount: 3,
            liveSessionCount: 3,
            targetFingerprint: "fp-kind",
            requiresExplicitConfirmation: true,
            targets: [
              makeTarget("dev-webpage-ui", "wg-7-dev-team", ["sess-1", "sess-2"]),
              makeTarget("dev-webpage-ui", "wg-9-other", ["sess-3"]),
              makeTarget("dev-webpage-ui", "wg-12-more", []),
            ],
          }),
        );
      }
      if (req.scope === "workgroup") {
        return Promise.resolve(
          previewResult({
            scope: "workgroup",
            targetCount: 4,
            liveSessionCount: 2,
            targetFingerprint: "fp-wg",
            targets: [
              makeTarget("dev-webpage-ui", "wg-7-dev-team", ["sess-1"]),
              makeTarget("dev-rust", "wg-7-dev-team", ["sess-2"]),
              makeTarget("architect", "wg-7-dev-team", []),
              makeTarget("shipper", "wg-7-dev-team", []),
            ],
          }),
        );
      }
      return Promise.resolve(previewResult({ scope: "replica", targetFingerprint: "fp-replica" }));
    },
  );
}

// ── Selection-lock fixtures ──────────────────────────────────────────

/** Bulk `kind` with `+ lock` over three candidates, two of them protected - one
 *  of those already on the requested pair, which must STILL be listed. */
export function kindLockPreview(): PreviewCodingAgentProfileSelectionResult {
  const requested: SavedPair = { codingAgentId: "codex", requestedProfile: "A" };
  const unlocked = makeTarget("dev-webpage-ui", "wg-7-dev-team", ["sess-1"]);
  const equalPair = makeTarget("dev-webpage-ui", "wg-9-other", ["sess-2"], {
    savedPair: requested,
    selectionState: "locked",
  });
  const otherPair = makeTarget("dev-webpage-ui", "wg-12-more", [], {
    savedPair: { codingAgentId: "claude", requestedProfile: "A" },
    selectionState: "locked",
  });
  return previewResult({
    scope: "kind",
    targetCount: 3,
    liveSessionCount: 3,
    // Before a policy is chosen the top-level view IS the unlockedOnly outcome.
    targetFingerprint: "fp-unlocked-only",
    requiresExplicitConfirmation: true,
    targets: [unlocked, equalPair, otherPair],
    countsComplete: true,
    candidateCount: 3,
    protectedCount: 2,
    invalidCount: 0,
    conflictCount: 2,
    decisions: {
      unlockedOnly: {
        fingerprint: "fp-unlocked-only",
        eligiblePaths: [unlocked.replicaPath],
        eligibleCount: 1,
        skippedLockedCount: 2,
        liveSessionCount: 1,
      },
      forceReviewed: {
        fingerprint: "fp-force-all",
        eligiblePaths: [unlocked.replicaPath, equalPair.replicaPath, otherPair.replicaPath],
        eligibleCount: 3,
        skippedLockedCount: 0,
        liveSessionCount: 3,
      },
    },
  });
}

export function removePreview(
  scope: ProfileAssignmentScope,
  overrides: Partial<PreviewSelectionLockRemovalResult> = {},
): PreviewSelectionLockRemovalResult {
  const counts: Record<ProfileAssignmentScope, { candidateCount: number; protectedCount: number }> = {
    // The focused replica is UNLOCKED while the wider scopes still hold
    // protected peers: protection is counted per scope, never from the focus.
    replica: { candidateCount: 1, protectedCount: 0 },
    kind: { candidateCount: 3, protectedCount: 2 },
    workgroup: { candidateCount: 4, protectedCount: 3 },
  };
  const base = counts[scope];
  return {
    scope,
    targetFingerprint: `fp-remove-${scope}`,
    candidateCount: base.candidateCount,
    countsComplete: true,
    protectedCount: base.protectedCount,
    alreadyUnlockedCount: base.candidateCount - base.protectedCount,
    invalidCount: 0,
    targets: [],
    warnings: [],
    ...overrides,
  };
}

export function removalApplyResult(
  overrides: Partial<ApplySelectionLockRemovalResult> = {},
): ApplySelectionLockRemovalResult {
  return {
    scope: "replica",
    targetFingerprint: "fp-remove-replica",
    removedCount: 1,
    removedReplicaPaths: [WG_REPLICA_PATH],
    alreadyUnlockedPaths: [],
    failedReplicaPaths: [],
    remainingProtectedCount: 0,
    candidateCount: 1,
    countsComplete: true,
    invalidCount: 0,
    errors: [],
    warnings: [],
    ...overrides,
  };
}

export function defaultResult(
  overrides: Partial<ReplicaSelectionDefaultResult> = {},
): ReplicaSelectionDefaultResult {
  return {
    targetReplicaPath: WG_REPLICA_PATH,
    matrixPath: "C:\\repos\\proj\\.ac\\_agent_dev-webpage-ui",
    default: { codingAgentId: "codex", requestedProfile: "A", selectionLocked: false },
    defaultFingerprint: "fp-default-1",
    warnings: [],
    ...overrides,
  };
}

export function lockAwareRemovalPreviews(): void {
  mockSettingsApi.previewSelectionLockRemoval.mockImplementation(
    (req: { scope: ProfileAssignmentScope }) => Promise.resolve(removePreview(req.scope)),
  );
}

export function applyResult(
  overrides: Partial<ApplyCodingAgentProfileSelectionResult> = {},
): ApplyCodingAgentProfileSelectionResult {
  return {
    scope: "replica",
    updatedCount: 1,
    restartedCount: 0,
    updatedReplicaPaths: [WG_REPLICA_PATH],
    restartedSessionIds: [],
    destroyedButNotRecreatedSessionIds: [],
    targetFingerprint: "fp-replica",
    warnings: [],
    errors: [],
    ...overrides,
  };
}

export async function settle(times = 4): Promise<void> {
  for (let index = 0; index < times; index += 1) {
    await Promise.resolve();
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
  }
}

export function target<T extends HTMLElement = HTMLElement>(testId: string): T {
  const element = document.querySelector<T>(`[data-ac-testid="${testId}"]`);
  if (!element) throw new Error(`Missing test target: ${testId}`);
  return element;
}

export function maybe<T extends HTMLElement = HTMLElement>(testId: string): T | null {
  return document.querySelector<T>(`[data-ac-testid="${testId}"]`);
}

export function text(testId: string): string {
  return target(testId).textContent?.replace(/\s+/g, " ").trim() ?? "";
}

export function clickRadio(testId: string): void {
  const input = target(testId).querySelector<HTMLInputElement>("input");
  if (!input) throw new Error(`Missing radio input in ${testId}`);
  input.click();
}

export function renderPicker(
  overrides: Partial<{
    sessionName: string;
    agentPath: string | null;
    currentAgentId: string | null;
    explicitCurrentAgentId: string | null;
    currentRequestedProfile: string | null;
    scopeContext: AgentPickerScopeContext | undefined;
    disableRedundantReplicaAssign: boolean;
    targetProfileOutdated: boolean;
    onSelect: (selection: AgentPickerSelection) => void | Promise<void>;
    onClose: () => void;
  }> = {},
) {
  const root = document.createElement("div");
  const onSelect = vi.fn();
  const onClose = vi.fn();
  document.body.append(root);
  const dispose = render(
    () => (
      <AgentPickerModal
        sessionName={overrides.sessionName ?? "architect"}
        agentPath={overrides.agentPath === undefined ? ORIGIN_AGENT_PATH : overrides.agentPath}
        currentAgentId={overrides.currentAgentId ?? "codex"}
        // Defaults to the currentAgentId baseline (simulating an explicitly-assigned
        // replica / live session); the never-assigned case passes null explicitly.
        explicitCurrentAgentId={
          overrides.explicitCurrentAgentId !== undefined
            ? overrides.explicitCurrentAgentId
            : overrides.currentAgentId ?? "codex"
        }
        currentRequestedProfile={overrides.currentRequestedProfile}
        scopeContext={overrides.scopeContext}
        disableRedundantReplicaAssign={overrides.disableRedundantReplicaAssign}
        targetProfileOutdated={overrides.targetProfileOutdated}
        onSelect={overrides.onSelect ?? onSelect}
        onClose={overrides.onClose ?? onClose}
      />
    ),
    root,
  );
  return { dispose, onSelect, onClose };
}


/** The shared beforeEach/afterEach of the outer AgentPickerModal block. */
export function installPickerHooks(): void {
  beforeEach(() => {
    state.currentSettings = settings();
    mockSettingsApi.get.mockReset();
    mockSettingsApi.resolveCodingAgentProfile.mockReset();
    mockSettingsApi.previewCodingAgentProfileSelection.mockReset();
    mockSettingsApi.applyCodingAgentProfileSelection.mockReset();
    mockSettingsApi.previewSelectionLockRemoval.mockReset();
    mockSettingsApi.applySelectionLockRemoval.mockReset();
    mockSettingsApi.getReplicaSelectionDefault.mockReset();
    mockSettingsApi.setReplicaSelectionDefault.mockReset();
    mockSettingsApi.onCodingAgentProfileSelectionUpdated.mockReset();
    mockSettingsApi.onCodingAgentSettingsUpdated.mockReset();
    mockSettingsApi.moveCodingAgent.mockReset();
    mockSettingsApi.reorderCodingAgent.mockReset();
    mockSettingsApi.get.mockResolvedValue(state.currentSettings);
    mockSettingsApi.resolveCodingAgentProfile.mockImplementation(defaultBackendResolve);
    scopeAwarePreview();
    lockAwareRemovalPreviews();
    mockSettingsApi.applyCodingAgentProfileSelection.mockResolvedValue(applyResult());
    mockSettingsApi.applySelectionLockRemoval.mockResolvedValue(removalApplyResult());
    mockSettingsApi.getReplicaSelectionDefault.mockImplementation(() =>
      Promise.resolve(defaultResult()),
    );
    mockSettingsApi.setReplicaSelectionDefault.mockImplementation(() =>
      Promise.resolve(defaultResult()),
    );
    mockSettingsApi.onCodingAgentProfileSelectionUpdated.mockImplementation(() =>
      Promise.resolve(() => {}),
    );
    mockSettingsApi.onCodingAgentSettingsUpdated.mockImplementation(() =>
      Promise.resolve(() => {}),
    );
  });

  afterEach(() => {
    // Empties the body like the old cleanup; the raw-HTML sink scan reads this non-test file.
    document.body.replaceChildren();
    vi.clearAllMocks();
  });
}
