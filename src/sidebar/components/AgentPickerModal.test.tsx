// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createSignal } from "solid-js";
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

const mockSettingsApi = vi.hoisted(() => ({
  get: vi.fn(),
  moveCodingAgent: vi.fn(),
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
  // #2306 - the pure move-order contract helpers are the real shared code, not a mock.
  const { assertCodingAgentMoveOrder, expectedCodingAgentMoveOrder } = await vi.importActual<
    typeof import("../../shared/ipc")
  >("../../shared/ipc");
  return {
    SettingsAPI: {
      get: mockSettingsApi.get,
      moveCodingAgent: mockSettingsApi.moveCodingAgent,
      resolveCodingAgentProfile: mockSettingsApi.resolveCodingAgentProfile,
      previewCodingAgentProfileSelection: mockSettingsApi.previewCodingAgentProfileSelection,
      applyCodingAgentProfileSelection: mockSettingsApi.applyCodingAgentProfileSelection,
      previewSelectionLockRemoval: mockSettingsApi.previewSelectionLockRemoval,
      applySelectionLockRemoval: mockSettingsApi.applySelectionLockRemoval,
      getReplicaSelectionDefault: mockSettingsApi.getReplicaSelectionDefault,
      setReplicaSelectionDefault: mockSettingsApi.setReplicaSelectionDefault,
    },
    onCodingAgentProfileSelectionUpdated: mockSettingsApi.onCodingAgentProfileSelectionUpdated,
    onCodingAgentSettingsUpdated: mockSettingsApi.onCodingAgentSettingsUpdated,
    assertCodingAgentMoveOrder,
    expectedCodingAgentMoveOrder,
  };
});

const ORIGIN_AGENT_PATH = "C:\\Users\\maria\\0_repos\\AgentsCommander_ac\\.ac\\_agent_architect";
const REPO_PATH = "C:\\work\\repo";
const WG_REPLICA_PATH = "C:\\repos\\proj\\.ac\\wg-7-dev-team\\__agent_dev-webpage-ui";

const WG_SCOPE_CONTEXT: AgentPickerScopeContext = {
  workgroupPath: "C:\\repos\\proj\\.ac\\wg-7-dev-team",
  workgroupName: "wg-7-dev-team",
  targetReplicaPath: WG_REPLICA_PATH,
  targetReplicaName: "dev-webpage-ui",
  currentCodingAgentId: "codex",
  currentProfile: "A",
  // #1943 - the gray/default focus is an UNLOCKED replica whose SCOPE may still
  // hold protected peers, which is exactly the case the removal bar must keep
  // separate from the picker selection.
  savedPair: null,
  selectionState: "unlocked",
};

let currentSettings: AppSettings;

function agent(overrides: Partial<AgentConfig>): AgentConfig {
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

function settings(overrides: Partial<AppSettings> = {}): AppSettings {
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

function resolution(
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

function defaultBackendResolve(
  _agentPath: string | null,
  agentId: string,
  requestedProfile?: string | null,
): Promise<CodingAgentProfileResolution> {
  const requested =
    requestedProfile ?? currentSettings.codingAgentProfiles.defaultProfileByAgent.architect ?? "A";
  const preview = resolveProfilePreview(currentSettings.codingAgentProfiles, agentId, requested);
  return Promise.resolve(
    resolution({
      ...preview,
      requestedProfileInput: requestedProfile ?? null,
      agentDefaultProfile: currentSettings.codingAgentProfiles.defaultProfileByAgent.architect ?? null,
    }),
  );
}

function makeTarget(
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

function previewResult(
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

function scopeAwarePreview(): void {
  mockSettingsApi.previewCodingAgentProfileSelection.mockImplementation(
    (req: { scope: string; assignmentMode?: string }) => {
      if (req.scope === "kind") {
        // #1943 - the SAME scope changes meaning with `+ lock`, so the two
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

// ── #1943 selection-lock fixtures ──────────────────────────────────────────

/** Bulk `kind` with `+ lock` over three candidates, two of them protected - one
 *  of those already on the requested pair, which must STILL be listed. */
function kindLockPreview(): PreviewCodingAgentProfileSelectionResult {
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

function removePreview(
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

function removalApplyResult(
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

function defaultResult(
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

function lockAwareRemovalPreviews(): void {
  mockSettingsApi.previewSelectionLockRemoval.mockImplementation(
    (req: { scope: ProfileAssignmentScope }) => Promise.resolve(removePreview(req.scope)),
  );
}

function applyResult(
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

async function settle(times = 4): Promise<void> {
  for (let index = 0; index < times; index += 1) {
    await Promise.resolve();
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
  }
}

function target<T extends HTMLElement = HTMLElement>(testId: string): T {
  const element = document.querySelector<T>(`[data-ac-testid="${testId}"]`);
  if (!element) throw new Error(`Missing test target: ${testId}`);
  return element;
}

function maybe<T extends HTMLElement = HTMLElement>(testId: string): T | null {
  return document.querySelector<T>(`[data-ac-testid="${testId}"]`);
}

function text(testId: string): string {
  return target(testId).textContent?.replace(/\s+/g, " ").trim() ?? "";
}

function clickRadio(testId: string): void {
  const input = target(testId).querySelector<HTMLInputElement>("input");
  if (!input) throw new Error(`Missing radio input in ${testId}`);
  input.click();
}

function renderPicker(
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

describe("AgentPickerModal", () => {
  beforeEach(() => {
    currentSettings = settings();
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
    mockSettingsApi.get.mockResolvedValue(currentSettings);
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
    document.body.innerHTML = "";
    vi.clearAllMocks();
  });

  it("renders the selector regions and the scope-picker apply button", async () => {
    const { dispose } = renderPicker();
    await settle();

    expect(target("agentPicker.providers")).toBeTruthy();
    expect(target("agentPicker.profiles")).toBeTruthy();
    expect(target("agentPicker.comparison")).toBeTruthy();
    expect(target("agentPicker.cancel")).toBeTruthy();
    expect(target("agentPicker.apply")).toBeTruthy();
    expect(text("agentPicker.apply")).toContain("Assign to this replica");
    expect(target("agentPicker.provider.codex").getAttribute("data-ac-agent-id")).toBe("codex");
    expect(target("agentPicker.profile.A").getAttribute("data-ac-profile-letter")).toBe("A");

    dispose();
  });

  it("hides broad scope when no scope context is supplied", async () => {
    const { dispose } = renderPicker({ scopeContext: undefined });
    await settle();

    expect(maybe("agentPicker.scope.replica")).toBeNull();
    expect(maybe("agentPicker.scope.kind")).toBeNull();
    expect(maybe("agentPicker.scope.workgroup")).toBeNull();
    // Replica scope is implied; apply is enabled.
    expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

    dispose();
  });

  it("preserves the no-agent empty state with apply disabled", async () => {
    currentSettings = settings({ agents: [] });
    mockSettingsApi.get.mockResolvedValue(currentSettings);
    const { dispose } = renderPicker({ currentAgentId: null });
    await settle();

    expect(document.body.textContent).toContain("No agents configured. Add agents in Settings.");
    expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);

    dispose();
  });

  it("updates active provider state and the active comparison row when a coding agent is clicked", async () => {
    const { dispose } = renderPicker({ agentPath: REPO_PATH });
    await settle();

    target<HTMLButtonElement>("agentPicker.provider.claude").click();
    await settle();

    expect(target("agentPicker.provider.claude").getAttribute("data-ac-state")).toBe("active");
    expect(target("agentPicker.comparison.row.claude").getAttribute("data-ac-state")).toBe("active");
    expect(text("agentPicker.comparison.row.claude")).toContain("Claude Code");
    expect(text("agentPicker.comparison.row.claude")).toContain("A direct");
    expect(text("agentPicker.comparison.row.claude.launchLine")).toBe(
      "claude claude --dangerously-skip-permissions",
    );

    dispose();
  });

  it("shows declared env vars for the selected profile and keeps comparison compact", async () => {
    const { dispose } = renderPicker({ agentPath: REPO_PATH });
    await settle();

    target<HTMLButtonElement>("agentPicker.profile.B").click();
    await settle();

    expect(text("agentPicker.profile.B.env")).toContain("Declared env");
    expect(text("agentPicker.profile.B.env")).toContain("CODEX_PROFILE");
    expect(text("agentPicker.profile.B.env")).toContain("fast");
    expect(text("agentPicker.comparison.row.codex")).toContain("B-FAST direct");
    expect(target("agentPicker.comparison").querySelector("[data-ac-profile-letter]")).toBeNull();
    const comparisonText = text("agentPicker.comparison");
    expect(comparisonText).toContain("Same Profile In Other Agents");
    expect(comparisonText).not.toContain("Effective Projection");
    expect(comparisonText).not.toContain("Chosen pair");
    expect(comparisonText).not.toMatch(/Command Delta/i);
    expect(comparisonText).not.toMatch(/Env Summary/i);
    expect(comparisonText).not.toContain("2 env vars");
    expect(comparisonText).not.toContain("CODEX_PROFILE=fast");
    expect(maybe("agentPicker.fallback")).toBeNull();

    dispose();
  });

  it("#548: each provider chip resolves its OWN per-agent label, not the highlighted agent's", async () => {
    // codex (agents[0] = primigenio) and claude each get a distinct A label.
    const base = settings();
    currentSettings = settings({
      codingAgentProfiles: {
        ...base.codingAgentProfiles,
        profileLabelsByAgent: {
          codex: { A: "alpha-codex" },
          claude: { A: "alpha-claude" },
        },
      },
    });
    mockSettingsApi.get.mockResolvedValue(currentSettings);
    // Repo path → no backend resolution; each provider's default resolves to A.
    const { dispose } = renderPicker({ agentPath: REPO_PATH, currentAgentId: "codex" });
    await settle();

    const chip = (id: string) =>
      target(`agentPicker.provider.${id}`)
        .querySelector(".agent-profile-provider-chip")
        ?.textContent?.trim();

    // codex is the highlighted row. With the :568 bug, claude's chip would resolve
    // against codex and read "A-ALPHA-CODEX". Each must show its OWN A label.
    expect(chip("codex")).toBe("A-ALPHA-CODEX");
    expect(chip("claude")).toBe("A-ALPHA-CLAUDE");

    dispose();
  });

  it("does not pre-select a coding agent on hover; selection stays click-only (#563)", async () => {
    const { dispose } = renderPicker({ agentPath: REPO_PATH });
    await settle();

    // Baseline: codex is the initial selection and drives the active comparison row.
    expect(target("agentPicker.provider.codex").getAttribute("data-ac-state")).toBe("active");
    expect(target("agentPicker.comparison.row.codex").getAttribute("data-ac-state")).toBe("active");

    // Hovering claude must NOT activate it nor update the active comparison row.
    const claudeCard = target<HTMLButtonElement>("agentPicker.provider.claude");
    claudeCard.dispatchEvent(new MouseEvent("mouseenter", { bubbles: false }));
    claudeCard.dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    await settle();
    expect(target("agentPicker.provider.claude").getAttribute("data-ac-state")).toBe("inactive");
    expect(target("agentPicker.provider.codex").getAttribute("data-ac-state")).toBe("active");
    expect(target("agentPicker.comparison.row.codex").getAttribute("data-ac-state")).toBe("active");
    expect(target("agentPicker.comparison.row.claude").getAttribute("data-ac-state")).toBe("inactive");

    // Clicking still selects and updates the active comparison row.
    claudeCard.click();
    await settle();
    expect(target("agentPicker.provider.claude").getAttribute("data-ac-state")).toBe("active");
    expect(target("agentPicker.comparison.row.claude").getAttribute("data-ac-state")).toBe("active");
    expect(text("agentPicker.comparison.row.claude")).toContain("A direct");
    expect(text("agentPicker.comparison.row.claude.launchLine")).toBe(
      "claude claude --dangerously-skip-permissions",
    );

    dispose();
  });

  it("shows fallback on missing profile cards and in the comparison row", async () => {
    const { dispose } = renderPicker({ agentPath: REPO_PATH });
    await settle();

    target<HTMLButtonElement>("agentPicker.profile.C").click();
    await settle();

    expect(target("agentPicker.profile.C").getAttribute("data-ac-state")).toBe("active");
    expect(text("agentPicker.profile.C")).toContain("Fallback C->B");
    expect(text("agentPicker.fallback")).toContain("C-REVIEW is not configured");
    expect(text("agentPicker.fallback")).toContain("A remains the final fallback");
    expect(text("agentPicker.comparison.row.codex")).toContain("C-REVIEW → B-FAST (fallback)");
    expect(target("agentPicker.comparison.row.codex").getAttribute("data-ac-profile-status")).toBe("fallback");

    dispose();
  });

  it("renders safe, workgroup-danger, and cross-workgroup kind scopes for a WG replica", async () => {
    const { dispose } = renderPicker({
      agentPath: WG_REPLICA_PATH,
      scopeContext: WG_SCOPE_CONTEXT,
      currentRequestedProfile: "A",
    });
    await settle();

    expect(target("agentPicker.scope.replica")).toBeTruthy();
    expect(target("agentPicker.scope.kind")).toBeTruthy();
    expect(target("agentPicker.scope.workgroup")).toBeTruthy();
    // #800: all three broad-scope previews are fetched up front, so each
    // radio button shows its true targetCount (not 0 for the unselected ones).
    expect(text("agentPicker.scope.replica")).toContain("1 replica");
    expect(text("agentPicker.scope.kind")).toContain("3 replicas");
    expect(text("agentPicker.scope.workgroup")).toContain("4 replicas");
    // Replica scope is safe → apply enabled immediately.
    expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

    dispose();
  });

  it("keeps workgroup apply disabled until the arm checkbox is checked", async () => {
    const { dispose } = renderPicker({
      agentPath: WG_REPLICA_PATH,
      scopeContext: WG_SCOPE_CONTEXT,
      currentRequestedProfile: "A",
    });
    await settle();

    clickRadio("agentPicker.scope.workgroup");
    await settle();

    expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);
    target<HTMLInputElement>("agentPicker.armToggle").click();
    await settle();
    expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

    dispose();
  });

  it("keeps kind apply disabled until the arm checkbox is checked", async () => {
    const { dispose } = renderPicker({
      agentPath: WG_REPLICA_PATH,
      scopeContext: WG_SCOPE_CONTEXT,
      currentRequestedProfile: "A",
    });
    await settle();

    clickRadio("agentPicker.scope.kind");
    await settle();

    expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);
    expect(maybe("agentPicker.kindConfirm")).toBeNull();
    expect(target<HTMLInputElement>("agentPicker.armToggle").closest("label")?.textContent).toContain(
      "I understand this overwrites 3 replicas of this kind",
    );

    target<HTMLInputElement>("agentPicker.armToggle").click();
    await settle();
    expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

    dispose();
  });

  it("resets the kind arm checkbox when the profile changes", async () => {
    const { dispose } = renderPicker({
      agentPath: WG_REPLICA_PATH,
      scopeContext: WG_SCOPE_CONTEXT,
      currentRequestedProfile: "A",
    });
    await settle();

    clickRadio("agentPicker.scope.kind");
    await settle();
    target<HTMLInputElement>("agentPicker.armToggle").click();
    await settle();
    expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

    // Changing the profile must reset the checkbox confirmation and re-disable apply.
    target<HTMLButtonElement>("agentPicker.profile.B").click();
    await settle();
    expect(target<HTMLInputElement>("agentPicker.armToggle").checked).toBe(false);
    expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);

    dispose();
  });

  it("sends confirmedTargetFingerprint and null typedConfirmation for a kind apply", async () => {
    const { dispose, onSelect } = renderPicker({
      agentPath: WG_REPLICA_PATH,
      scopeContext: WG_SCOPE_CONTEXT,
      currentRequestedProfile: "A",
    });
    await settle();

    clickRadio("agentPicker.scope.kind");
    await settle();
    target<HTMLInputElement>("agentPicker.armToggle").click();
    await settle();

    target<HTMLButtonElement>("agentPicker.apply").click();
    await settle();

    expect(mockSettingsApi.applyCodingAgentProfileSelection).toHaveBeenCalledWith(
      expect.objectContaining({
        scope: "kind",
        codingAgentId: "codex",
        profile: "A",
        restartSessions: false,
        confirmedTargetFingerprint: "fp-kind",
        typedConfirmation: null,
      }),
    );
    expect(onSelect).toHaveBeenCalledWith(
      expect.objectContaining({ scope: "kind", restartSessions: false }),
    );

    dispose();
  });

  it("keeps the restart toggle for kind scope and carries it into preview and apply", async () => {
    const { dispose } = renderPicker({
      agentPath: WG_REPLICA_PATH,
      scopeContext: WG_SCOPE_CONTEXT,
      currentRequestedProfile: "A",
    });
    await settle();

    clickRadio("agentPicker.scope.kind");
    await settle();

    target<HTMLInputElement>("agentPicker.restartToggle").click();
    await settle();

    expect(mockSettingsApi.previewCodingAgentProfileSelection).toHaveBeenCalledWith(
      expect.objectContaining({ scope: "kind", restartSessions: true }),
    );

    target<HTMLInputElement>("agentPicker.armToggle").click();
    await settle();
    target<HTMLButtonElement>("agentPicker.apply").click();
    await settle();

    expect(mockSettingsApi.applyCodingAgentProfileSelection).toHaveBeenCalledWith(
      expect.objectContaining({
        scope: "kind",
        restartSessions: true,
        confirmedTargetFingerprint: "fp-kind",
        typedConfirmation: null,
      }),
    );

    dispose();
  });

  it("requires a backend preview fingerprint plus the arm checkbox for a workgroup apply", async () => {
    const { dispose } = renderPicker({
      agentPath: WG_REPLICA_PATH,
      scopeContext: WG_SCOPE_CONTEXT,
      currentRequestedProfile: "A",
    });
    await settle();

    clickRadio("agentPicker.scope.workgroup");
    await settle();
    target<HTMLInputElement>("agentPicker.armToggle").click();
    await settle();

    target<HTMLButtonElement>("agentPicker.apply").click();
    await settle();

    expect(mockSettingsApi.applyCodingAgentProfileSelection).toHaveBeenCalledWith(
      expect.objectContaining({
        scope: "workgroup",
        confirmedTargetFingerprint: "fp-wg",
        typedConfirmation: null,
      }),
    );

    dispose();
  });

  it("hides the restart toggle for replica scope and applies without a backend restart (#537)", async () => {
    // #537: replica scope is restarted via the post-assign "Restart now?" modal, so
    // the in-modal toggle is gone and apply never asks the backend to restart.
    const { dispose } = renderPicker({
      agentPath: WG_REPLICA_PATH,
      scopeContext: WG_SCOPE_CONTEXT,
      currentRequestedProfile: "A",
    });
    await settle();

    expect(maybe("agentPicker.restartToggle")).toBeNull();

    target<HTMLButtonElement>("agentPicker.apply").click();
    await settle();
    expect(mockSettingsApi.applyCodingAgentProfileSelection).toHaveBeenCalledWith(
      expect.objectContaining({ scope: "replica", restartSessions: false }),
    );

    dispose();
  });

  it("keeps the restart toggle for workgroup scope and carries it into preview and apply", async () => {
    const { dispose } = renderPicker({
      agentPath: WG_REPLICA_PATH,
      scopeContext: WG_SCOPE_CONTEXT,
      currentRequestedProfile: "A",
    });
    await settle();

    clickRadio("agentPicker.scope.workgroup");
    await settle();

    // Toggle is available again for the multi-target scope.
    target<HTMLInputElement>("agentPicker.restartToggle").click();
    await settle();

    // Restart change re-previews with restartSessions: true.
    expect(mockSettingsApi.previewCodingAgentProfileSelection).toHaveBeenLastCalledWith(
      expect.objectContaining({ restartSessions: true }),
    );

    // Arm the danger gate, then apply.
    target<HTMLInputElement>("agentPicker.armToggle").click();
    await settle();
    target<HTMLButtonElement>("agentPicker.apply").click();
    await settle();
    expect(mockSettingsApi.applyCodingAgentProfileSelection).toHaveBeenCalledWith(
      expect.objectContaining({ scope: "workgroup", restartSessions: true }),
    );

    dispose();
  });

  it("renders live counts and the cross-workgroup target list from the backend preview", async () => {
    const { dispose } = renderPicker({
      agentPath: WG_REPLICA_PATH,
      scopeContext: WG_SCOPE_CONTEXT,
      currentRequestedProfile: "A",
    });
    await settle();

    clickRadio("agentPicker.scope.kind");
    await settle();

    const targets = target("agentPicker.targets");
    expect(targets).toBeTruthy();
    expect(text("agentPicker.targets")).toContain("3 replica(s) across 3 room(s)");
    expect(text("agentPicker.targets")).toContain("3 live session(s)");
    // A replica with two live sessions is surfaced per-row.
    const rows = targets.querySelectorAll('[data-ac-role="row"]');
    expect(rows.length).toBe(3);
    expect(targets.querySelector('[data-ac-live-sessions="2"]')).toBeTruthy();

    dispose();
  });

  it("renders apply errors and keeps the modal open without selecting", async () => {
    mockSettingsApi.applyCodingAgentProfileSelection.mockResolvedValue(
      applyResult({
        scope: "kind",
        updatedCount: 0,
        errors: [
          { code: "staleFingerprint", message: "Targets changed; rerun preview.", sessionIds: [], replicaPaths: [] },
        ],
      }),
    );
    const { dispose, onSelect } = renderPicker({
      agentPath: WG_REPLICA_PATH,
      scopeContext: WG_SCOPE_CONTEXT,
      currentRequestedProfile: "A",
    });
    await settle();

    clickRadio("agentPicker.scope.kind");
    await settle();
    target<HTMLInputElement>("agentPicker.armToggle").click();
    await settle();

    target<HTMLButtonElement>("agentPicker.apply").click();
    await settle();

    // #537: the failure banner leads with the human-readable backend message
    // (not the internal code) and is surfaced loudly via a toast as well.
    expect(target("agentPicker.errors")).toBeTruthy();
    expect(text("agentPicker.errors")).toContain("Assignment failed");
    expect(text("agentPicker.errors")).toContain("Targets changed; rerun preview.");
    expect(maybe("agentPicker.toast")).toBeTruthy();
    expect(text("agentPicker.toast")).toContain("Targets changed; rerun preview.");
    // Modal stays open; selection is not committed; checkbox confirmation is reset.
    expect(onSelect).not.toHaveBeenCalled();
    expect(maybe("agentPicker.modal")).toBeTruthy();
    expect(target<HTMLInputElement>("agentPicker.armToggle").checked).toBe(false);

    dispose();
  });

  it("resets broad-scope confirmation and re-previews when stale fingerprint apply rejects", async () => {
    const staleFingerprintMessage =
      "Target selection changed. Rerun preview before applying profile selection.";
    mockSettingsApi.applyCodingAgentProfileSelection.mockRejectedValue(
      new Error(staleFingerprintMessage),
    );
    const { dispose, onSelect } = renderPicker({
      agentPath: WG_REPLICA_PATH,
      scopeContext: WG_SCOPE_CONTEXT,
      currentRequestedProfile: "A",
    });
    await settle();

    clickRadio("agentPicker.scope.kind");
    await settle();
    target<HTMLInputElement>("agentPicker.armToggle").click();
    await settle();
    expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

    mockSettingsApi.previewCodingAgentProfileSelection.mockClear();
    target<HTMLButtonElement>("agentPicker.apply").click();
    await settle();

    expect(onSelect).not.toHaveBeenCalled();
    expect(text("agentPicker.toast")).toContain(staleFingerprintMessage);
    expect(target<HTMLInputElement>("agentPicker.armToggle").checked).toBe(false);
    expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);
    expect(mockSettingsApi.previewCodingAgentProfileSelection).toHaveBeenCalledTimes(1);
    expect(mockSettingsApi.previewCodingAgentProfileSelection).toHaveBeenCalledWith(
      expect.objectContaining({
        scope: "kind",
        codingAgentId: "codex",
        profile: "A",
      }),
    );

    dispose();
  });

  it("applies a replica-scope selection through the backend and then commits", async () => {
    const { dispose, onSelect } = renderPicker({
      agentPath: WG_REPLICA_PATH,
      scopeContext: WG_SCOPE_CONTEXT,
      currentRequestedProfile: "A",
    });
    await settle();

    target<HTMLButtonElement>("agentPicker.apply").click();
    await settle();

    expect(mockSettingsApi.applyCodingAgentProfileSelection).toHaveBeenCalledWith(
      expect.objectContaining({
        scope: "replica",
        codingAgentId: "codex",
        profile: "A",
        confirmedTargetFingerprint: null,
        typedConfirmation: null,
      }),
    );
    expect(onSelect).toHaveBeenCalledWith(
      expect.objectContaining({ scope: "replica", effectiveProfile: "A" }),
    );

    dispose();
  });

  it("does not call the backend apply for a normal repo path", async () => {
    const { dispose, onSelect } = renderPicker({ agentPath: REPO_PATH, scopeContext: undefined });
    await settle();

    target<HTMLButtonElement>("agentPicker.apply").click();
    await settle();

    expect(mockSettingsApi.applyCodingAgentProfileSelection).not.toHaveBeenCalled();
    expect(mockSettingsApi.previewCodingAgentProfileSelection).not.toHaveBeenCalled();
    expect(onSelect).toHaveBeenCalledWith(expect.objectContaining({ scope: "replica" }));

    dispose();
  });

  it("surfaces backend profile-resolution warnings without disabling apply", async () => {
    mockSettingsApi.resolveCodingAgentProfile.mockResolvedValue(
      resolution({
        requestedProfile: "A",
        effectiveProfile: "A",
        fallbackChain: ["A"],
        warnings: ["invalid persisted override ignored"],
      }),
    );
    const { dispose } = renderPicker();
    await settle();

    expect(text("agentPicker.fallback")).toContain("Profile warning: invalid persisted override ignored");
    expect(text("agentPicker.fallback")).not.toContain("launches with configured");
    expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

    dispose();
  });

  it("ignores stale backend resolution results after the selected coding agent changes", async () => {
    const pending: Array<{
      agentId: string;
      resolve: (value: CodingAgentProfileResolution) => void;
    }> = [];
    mockSettingsApi.resolveCodingAgentProfile.mockImplementation(
      (_agentPath: string | null, agentId: string) =>
        new Promise<CodingAgentProfileResolution>((resolveBackend) => {
          pending.push({ agentId, resolve: resolveBackend });
        }),
    );
    const { dispose } = renderPicker();
    await settle();

    target<HTMLButtonElement>("agentPicker.provider.claude").click();
    await settle();
    expect(pending.map((item) => item.agentId)).toEqual(["codex", "claude"]);

    pending[0].resolve(
      resolution({ requestedProfile: "C", effectiveProfile: "C", fallbackChain: ["C"], warnings: ["old warning"] }),
    );
    await settle();
    expect(text("agentPicker.fallback")).not.toContain("old warning");
    expect(target("agentPicker.comparison.row.claude").getAttribute("data-ac-state")).toBe("active");

    dispose();
  });

  it("renders a per-card status pill (match / configured / fallback / MISSING)", async () => {
    // Default fixture: codex configures A + B; claude configures A only.
    const { dispose } = renderPicker({ agentPath: REPO_PATH, currentAgentId: "codex" });
    await settle();

    // Selected agent = codex. A baseline → MATCH; B has its own cell → CONFIGURED;
    // C has no cell anywhere → resolves through B → FALLBACK.
    expect(target("agentPicker.profile.A.pill").getAttribute("data-ac-state")).toBe("match");
    expect(text("agentPicker.profile.A.pill")).toContain("MATCH");
    expect(target("agentPicker.profile.B.pill").getAttribute("data-ac-state")).toBe("configured");
    expect(text("agentPicker.profile.B.pill")).toContain("CONFIGURED");
    expect(target("agentPicker.profile.C.pill").getAttribute("data-ac-state")).toBe("fallback");
    expect(text("agentPicker.profile.C.pill")).toContain("FALLBACK");

    // Switch to claude: B has no cell here but is configured on codex → MISSING.
    target<HTMLButtonElement>("agentPicker.provider.claude").click();
    await settle();
    expect(target("agentPicker.profile.A.pill").getAttribute("data-ac-state")).toBe("match");
    expect(target("agentPicker.profile.B.pill").getAttribute("data-ac-state")).toBe("missing");
    expect(text("agentPicker.profile.B.pill")).toContain("MISSING");

    dispose();
  });

  // #551: disable "Assign to this replica" when the pending selection still equals
  // the replica's current Coding Agent + Profile (assign flows opt in).
  describe("redundant replica assignment (#551)", () => {
    const REDUNDANT_TOOLTIP = "This replica already uses this Coding Agent + Profile.";

    it("disables apply with a tooltip while the selection matches the current pair", async () => {
      const { dispose } = renderPicker({
        currentAgentId: "codex",
        currentRequestedProfile: "B",
        disableRedundantReplicaAssign: true,
      });
      await settle();

      const apply = target<HTMLButtonElement>("agentPicker.apply");
      expect(text("agentPicker.apply")).toContain("Assign to this replica");
      expect(apply.disabled).toBe(true);
      expect(apply.getAttribute("title")).toBe(REDUNDANT_TOOLTIP);

      dispose();
    });

    it("re-enables apply when a different coding agent is selected", async () => {
      const { dispose } = renderPicker({
        currentAgentId: "codex",
        currentRequestedProfile: "B",
        disableRedundantReplicaAssign: true,
      });
      await settle();
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);

      target<HTMLButtonElement>("agentPicker.provider.claude").click();
      await settle();

      const apply = target<HTMLButtonElement>("agentPicker.apply");
      expect(apply.disabled).toBe(false);
      expect(apply.getAttribute("title")).toBeNull();

      dispose();
    });

    it("re-enables apply on a different profile and re-disables when the current pair returns", async () => {
      const { dispose } = renderPicker({
        currentAgentId: "codex",
        currentRequestedProfile: "B",
        disableRedundantReplicaAssign: true,
      });
      await settle();
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);

      // codex configures A and B; A differs from the current B.
      target<HTMLButtonElement>("agentPicker.profile.A").click();
      await settle();
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

      // Returning to the current letter re-disables (value comparison, not a touched flag).
      target<HTMLButtonElement>("agentPicker.profile.B").click();
      await settle();
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);

      dispose();
    });

    it("leaves apply enabled for a redundant pair when the opt-in is off (launch flow)", async () => {
      const { dispose } = renderPicker({
        currentAgentId: "codex",
        currentRequestedProfile: "B",
        // disableRedundantReplicaAssign omitted → unchanged launch/legacy behavior
      });
      await settle();

      const apply = target<HTMLButtonElement>("agentPicker.apply");
      expect(apply.disabled).toBe(false);
      expect(apply.getAttribute("title")).toBeNull();

      dispose();
    });

    // #592: drift overrides the #551 redundancy disable. When the target session's
    // loaded profile no longer matches its configuration (profileOutdated), the
    // same-pair re-assign is meaningful (re-stamp the cell content + relaunch), so
    // "Assign to this replica" must stay ENABLED even on the otherwise-redundant pair.
    it("re-enables apply for a redundant pair when the target session has drifted (#592)", async () => {
      const { dispose } = renderPicker({
        currentAgentId: "codex",
        currentRequestedProfile: "B",
        disableRedundantReplicaAssign: true,
        targetProfileOutdated: true,
      });
      await settle();

      const apply = target<HTMLButtonElement>("agentPicker.apply");
      expect(text("agentPicker.apply")).toContain("Assign to this replica");
      // Without drift this exact pair would be disabled with the redundant tooltip
      // (see the first test in this block); drift flips it back on.
      expect(apply.disabled).toBe(false);
      expect(apply.getAttribute("title")).toBeNull();

      dispose();
    });

    it("scopes the disable to replica scope for a WG replica", async () => {
      const { dispose } = renderPicker({
        agentPath: WG_REPLICA_PATH,
        scopeContext: WG_SCOPE_CONTEXT,
        currentAgentId: "codex",
        currentRequestedProfile: "A",
        disableRedundantReplicaAssign: true,
      });
      await settle();

      // Replica scope + current pair → disabled with tooltip.
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);
      expect(target("agentPicker.apply").getAttribute("title")).toBe(REDUNDANT_TOOLTIP);

      // A different profile re-enables replica scope (redundancy is replica-only).
      target<HTMLButtonElement>("agentPicker.profile.B").click();
      await settle();
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);
      expect(target("agentPicker.apply").getAttribute("title")).toBeNull();

      dispose();
    });

    it("uses the backend instance override as the baseline so a stale session profile can't leak a no-op", async () => {
      // Live-session flow where the replica's persisted instance override ("C")
      // differs from the session's launch-time requested profile ("B"). The
      // backend ranks the override first (resolve_profile), so the modal resolves
      // to "C"; the redundancy baseline must be "C", not the stale "B".
      mockSettingsApi.resolveCodingAgentProfile.mockResolvedValue(
        resolution({
          requestedProfile: "C",
          effectiveProfile: "C",
          fallbackChain: ["C"],
          instanceProfileOverride: "C",
        }),
      );
      const { dispose } = renderPicker({
        currentAgentId: "codex",
        currentRequestedProfile: "B",
        disableRedundantReplicaAssign: true,
      });
      await settle();

      // Opens resolved to the override "C" → redundant → disabled.
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);

      // Picking a different letter ("A") is a real change → enabled.
      target<HTMLButtonElement>("agentPicker.profile.A").click();
      await settle();
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

      // Picking the override letter "C" is the replica's current pair → disabled
      // again. Without the override-aware baseline this would compare against the
      // stale "B" and wrongly stay enabled (a no-op assign + needless restart
      // prompt — exactly what #551 blocks).
      target<HTMLButtonElement>("agentPicker.profile.C").click();
      await settle();
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);
      expect(target("agentPicker.apply").getAttribute("title")).toBe(REDUNDANT_TOOLTIP);

      dispose();
    });

    it("uses the origin default profile tier as the baseline (#551 FIX 1)", async () => {
      // Replica with an origin-matrix default profile of "C" (set via "set default
      // profile") and NO instance override / explicit requested profile. The backend
      // fallback chain ranks origin_default above agent_default, so the redundancy
      // baseline must be the origin "C" — not the local defaultProfileByAgent ("B"
      // for "architect" in the fixture), which the buggy memo read instead.
      mockSettingsApi.resolveCodingAgentProfile.mockResolvedValue(
        resolution({
          requestedProfile: "B",
          effectiveProfile: "B",
          fallbackChain: ["B"],
          instanceProfileOverride: null,
          originDefaultProfile: "C",
          agentDefaultProfile: "B",
        }),
      );
      const { dispose } = renderPicker({
        currentAgentId: "codex",
        currentRequestedProfile: null,
        disableRedundantReplicaAssign: true,
      });
      await settle();

      // Selecting the origin-default letter "C" is the replica's current pair → no-op
      // → disabled. (The buggy memo resolved the baseline to "B" and wrongly enabled.)
      target<HTMLButtonElement>("agentPicker.profile.C").click();
      await settle();
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);
      expect(target("agentPicker.apply").getAttribute("title")).toBe(REDUNDANT_TOOLTIP);

      // Selecting the local agent-default letter "B" is a genuine change away from the
      // origin default → enabled. (The buggy memo treated "B" as current → disabled.)
      target<HTMLButtonElement>("agentPicker.profile.B").click();
      await settle();
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);
      expect(target("agentPicker.apply").getAttribute("title")).toBeNull();

      dispose();
    });

    it("keeps assign enabled for a never-assigned replica on its preferred-agent hint (#551 FIX 2)", async () => {
      // Gray replica that was never assigned a coding agent (no currentCodingAgentId)
      // but carries a lastCodingAgent/preferredAgentId hint ("codex"). The picker
      // opens pre-selected on that hint, but with no EXPLICIT current agent the assign
      // is a genuine first pin — never a redundant no-op — so it must stay enabled.
      const { dispose } = renderPicker({
        currentAgentId: "codex", // preferredAgentId hint → pre-selects the picker
        explicitCurrentAgentId: null, // never assigned → no explicit current agent
        currentRequestedProfile: null,
        disableRedundantReplicaAssign: true,
      });
      await settle();

      // Pre-selected on the preferred agent + its default profile, yet enabled.
      const apply = target<HTMLButtonElement>("agentPicker.apply");
      expect(apply.disabled).toBe(false);
      expect(apply.getAttribute("title")).toBeNull();

      // Re-affirming the preferred agent and its default profile keeps it enabled —
      // the user can pin the hinted pair in one click.
      target<HTMLButtonElement>("agentPicker.provider.codex").click();
      target<HTMLButtonElement>("agentPicker.profile.A").click();
      await settle();
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

      dispose();
    });
  });

  // #1943 - the approved selection-lock picker. The fingerprint echoed with each
  // policy is the whole safety mechanism, so every assertion here pins exactly
  // what the backend receives (and, for Cancel, that it receives nothing).
  describe("selection lock picker (#1943)", () => {
    function renderLockPicker(overrides: Parameters<typeof renderPicker>[0] = {}) {
      return renderPicker({
        agentPath: WG_REPLICA_PATH,
        scopeContext: WG_SCOPE_CONTEXT,
        currentRequestedProfile: "A",
        disableRedundantReplicaAssign: true,
        ...overrides,
      });
    }

    function scopeRadios(): HTMLInputElement[] {
      return Array.from(
        document.querySelectorAll<HTMLInputElement>('input[name="agentPickerScope"]'),
      );
    }

    async function armKindLockAndReview(): Promise<void> {
      clickRadio("agentPicker.scope.lock.kind");
      await settle();
      target<HTMLInputElement>("agentPicker.armToggle").click();
      await settle();
      target<HTMLButtonElement>("agentPicker.apply").click();
      await settle();
    }

    it("renders six radios as ONE selection while keeping the ordinary scope row", async () => {
      const { dispose } = renderLockPicker();
      await settle();

      // The ordinary row keeps its three test ids and its live counts.
      expect(text("agentPicker.scope.replica")).toContain("1 replica");
      expect(text("agentPicker.scope.kind")).toContain("3 replicas");
      expect(text("agentPicker.scope.workgroup")).toContain("4 replicas");
      // The lock row repeats exactly the same three scopes.
      expect(text("agentPicker.scope.lock.replica")).toContain("This replica + lock");
      expect(text("agentPicker.scope.lock.kind")).toContain("All replicas of this kind + lock");
      expect(text("agentPicker.scope.lock.workgroup")).toContain("Entire room + lock");

      expect(scopeRadios()).toHaveLength(6);
      expect(scopeRadios().filter((input) => input.checked)).toHaveLength(1);

      clickRadio("agentPicker.scope.lock.kind");
      await settle();

      // Still one selection, and the label states the policy it will apply.
      expect(scopeRadios().filter((input) => input.checked)).toHaveLength(1);
      expect(target("agentPicker.scope.lock.kind").getAttribute("data-ac-state")).toBe("active");
      expect(target("agentPicker.scope.kind").getAttribute("data-ac-state")).toBe("inactive");
      expect(text("agentPicker.apply")).toContain("Overwrite 3 of this kind + lock");
      // Every scope is re-previewed for the new mode, keeping all six counts real.
      expect(mockSettingsApi.previewCodingAgentProfileSelection).toHaveBeenCalledWith(
        expect.objectContaining({ scope: "kind", assignmentMode: "assignAndLock" }),
      );

      dispose();
    });

    it("keeps same-pair assignAndLock enabled when the ordinary redundant assign is disabled", async () => {
      const { dispose } = renderLockPicker({ currentAgentId: "codex", currentRequestedProfile: "A" });
      await settle();

      // Ordinary replica scope on the already-current pair is the #551 no-op.
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);

      clickRadio("agentPicker.scope.lock.replica");
      await settle();

      // Writing the same pair PLUS the lock is a real change, never a no-op.
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);
      expect(text("agentPicker.apply")).toContain("Assign + lock this replica");

      dispose();
    });

    it("sends the replica preview fingerprint for This replica + lock (#2051)", async () => {
      const { dispose, onSelect } = renderLockPicker();
      await settle();

      clickRadio("agentPicker.scope.lock.replica");
      await settle();
      target<HTMLButtonElement>("agentPicker.apply").click();
      await settle();

      // #2051 - the replica preview IS the confirmation: echoing its own
      // fingerprint is what makes the one-step lock apply possible.
      expect(mockSettingsApi.applyCodingAgentProfileSelection).toHaveBeenCalledWith(
        expect.objectContaining({
          scope: "replica",
          assignmentMode: "assignAndLock",
          restartSessions: false,
          confirmedTargetFingerprint: "fp-replica",
        }),
      );
      expect(onSelect).toHaveBeenCalled();

      dispose();
    });

    it("blocks This replica + lock until the replica preview is loaded (#2051)", async () => {
      let resolveReplica: (value: PreviewCodingAgentProfileSelectionResult) => void = () => {};
      mockSettingsApi.previewCodingAgentProfileSelection.mockImplementation(
        (req: { scope: string }) => {
          if (req.scope === "replica") {
            return new Promise<PreviewCodingAgentProfileSelectionResult>((resolve) => {
              resolveReplica = resolve;
            });
          }
          return Promise.resolve(previewResult({ scope: "kind", targetFingerprint: "fp-kind" }));
        },
      );
      const { dispose } = renderLockPicker();
      await settle();

      clickRadio("agentPicker.scope.lock.replica");
      await settle();

      // #2051 - the fingerprint is the confirmation, so the button cannot be
      // clicked before the preview that mints it has landed.
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);
      expect(text("agentPicker.previewBusy")).toContain("Loading targets");

      resolveReplica(previewResult({ scope: "replica", targetFingerprint: "fp-replica-fresh" }));
      await settle();

      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

      dispose();
    });

    it("re-previews replica scope after a rejected replica + lock apply (#2051)", async () => {
      const staleFingerprintMessage =
        "stalePreview: Target selection changed. Rerun preview before applying profile selection.";
      mockSettingsApi.applyCodingAgentProfileSelection.mockRejectedValue(
        new Error(staleFingerprintMessage),
      );
      const { dispose, onSelect } = renderLockPicker();
      await settle();

      clickRadio("agentPicker.scope.lock.replica");
      await settle();

      mockSettingsApi.previewCodingAgentProfileSelection.mockClear();
      target<HTMLButtonElement>("agentPicker.apply").click();
      await settle();

      // #2051 - a rejected fingerprint is never replayed: the replica preview is
      // refreshed (D5) so the next click carries the current value.
      expect(onSelect).not.toHaveBeenCalled();
      expect(text("agentPicker.toast")).toContain(staleFingerprintMessage);
      expect(mockSettingsApi.previewCodingAgentProfileSelection).toHaveBeenCalledTimes(1);
      expect(mockSettingsApi.previewCodingAgentProfileSelection).toHaveBeenCalledWith(
        expect.objectContaining({ scope: "replica", assignmentMode: "assignAndLock" }),
      );

      dispose();
    });

    it("hashes the replica preview with the restart value the apply sends (#2051)", async () => {
      const calls: Array<{ scope: string; restartSessions?: boolean }> = [];
      const basePreview =
        mockSettingsApi.previewCodingAgentProfileSelection.getMockImplementation();
      mockSettingsApi.previewCodingAgentProfileSelection.mockImplementation(
        (req: { scope: string; restartSessions?: boolean }) => {
          calls.push({ scope: req.scope, restartSessions: req.restartSessions });
          if (req.scope === "replica") {
            // Mimic the backend: the restart flag is part of the hashed tuple.
            return Promise.resolve(
              previewResult({
                scope: "replica",
                targetFingerprint: req.restartSessions ? "fp-replica-restart" : "fp-replica",
              }),
            );
          }
          return basePreview?.(req) ?? Promise.resolve(previewResult({ scope: "kind" }));
        },
      );
      const { dispose } = renderLockPicker();
      await settle();

      // A restart choice made on a bulk scope must not leak into the replica
      // hash once the hidden toggle survives the switch.
      clickRadio("agentPicker.scope.kind");
      await settle();
      target<HTMLInputElement>("agentPicker.restartToggle").click();
      await settle();
      clickRadio("agentPicker.scope.lock.replica");
      await settle();

      const replicaCalls = calls.filter((call) => call.scope === "replica");
      expect(replicaCalls.length).toBeGreaterThan(0);
      for (const call of replicaCalls) {
        expect(call.restartSessions).toBe(false);
      }

      target<HTMLButtonElement>("agentPicker.apply").click();
      await settle();

      expect(mockSettingsApi.applyCodingAgentProfileSelection).toHaveBeenCalledWith(
        expect.objectContaining({
          scope: "replica",
          restartSessions: false,
          confirmedTargetFingerprint: "fp-replica",
        }),
      );

      dispose();
    });

    it("opens an accessible review for a conflicting bulk lock and sends nothing on Cancel", async () => {
      const { dispose, onSelect } = renderLockPicker();
      await settle();

      await armKindLockAndReview();

      // The review opens INSTEAD of writing: no policy has been chosen yet.
      expect(maybe("agentPicker.conflict")).toBeTruthy();
      expect(mockSettingsApi.applyCodingAgentProfileSelection).not.toHaveBeenCalled();

      const dialog = target("agentPicker.conflict");
      expect(dialog.querySelector('[role="alertdialog"]')).toBeTruthy();
      const rows = dialog.querySelectorAll('[data-ac-role="row"]');
      // Every protected candidate is listed, INCLUDING the one already on the
      // requested pair.
      expect(rows).toHaveLength(2);
      expect(dialog.textContent).toContain("2 replicas are already locked");

      target<HTMLButtonElement>("agentPicker.conflict.cancel").click();
      await settle();

      expect(maybe("agentPicker.conflict")).toBeNull();
      expect(mockSettingsApi.applyCodingAgentProfileSelection).not.toHaveBeenCalled();
      expect(onSelect).not.toHaveBeenCalled();

      dispose();
    });

    it("closes the review on Escape without sending anything", async () => {
      const { dispose } = renderLockPicker();
      await settle();

      await armKindLockAndReview();
      expect(maybe("agentPicker.conflict")).toBeTruthy();

      const overlay = target("agentPicker.overlay");
      overlay.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      await settle();

      expect(maybe("agentPicker.conflict")).toBeNull();
      expect(maybe("agentPicker.modal")).toBeTruthy();
      expect(mockSettingsApi.applyCodingAgentProfileSelection).not.toHaveBeenCalled();

      dispose();
    });

    it("sends only unlocked candidates with the fingerprint of that exact policy", async () => {
      const { dispose } = renderLockPicker();
      await settle();

      await armKindLockAndReview();
      target<HTMLButtonElement>("agentPicker.conflict.unlockedOnly").click();
      await settle();

      expect(mockSettingsApi.applyCodingAgentProfileSelection).toHaveBeenCalledWith(
        expect.objectContaining({
          scope: "kind",
          assignmentMode: "assignAndLock",
          conflictDecision: "unlockedOnly",
          confirmedTargetFingerprint: "fp-unlocked-only",
        }),
      );

      dispose();
    });

    it("sends the force fingerprint only after the force was actually reviewed", async () => {
      const { dispose } = renderLockPicker();
      await settle();

      await armKindLockAndReview();
      target<HTMLButtonElement>("agentPicker.conflict.forceAll").click();
      await settle();

      expect(mockSettingsApi.applyCodingAgentProfileSelection).toHaveBeenCalledWith(
        expect.objectContaining({
          scope: "kind",
          assignmentMode: "assignAndLock",
          conflictDecision: "forceReviewed",
          confirmedTargetFingerprint: "fp-force-all",
        }),
      );

      dispose();
    });

    it("drops the reviewed force and the arming when the operation changes", async () => {
      const { dispose } = renderLockPicker();
      await settle();

      await armKindLockAndReview();
      target<HTMLButtonElement>("agentPicker.conflict.forceAll").click();
      await settle();
      expect(mockSettingsApi.applyCodingAgentProfileSelection).toHaveBeenLastCalledWith(
        expect.objectContaining({ conflictDecision: "forceReviewed" }),
      );

      // A new profile is a NEW operation: the reviewed force cannot be replayed.
      mockSettingsApi.applyCodingAgentProfileSelection.mockClear();
      target<HTMLButtonElement>("agentPicker.profile.B").click();
      await settle();
      expect(target<HTMLInputElement>("agentPicker.armToggle").checked).toBe(false);
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);

      // Re-arming is not enough: the fresh preview demands a fresh review.
      target<HTMLInputElement>("agentPicker.armToggle").click();
      await settle();
      expect(target<HTMLInputElement>("agentPicker.armToggle").disabled).toBe(false);
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);
      target<HTMLButtonElement>("agentPicker.apply").click();
      await settle();

      expect(maybe("agentPicker.conflict")).toBeTruthy();
      expect(mockSettingsApi.applyCodingAgentProfileSelection).not.toHaveBeenCalled();

      dispose();
    });

    it("lists only protected rows for a bulk lock while the totals stay complete", async () => {
      const { dispose } = renderLockPicker();
      await settle();

      clickRadio("agentPicker.scope.lock.kind");
      await settle();

      const list = target("agentPicker.lockKindList");
      // Two protected rows of three candidates: the unprotected one is not drawn.
      expect(list.querySelectorAll('[data-ac-role="row"]')).toHaveLength(2);
      // The head still reports the complete total, and so does the radio count.
      expect(text("agentPicker.lockKindList")).toContain("3 replica(s) of this kind");
      expect(text("agentPicker.lockKindList")).toContain("3 room(s)");
      expect(text("agentPicker.scope.lock.kind")).toContain("3 replicas");

      dispose();
    });

    it("counts protection per removal scope, independent of the focused replica", async () => {
      const { dispose } = renderLockPicker();
      await settle();

      // The focused replica is unlocked, so ITS scope has nothing to remove...
      expect(text("agentPicker.removeScopeCount.replica")).toBe("0 protected");
      expect(text("agentPicker.removeLock")).toBe("Nothing to remove");
      expect(target<HTMLButtonElement>("agentPicker.removeLock").disabled).toBe(true);
      expect(text("agentPicker.removeNote")).toContain("No protected replicas in this scope");

      // ...while a scope with protected peers stays actionable: the focused
      // replica never decides for the whole scope.
      clickRadio("agentPicker.removeScope.kind");
      await settle();
      expect(text("agentPicker.removeScopeCount.kind")).toBe("2 of 3 protected");
      expect(text("agentPicker.removeLock")).toBe("Remove lock from 2 replicas");
      expect(target<HTMLButtonElement>("agentPicker.removeLock").disabled).toBe(false);

      clickRadio("agentPicker.removeScope.workgroup");
      await settle();
      expect(text("agentPicker.removeScopeCount.workgroup")).toBe("3 of 4 protected");
      expect(text("agentPicker.removeLock")).toBe("Remove lock from 3 replicas");

      // The removal scope never moves the Apply to selection.
      expect(target("agentPicker.scope.replica").getAttribute("data-ac-state")).toBe("active");
      expect(text("agentPicker.apply")).toContain("Assign to this replica");

      dispose();
    });

    it("reports a complete room removal with the count, the kept pair and no restart", async () => {
      const { dispose } = renderLockPicker();
      await settle();

      clickRadio("agentPicker.removeScope.workgroup");
      await settle();
      expect(text("agentPicker.removeScopeCount.workgroup")).toBe("3 of 4 protected");

      // After the removal the scope is re-enumerated from the backend, so the
      // chip shows the authoritative 0 of 4 instead of a local guess.
      mockSettingsApi.previewSelectionLockRemoval.mockImplementation(
        (req: { scope: ProfileAssignmentScope }) =>
          Promise.resolve(
            removePreview(req.scope, {
              protectedCount: 0,
              alreadyUnlockedCount: req.scope === "workgroup" ? 4 : 1,
            }),
          ),
      );
      mockSettingsApi.applySelectionLockRemoval.mockResolvedValue(
        removalApplyResult({
          scope: "workgroup",
          targetFingerprint: "fp-remove-workgroup",
          removedCount: 3,
          removedReplicaPaths: [WG_REPLICA_PATH, "p2", "p3"],
          candidateCount: 4,
          remainingProtectedCount: 0,
        }),
      );

      target<HTMLButtonElement>("agentPicker.removeLock").click();
      await settle();

      // A removal carries no pair, no restart and no decision.
      expect(mockSettingsApi.applySelectionLockRemoval).toHaveBeenCalledWith({
        targetReplicaPath: WG_REPLICA_PATH,
        scope: "workgroup",
        confirmedTargetFingerprint: "fp-remove-workgroup",
      });
      expect(text("agentPicker.removeDone")).toBe(
        "Lock removed from 3 replicas · Coding Agent + Profile kept · no restart",
      );
      expect(text("agentPicker.removeScopeCount.workgroup")).toBe("0 of 4 protected");
      expect(text("agentPicker.removeLock")).toBe("Nothing to remove");
      expect(mockSettingsApi.applyCodingAgentProfileSelection).not.toHaveBeenCalled();

      dispose();
    });

    it("reports the count that landed and never claims 0 of N on an unknown total", async () => {
      const { dispose } = renderLockPicker();
      await settle();

      clickRadio("agentPicker.removeScope.workgroup");
      await settle();

      mockSettingsApi.previewSelectionLockRemoval.mockImplementation(
        (req: { scope: ProfileAssignmentScope }) =>
          Promise.resolve(
            removePreview(req.scope, {
              countsComplete: req.scope !== "workgroup",
            }),
          ),
      );
      mockSettingsApi.applySelectionLockRemoval.mockResolvedValue(
        removalApplyResult({
          scope: "workgroup",
          targetFingerprint: "fp-remove-workgroup",
          removedCount: 2,
          candidateCount: 4,
          remainingProtectedCount: null,
          failedReplicaPaths: ["p4"],
          errors: [
            {
              code: "configWriteFailed",
              message: "p4 could not be written",
              sessionIds: [],
              replicaPaths: ["p4"],
            },
          ],
        }),
      );

      target<HTMLButtonElement>("agentPicker.removeLock").click();
      await settle();

      expect(text("agentPicker.removeDone")).toContain("Lock removed from 2 replicas");
      expect(text("agentPicker.removeErrors")).toContain("p4 could not be written");
      // An incomplete enumeration is never rendered as a confident zero.
      expect(text("agentPicker.lockState")).toBe("Protection unknown");
      expect(text("agentPicker.removeScopeCount.workgroup")).toBe("count unknown");
      expect(text("agentPicker.removeLock")).toBe("Nothing to remove");

      dispose();
    });

    it("never draws an invalid protection state as unlocked, and disables the lock controls", async () => {
      const { dispose } = renderLockPicker({
        scopeContext: {
          ...WG_SCOPE_CONTEXT,
          selectionState: "invalid",
          selectionError: "identity unreadable",
        },
      });
      await settle();

      expect(text("agentPicker.lockDiagnostic")).toContain("identity unreadable");
      expect(text("agentPicker.lockState")).toBe("Check state");
      expect(text("agentPicker.lockState")).not.toBe("Unlocked");
      expect(
        target("agentPicker.scope.lock.replica").querySelector<HTMLInputElement>("input")?.disabled,
      ).toBe(true);
      expect(target<HTMLButtonElement>("agentPicker.removeLock").disabled).toBe(true);
      expect(target<HTMLButtonElement>("agentPicker.defaultSave").disabled).toBe(true);

      dispose();
    });

    it("blocks an already-selected + lock when the protection state stops being readable", async () => {
      // The scope context is reactive here on purpose: the reviewer's scenario is
      // an external update that turns a usable state into an unreadable one AFTER
      // `+ lock` was already chosen.
      const [lockState, setLockState] = createSignal<SelectionState | undefined>("unlocked");
      const root = document.createElement("div");
      document.body.append(root);
      const dispose = render(
        () => (
          <AgentPickerModal
            sessionName="wg-7-dev-team/dev-webpage-ui"
            agentPath={WG_REPLICA_PATH}
            currentAgentId="codex"
            explicitCurrentAgentId="codex"
            currentRequestedProfile="A"
            scopeContext={{ ...WG_SCOPE_CONTEXT, selectionState: lockState() }}
            disableRedundantReplicaAssign
            onSelect={vi.fn()}
            onClose={vi.fn()}
          />
        ),
        root,
      );
      await settle();

      clickRadio("agentPicker.scope.lock.kind");
      await settle();
      target<HTMLInputElement>("agentPicker.armToggle").click();
      await settle();
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

      // Another window invalidates the protection and discovery reloads.
      setLockState("invalid");
      await settle();

      expect(text("agentPicker.lockState")).toBe("Check state");
      // The lock operation is BLOCKED, never silently demoted to an ordinary
      // assignment: demoting would change the policy the user chose.
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);
      expect(target("agentPicker.scope.lock.kind").getAttribute("data-ac-state")).toBe("active");
      expect(target("agentPicker.scope.kind").getAttribute("data-ac-state")).toBe("inactive");
      const lockRadio = target("agentPicker.scope.lock.kind").querySelector<HTMLInputElement>("input");
      expect(lockRadio?.checked).toBe(true);
      expect(lockRadio?.disabled).toBe(true);

      // Re-arming cannot smuggle the lock operation through either.
      target<HTMLInputElement>("agentPicker.armToggle").click();
      await settle();
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);
      expect(mockSettingsApi.applyCodingAgentProfileSelection).not.toHaveBeenCalled();

      dispose();
      document.body.replaceChildren();
    });

    it("diagnoses a protection state this build did not receive", async () => {
      const { dispose } = renderLockPicker({
        // Redundancy is disabled here so `apply` reports ONLY the lock state.
        disableRedundantReplicaAssign: false,
        scopeContext: { ...WG_SCOPE_CONTEXT, selectionState: undefined, savedPair: undefined },
      });
      await settle();

      expect(text("agentPicker.lockDiagnostic")).toContain("was not reported");
      // The chip must never claim the reassuring state while the diagnostic right
      // beside it says the protection state was not reported at all.
      expect(text("agentPicker.lockState")).toBe("State unknown");
      expect(text("agentPicker.lockState")).not.toBe("Unlocked");
      // A bulk count is not a claim about the focused replica, so it survives.
      expect(text("agentPicker.removeScopeCount.kind")).toBe("2 of 3 protected");
      expect(
        target("agentPicker.scope.lock.replica").querySelector<HTMLInputElement>("input")?.disabled,
      ).toBe(true);
      expect(target<HTMLButtonElement>("agentPicker.removeLock").disabled).toBe(true);
      // Assignment itself is untouched by an unknown lock state.
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

      dispose();
    });

    it("shows the stored Matrix default and saves the draft only when asked", async () => {
      mockSettingsApi.setReplicaSelectionDefault.mockImplementation(
        (req: { codingAgentId: string; requestedProfile: string; selectionLocked: boolean }) =>
          Promise.resolve(
            defaultResult({
              default: {
                codingAgentId: req.codingAgentId,
                requestedProfile: req.requestedProfile,
                selectionLocked: req.selectionLocked,
              },
              defaultFingerprint: "fp-default-2",
            }),
          ),
      );
      const { dispose } = renderLockPicker();
      await settle();

      expect(text("agentPicker.defaultPersisted")).toContain("Codex · Profile A");
      expect(text("agentPicker.defaultPersisted")).toContain("Start unlocked");

      // Neither a picker change nor the draft toggle saves anything.
      target<HTMLButtonElement>("agentPicker.profile.B").click();
      await settle();
      target<HTMLInputElement>("agentPicker.defaultStartLocked").click();
      await settle();
      expect(mockSettingsApi.setReplicaSelectionDefault).not.toHaveBeenCalled();
      expect(text("agentPicker.defaultPersisted")).toContain("Codex · Profile A");

      target<HTMLButtonElement>("agentPicker.defaultSave").click();
      await settle();

      // The SAVE writes the currently selected pair with the draft flag, against
      // the latest fingerprint the backend gave us.
      expect(mockSettingsApi.setReplicaSelectionDefault).toHaveBeenCalledWith({
        targetReplicaPath: WG_REPLICA_PATH,
        codingAgentId: "codex",
        requestedProfile: "B",
        selectionLocked: true,
        confirmedDefaultFingerprint: "fp-default-1",
      });
      // Only the confirmed payload replaces the stored view.
      expect(text("agentPicker.defaultPersisted")).toContain("Codex · Profile B");
      expect(text("agentPicker.defaultPersisted")).toContain("Start locked");
      expect(mockSettingsApi.applyCodingAgentProfileSelection).not.toHaveBeenCalled();

      dispose();
    });

    it("keeps the stored default and the draft when the default save is stale", async () => {
      const { dispose } = renderLockPicker();
      await settle();

      target<HTMLButtonElement>("agentPicker.profile.B").click();
      await settle();
      target<HTMLInputElement>("agentPicker.defaultStartLocked").click();
      await settle();

      mockSettingsApi.setReplicaSelectionDefault.mockRejectedValue(
        new Error("staleDefaultFingerprint"),
      );
      target<HTMLButtonElement>("agentPicker.defaultSave").click();
      await settle();

      expect(text("agentPicker.defaultNotice")).toContain(
        "Review the refreshed default before retrying",
      );
      // The stored view keeps the CONFIRMED pair, not the unsaved draft.
      expect(text("agentPicker.defaultPersisted")).toContain("Codex · Profile A");
      expect(text("agentPicker.defaultPersisted")).toContain("Start unlocked");
      // The draft survives for the retry, and a refreshed default was fetched.
      expect(target<HTMLInputElement>("agentPicker.defaultStartLocked").checked).toBe(true);
      expect(mockSettingsApi.getReplicaSelectionDefault.mock.calls.length).toBeGreaterThan(1);

      dispose();
    });

    it("keeps the mutating controls disabled until the removal settles", async () => {
      const { dispose } = renderLockPicker();
      await settle();

      clickRadio("agentPicker.removeScope.kind");
      await settle();

      let resolveRemoval!: (value: ApplySelectionLockRemovalResult) => void;
      mockSettingsApi.applySelectionLockRemoval.mockImplementation(
        () =>
          new Promise<ApplySelectionLockRemovalResult>((resolve) => {
            resolveRemoval = resolve;
          }),
      );

      target<HTMLButtonElement>("agentPicker.removeLock").click();
      await settle();

      // #1942 keeps the promise open until the operation really settles.
      expect(target<HTMLButtonElement>("agentPicker.removeLock").disabled).toBe(true);

      resolveRemoval(removalApplyResult({ scope: "kind", removedCount: 2, candidateCount: 3 }));
      await settle();

      expect(text("agentPicker.removeDone")).toContain("Lock removed from 2 replicas");
      expect(mockSettingsApi.applySelectionLockRemoval).toHaveBeenCalledTimes(1);

      dispose();
    });

    it("blocks a second mutation while a removal or a default save is in flight", async () => {
      // The #551 redundant-pair opt-in is off so the ONLY thing that can disable
      // Apply in this test is the pending mutation itself.
      const { dispose } = renderLockPicker({ disableRedundantReplicaAssign: false });
      await settle();

      // Baseline: an ordinary replica assign needs no arming and no lock state.
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

      let resolveRemoval!: (value: ApplySelectionLockRemovalResult) => void;
      mockSettingsApi.applySelectionLockRemoval.mockImplementation(
        () =>
          new Promise<ApplySelectionLockRemovalResult>((resolve) => {
            resolveRemoval = resolve;
          }),
      );
      clickRadio("agentPicker.removeScope.kind");
      await settle();
      target<HTMLButtonElement>("agentPicker.removeLock").click();
      await settle();

      // A removal is an owned backend operation: no concurrent assignment.
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);

      resolveRemoval(removalApplyResult({ scope: "kind", removedCount: 2, candidateCount: 3 }));
      await settle();
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

      // And the same rule for a default save that has not settled.
      let resolveDefault!: (value: ReplicaSelectionDefaultResult) => void;
      mockSettingsApi.setReplicaSelectionDefault.mockImplementation(
        () =>
          new Promise<ReplicaSelectionDefaultResult>((resolve) => {
            resolveDefault = resolve;
          }),
      );
      target<HTMLButtonElement>("agentPicker.defaultSave").click();
      await settle();
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);

      resolveDefault(defaultResult());
      await settle();
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

      dispose();
    });

    it("reloads its own previews and default when another window updates the selection", async () => {
      let externalUpdate: (() => void) | null = null;
      mockSettingsApi.onCodingAgentProfileSelectionUpdated.mockImplementation(
        (callback: () => void) => {
          externalUpdate = callback;
          return Promise.resolve(() => {});
        },
      );
      const { dispose } = renderLockPicker();
      await settle();
      expect(externalUpdate).toBeTruthy();

      mockSettingsApi.previewSelectionLockRemoval.mockClear();
      mockSettingsApi.getReplicaSelectionDefault.mockClear();
      mockSettingsApi.previewCodingAgentProfileSelection.mockClear();

      externalUpdate!();
      await settle();

      expect(mockSettingsApi.previewSelectionLockRemoval).toHaveBeenCalledTimes(3);
      expect(mockSettingsApi.getReplicaSelectionDefault).toHaveBeenCalledTimes(1);
      expect(mockSettingsApi.previewCodingAgentProfileSelection).toHaveBeenCalledTimes(3);

      dispose();
    });

    it("releases the external-update listener on unmount", async () => {
      const unlisten = vi.fn();
      mockSettingsApi.onCodingAgentProfileSelectionUpdated.mockImplementation(() =>
        Promise.resolve(unlisten),
      );
      const { dispose } = renderLockPicker();
      await settle();

      dispose();
      await settle();

      expect(unlisten).toHaveBeenCalledTimes(1);
    });

    it("drops a stale removal preview so the newest scope state wins", async () => {
      const pending: Array<{
        scope: string;
        resolve: (value: PreviewSelectionLockRemovalResult) => void;
      }> = [];
      mockSettingsApi.previewSelectionLockRemoval.mockImplementation(
        (req: { scope: string }) =>
          new Promise<PreviewSelectionLockRemovalResult>((resolve) => {
            pending.push({ scope: req.scope, resolve });
          }),
      );
      let externalUpdate: (() => void) | null = null;
      mockSettingsApi.onCodingAgentProfileSelectionUpdated.mockImplementation(
        (callback: () => void) => {
          externalUpdate = callback;
          return Promise.resolve(() => {});
        },
      );
      const { dispose } = renderLockPicker();
      await settle();
      expect(pending).toHaveLength(3);

      externalUpdate!();
      await settle();
      expect(pending).toHaveLength(6);

      const stale = pending[0];
      const fresh = pending[3];
      expect(stale.scope).toBe("replica");
      expect(fresh.scope).toBe("replica");

      // The newest request lands first; the older reply arrives LAST and must be
      // dropped by the per-scope sequence token.
      fresh.resolve(removePreview("replica", { protectedCount: 1, alreadyUnlockedCount: 0 }));
      await settle();
      stale.resolve(removePreview("replica", { protectedCount: 0, alreadyUnlockedCount: 1 }));
      await settle();

      expect(text("agentPicker.removeScopeCount.replica")).toBe("1 protected");

      dispose();
    });

    it("places the lock bar below the three-panel body and keeps the review over the modal", async () => {
      const { dispose } = renderLockPicker();
      await settle();

      const modal = target("agentPicker.modal");
      const bar = target("agentPicker.lockBar");
      const body = modal.querySelector(".agent-profile-assignment-body");
      expect(body).toBeTruthy();
      // Layout A (#2014): the bar is a flex child of the modal BELOW the
      // three-panel body, so DOM order IS the layout contract.
      expect(bar.parentElement).toBe(modal);
      expect(
        body!.compareDocumentPosition(bar) & Node.DOCUMENT_POSITION_FOLLOWING,
      ).toBeTruthy();

      await armKindLockAndReview();
      const dialog = target("agentPicker.conflict");
      // The review covers the whole overlay instead of being clipped inside the
      // modal's own overflow, which is why it is a sibling of the modal.
      expect(modal.parentElement?.classList.contains("modal-overlay")).toBe(true);
      expect(dialog.parentElement).toBe(modal.parentElement);

      dispose();
    });

    it("O1 layout A order: body, lock bar, Matrix default, Apply to, action bar last", async () => {
      const { dispose } = renderLockPicker();
      await settle();

      const modal = target("agentPicker.modal");
      const follows = (first: Element, second: Element) =>
        (first.compareDocumentPosition(second) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0;

      const header = modal.querySelector(".agent-picker-modal-header")!;
      const body = modal.querySelector(".agent-profile-assignment-body")!;
      const lockBar = target("agentPicker.lockBar");
      const defaultSection = target("agentPicker.defaultSection");
      const botonera = modal.querySelector(".agent-picker-botonera")!;

      expect(follows(header, body)).toBe(true);
      expect(follows(body, lockBar)).toBe(true);
      expect(follows(lockBar, defaultSection)).toBe(true);
      expect(follows(defaultSection, botonera)).toBe(true);
      expect(modal.lastElementChild).toBe(botonera);
      expect(botonera.lastElementChild?.classList.contains("agent-picker-bar")).toBe(true);
      // Cancel and Assign exist exactly once; the action bar is the last block.
      expect(document.querySelectorAll('[data-ac-testid="agentPicker.cancel"]')).toHaveLength(1);
      expect(document.querySelectorAll('[data-ac-testid="agentPicker.apply"]')).toHaveLength(1);

      dispose();
    });

    it("leaves the root/origin picker untouched and never calls a lock command", async () => {
      const { dispose } = renderPicker({ scopeContext: undefined });
      await settle();

      expect(maybe("agentPicker.lockBar")).toBeNull();
      expect(maybe("agentPicker.scope.lock.replica")).toBeNull();
      expect(maybe("agentPicker.defaultSection")).toBeNull();
      expect(maybe("agentPicker.conflict")).toBeNull();
      expect(mockSettingsApi.previewSelectionLockRemoval).not.toHaveBeenCalled();
      expect(mockSettingsApi.applySelectionLockRemoval).not.toHaveBeenCalled();
      expect(mockSettingsApi.getReplicaSelectionDefault).not.toHaveBeenCalled();
      expect(mockSettingsApi.setReplicaSelectionDefault).not.toHaveBeenCalled();
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

      dispose();
    });
  });

  describe("#2014 launch lines and left-only filter", () => {
    async function setAgentFilter(value: string): Promise<void> {
      const input = target<HTMLInputElement>("agentPicker.agentFilter");
      input.value = value;
      input.dispatchEvent(new Event("input", { bubbles: true }));
      await settle();
    }

    function renderWgPicker(overrides: Parameters<typeof renderPicker>[0] = {}) {
      return renderPicker({
        agentPath: WG_REPLICA_PATH,
        scopeContext: WG_SCOPE_CONTEXT,
        currentRequestedProfile: "A",
        ...overrides,
      });
    }

    it("L1 shows each agent's full launch line instead of configured peer", async () => {
      const { dispose } = renderPicker({ agentPath: REPO_PATH });
      await settle();

      expect(text("agentPicker.comparison.row.codex.launchLine")).toBe("codex codex --model gpt-5");
      expect(text("agentPicker.comparison.row.claude.launchLine")).toBe(
        "claude claude --dangerously-skip-permissions",
      );
      const comparison = text("agentPicker.comparison");
      expect(comparison).not.toContain("configured peer");
      expect(comparison).not.toContain("selected coding agent");

      dispose();
    });

    it("L2 launch line follows the effective profile after fallback", async () => {
      const { dispose } = renderPicker({ agentPath: REPO_PATH });
      await settle();

      target<HTMLButtonElement>("agentPicker.profile.C").click();
      await settle();

      // codex C -> B (B is the last enabled cell below C); claude C -> B -> A.
      expect(text("agentPicker.comparison.row.codex.launchLine")).toBe("codex codex --profile fast");
      expect(text("agentPicker.comparison.row.claude.launchLine")).toBe(
        "claude claude --dangerously-skip-permissions",
      );
      expect(target("agentPicker.comparison.row.codex").getAttribute("data-ac-profile-status")).toBe(
        "fallback",
      );
      expect(target("agentPicker.comparison.row.claude").getAttribute("data-ac-profile-status")).toBe(
        "fallback",
      );

      dispose();
    });

    it("L3 launch line ignores a disabled cell and mirrors the backend fallback case", async () => {
      const base = settings();
      currentSettings = settings({
        codingAgentProfiles: {
          ...base.codingAgentProfiles,
          profileSlots: { ...base.codingAgentProfiles.profileSlots, D: { label: "" } },
          profilesByAgent: {
            ...base.codingAgentProfiles.profilesByAgent,
            codex: {
              A: { enabled: true, command: "codex --a", env: {}, notes: "" },
              C: { enabled: true, command: "codex --c", env: {}, notes: "" },
              D: { enabled: false, command: "codex --d", env: {}, notes: "" },
            },
          },
        },
      });
      mockSettingsApi.get.mockResolvedValue(currentSettings);
      const { dispose } = renderPicker({ agentPath: REPO_PATH });
      await settle();

      target<HTMLButtonElement>("agentPicker.profile.D").click();
      await settle();

      // The disabled D is skipped by resolution, never composed into the line.
      expect(text("agentPicker.comparison.row.codex.launchLine")).toBe("codex codex --c");

      dispose();
    });

    it("L4 launch line drops a disabled profile A to the bare command", async () => {
      const base = settings();
      currentSettings = settings({
        codingAgentProfiles: {
          ...base.codingAgentProfiles,
          profilesByAgent: {
            ...base.codingAgentProfiles.profilesByAgent,
            codex: { A: { enabled: false, command: "codex --a", env: {}, notes: "" } },
          },
        },
      });
      mockSettingsApi.get.mockResolvedValue(currentSettings);
      const { dispose } = renderPicker({ agentPath: REPO_PATH });
      await settle();

      // Resolution always stops at A, so only `enabled` can hide `--a`.
      expect(target("agentPicker.profile.A").getAttribute("data-ac-state")).toBe("active");
      expect(text("agentPicker.comparison.row.codex.launchLine")).toBe("codex");

      dispose();
    });

    it("F1 filters only the left list by name, executable, argument and case", async () => {
      const { dispose } = renderPicker({ agentPath: REPO_PATH });
      await settle();

      const present = (id: string) => maybe(`agentPicker.provider.${id}`) !== null;

      await setAgentFilter("CLAUDE");
      expect(present("claude")).toBe(true);
      expect(present("codex")).toBe(false);

      // Case-folding kill: only the lowercased label carries "claude code".
      await setAgentFilter("CLAUDE CODE");
      expect(present("claude")).toBe(true);
      expect(present("codex")).toBe(false);

      await setAgentFilter("codex");
      expect(present("claude")).toBe(false);
      expect(present("codex")).toBe(true);

      // Argument-only match: "--model" is in the line, never in the label.
      await setAgentFilter("--model");
      expect(present("claude")).toBe(false);
      expect(present("codex")).toBe(true);

      await setAgentFilter("Dangerously");
      expect(present("claude")).toBe(true);
      expect(present("codex")).toBe(false);

      dispose();
    });

    it("F2 right panel is byte-identical under every filter query, including zero matches", async () => {
      const { dispose } = renderPicker({ agentPath: REPO_PATH });
      await settle();

      const before = target("agentPicker.comparison").outerHTML;
      for (const query of ["claude", "--model"]) {
        await setAgentFilter(query);
        expect(target("agentPicker.comparison").outerHTML).toBe(before);
      }

      await setAgentFilter("zzz-no-match");
      expect(target("agentPicker.comparison").outerHTML).toBe(before);
      expect(maybe("agentPicker.provider.codex")).toBeNull();
      expect(maybe("agentPicker.provider.claude")).toBeNull();
      expect(text("agentPicker.agentFilterStatus")).toBe(
        'No coding agent matches "zzz-no-match". Clear the filter to see all 2.',
      );

      await setAgentFilter("");
      expect(target("agentPicker.comparison").outerHTML).toBe(before);
      expect(maybe("agentPicker.provider.codex")).toBeTruthy();
      expect(maybe("agentPicker.provider.claude")).toBeTruthy();
      expect(text("agentPicker.agentFilterStatus")).toBe("2 agents");

      dispose();
    });

    it("F3 filtering never changes selection, profile, radios or buttons", async () => {
      const { dispose, onSelect } = renderWgPicker();
      await settle();

      target<HTMLButtonElement>("agentPicker.provider.claude").click();
      await settle();
      target<HTMLButtonElement>("agentPicker.profile.B").click();
      await settle();

      const previewCalls = mockSettingsApi.previewCodingAgentProfileSelection.mock.calls.length;
      const applyCalls = mockSettingsApi.applyCodingAgentProfileSelection.mock.calls.length;
      const applyDisabled = target<HTMLButtonElement>("agentPicker.apply").disabled;
      const stateSnapshot = () => {
        const map = new Map<string, string | null>();
        document.querySelectorAll<HTMLElement>('[data-ac-testid^="agentPicker."]').forEach((element) => {
          const id = element.getAttribute("data-ac-testid")!;
          if (id.startsWith("agentPicker.provider.") || id.startsWith("agentPicker.agentFilter")) return;
          map.set(id, element.getAttribute("data-ac-state"));
        });
        return map;
      };
      const before = stateSnapshot();

      await setAgentFilter("codex");
      expect(maybe("agentPicker.provider.claude")).toBeNull();
      expect(target("agentPicker.provider.codex").getAttribute("data-ac-state")).not.toBe("active");
      expect(target("agentPicker.provider.codex").getAttribute("aria-pressed")).toBe("false");
      expect(target("agentPicker.comparison.row.claude").getAttribute("data-ac-state")).toBe("active");

      await setAgentFilter("");
      expect(stateSnapshot()).toEqual(before);
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(applyDisabled);
      expect(target("agentPicker.provider.claude").getAttribute("data-ac-state")).toBe("active");
      expect(target("agentPicker.provider.claude").getAttribute("aria-pressed")).toBe("true");
      expect(mockSettingsApi.previewCodingAgentProfileSelection).toHaveBeenCalledTimes(previewCalls);
      expect(mockSettingsApi.applyCodingAgentProfileSelection).toHaveBeenCalledTimes(applyCalls);
      expect(onSelect).not.toHaveBeenCalled();

      dispose();
    });

    it("F3b clicking a card while filtered selects that card", async () => {
      const { dispose } = renderPicker({ agentPath: REPO_PATH });
      await settle();

      // Sorted order is Claude Code (0), Codex (1): an index taken from the
      // filtered list would select claude instead of the clicked codex card.
      await setAgentFilter("codex");
      target<HTMLButtonElement>("agentPicker.provider.codex").click();
      await settle();

      expect(target("agentPicker.comparison.row.codex").getAttribute("data-ac-state")).toBe("active");
      expect(target("agentPicker.comparison.row.claude").getAttribute("data-ac-state")).not.toBe("active");
      expect(target("agentPicker.provider.codex").getAttribute("aria-pressed")).toBe("true");

      dispose();
    });

    it("F4 keys typed in the filter do not move profile or selection", async () => {
      const { dispose } = renderPicker({ agentPath: REPO_PATH });
      await settle();

      const input = target<HTMLInputElement>("agentPicker.agentFilter");
      input.focus();
      for (const key of ["ArrowRight", "ArrowLeft", "ArrowDown", "Enter"]) {
        input.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true }));
        await settle();
        // Checked after EVERY key: ArrowRight then ArrowLeft would otherwise
        // cancel each other out and hide a moved profile.
        expect(target("agentPicker.profile.A").getAttribute("data-ac-state")).toBe("active");
        expect(target("agentPicker.provider.codex").getAttribute("data-ac-state")).toBe("active");
        expect(target("agentPicker.comparison.row.codex").getAttribute("data-ac-state")).toBe("active");
      }
      expect(mockSettingsApi.applyCodingAgentProfileSelection).not.toHaveBeenCalled();

      dispose();
    });

    it("F5 filter is labelled and placed right before the first card", async () => {
      const { dispose } = renderPicker({ agentPath: REPO_PATH });
      await settle();

      const input = target<HTMLInputElement>("agentPicker.agentFilter");
      expect(input.labels && input.labels[0]?.textContent).toBe("Filter by name or start line");
      const wrapper = input.closest(".agent-profile-provider-filter");
      expect(wrapper?.nextElementSibling).toBe(target("agentPicker.providers"));

      dispose();
    });
  });

  describe("#2306 P3 coding-agent moves", () => {
    const FIVE: AgentConfig[] = ["a", "b", "c", "d", "e"].map((id, index) =>
      agent({ id, label: `Agent ${id.toUpperCase()}`, command: `cmd-${id}`, order: index }),
    );
    const byId = (id: string): AgentConfig => FIVE.find((candidate) => candidate.id === id)!;
    const orderOf = (ids: string[]): AgentConfig[] => ids.map((id) => byId(id));

    const orderedSnapshot = (
      agents: AgentConfig[],
      overlayOwnsAgents = false,
      overrides: Partial<AppSettings> = {},
    ): AppSettings =>
      ({
        ...settings({ ...overrides, agents }),
        overlayOwnsAgents,
      }) as unknown as AppSettings;

    const cardIds = (): string[] =>
      [
        ...target("agentPicker.providers").querySelectorAll<HTMLElement>(
          ".agent-profile-provider-card",
        ),
      ].map((card) => card.getAttribute("data-ac-agent-id") ?? "");

    const comparisonIds = (): string[] =>
      [
        ...target("agentPicker.comparison").querySelectorAll<HTMLElement>("[data-ac-agent-id]"),
      ].map((row) => row.getAttribute("data-ac-agent-id") ?? "");

    const setFilter = async (value: string): Promise<void> => {
      const input = target<HTMLInputElement>("agentPicker.agentFilter");
      input.value = value;
      input.dispatchEvent(new Event("input", { bubbles: true }));
      await settle();
    };

    const installMove = (): void => {
      mockSettingsApi.moveCodingAgent.mockImplementation(
        async (request: { id: string; neighborId: string; direction: "up" | "down" }) => {
          const ids = cardIds();
          const from = ids.indexOf(request.id);
          const to = request.direction === "up" ? from - 1 : from + 1;
          ids.splice(to, 0, ids.splice(from, 1)[0]!);
          currentSettings = orderedSnapshot(orderOf(ids));
          mockSettingsApi.get.mockResolvedValue(currentSettings);
          return ids;
        },
      );
    };

    const fireSettingsUpdate = (): (() => void) => {
      let fire!: () => void;
      mockSettingsApi.onCodingAgentSettingsUpdated.mockImplementation((cb: () => void) => {
        fire = cb;
        return Promise.resolve(() => {});
      });
      return () => fire();
    };

    it("renders backend vector order and selects currentAgentId without an alphabetical remap", async () => {
      currentSettings = orderedSnapshot([byId("b"), byId("a")]);
      mockSettingsApi.get.mockResolvedValue(currentSettings);
      const { dispose } = renderPicker({ currentAgentId: "a" });
      await settle();

      expect(cardIds()).toEqual(["b", "a"]);
      expect(target("agentPicker.provider.a").getAttribute("data-ac-state")).toBe("active");
      expect(target("agentPicker.provider.b").getAttribute("data-ac-state")).toBe("inactive");
      expect(comparisonIds()).toEqual(["b", "a"]);

      dispose();
    });

    it("sends the exact adjacent payload per direction and installs the authoritative refetch", async () => {
      currentSettings = orderedSnapshot(FIVE);
      mockSettingsApi.get.mockResolvedValue(currentSettings);
      installMove();
      const { dispose } = renderPicker({ currentAgentId: "c" });
      await settle();

      target<HTMLButtonElement>("agentPicker.provider.c.moveUp").click();
      await settle();

      expect(mockSettingsApi.moveCodingAgent).toHaveBeenNthCalledWith(1, {
        id: "c",
        neighborId: "b",
        direction: "up",
      });
      expect(cardIds()).toEqual(["a", "c", "b", "d", "e"]);
      expect(target("agentPicker.provider.c").getAttribute("data-ac-state")).toBe("active");
      expect(text("agentPicker.moveStatus")).toBe("Moved Agent C up to position 2 of 5.");

      target<HTMLButtonElement>("agentPicker.provider.c.moveDown").click();
      await settle();

      expect(mockSettingsApi.moveCodingAgent).toHaveBeenNthCalledWith(2, {
        id: "c",
        neighborId: "b",
        direction: "down",
      });
      expect(cardIds()).toEqual(["a", "b", "c", "d", "e"]);
      expect(text("agentPicker.moveStatus")).toBe("Moved Agent C down to position 3 of 5.");

      dispose();
    });

    it("disables first-up and last-down and every move while filtered", async () => {
      currentSettings = orderedSnapshot(FIVE);
      mockSettingsApi.get.mockResolvedValue(currentSettings);
      const { dispose } = renderPicker({ currentAgentId: "c" });
      await settle();

      expect(target<HTMLButtonElement>("agentPicker.provider.a.moveUp").disabled).toBe(true);
      expect(target<HTMLButtonElement>("agentPicker.provider.a.moveDown").disabled).toBe(false);
      expect(target<HTMLButtonElement>("agentPicker.provider.e.moveDown").disabled).toBe(true);
      expect(target<HTMLButtonElement>("agentPicker.provider.e.moveUp").disabled).toBe(false);

      await setFilter("Agent B");
      expect(cardIds()).toEqual(["b"]);
      expect(target<HTMLButtonElement>("agentPicker.provider.b.moveUp").disabled).toBe(true);
      expect(target<HTMLButtonElement>("agentPicker.provider.b.moveDown").disabled).toBe(true);

      target<HTMLButtonElement>("agentPicker.provider.b.moveUp").click();
      await settle();
      expect(mockSettingsApi.moveCodingAgent).not.toHaveBeenCalled();

      await setFilter("");
      expect(target<HTMLButtonElement>("agentPicker.provider.b.moveUp").disabled).toBe(false);
      expect(target<HTMLButtonElement>("agentPicker.provider.b.moveDown").disabled).toBe(false);

      dispose();
    });

    it("serializes moves per modal: every control disables until reconciliation", async () => {
      currentSettings = orderedSnapshot(FIVE);
      mockSettingsApi.get.mockResolvedValue(currentSettings);
      let resolveMove!: (ids: string[]) => void;
      mockSettingsApi.moveCodingAgent.mockImplementation(
        () => new Promise<string[]>((resolve) => { resolveMove = resolve; }),
      );
      const { dispose } = renderPicker({ currentAgentId: "c" });
      await settle();

      target<HTMLButtonElement>("agentPicker.provider.c.moveUp").click();
      await settle();

      expect(mockSettingsApi.moveCodingAgent).toHaveBeenCalledTimes(1);
      for (const id of ["a", "b", "c", "d", "e"]) {
        expect(target<HTMLButtonElement>(`agentPicker.provider.${id}.moveUp`).disabled).toBe(true);
        expect(target<HTMLButtonElement>(`agentPicker.provider.${id}.moveDown`).disabled).toBe(true);
      }

      const ids = ["a", "c", "b", "d", "e"];
      currentSettings = orderedSnapshot(orderOf(ids));
      mockSettingsApi.get.mockResolvedValue(currentSettings);
      resolveMove(ids);
      await settle();

      expect(cardIds()).toEqual(ids);
      expect(target<HTMLButtonElement>("agentPicker.provider.c.moveUp").disabled).toBe(false);

      dispose();
    });

    it("disables moves under the local-overlay owner, shows the reason and never calls the API", async () => {
      currentSettings = orderedSnapshot([byId("d"), byId("b"), byId("a")], true);
      mockSettingsApi.get.mockResolvedValue(currentSettings);
      const { dispose } = renderPicker({ currentAgentId: "b" });
      await settle();

      // The overlay-derived vector order is what both views display.
      expect(cardIds()).toEqual(["d", "b", "a"]);
      const up = target<HTMLButtonElement>("agentPicker.provider.b.moveUp");
      const down = target<HTMLButtonElement>("agentPicker.provider.b.moveDown");
      expect(up.disabled).toBe(true);
      expect(down.disabled).toBe(true);
      expect(up.getAttribute("title")).toContain("local settings overlay");
      expect(up.getAttribute("aria-label")).toContain("local settings overlay");
      expect(text("agentPicker.overlayReason")).toContain("local settings overlay");

      up.click();
      await settle();
      expect(mockSettingsApi.moveCodingAgent).not.toHaveBeenCalled();

      dispose();
    });

    it("command failure refetches, keeps the authoritative order and shows a polite inline error", async () => {
      currentSettings = orderedSnapshot(FIVE);
      mockSettingsApi.get.mockResolvedValue(currentSettings);
      mockSettingsApi.moveCodingAgent.mockRejectedValue(new Error("settings lock busy"));
      const { dispose } = renderPicker({ currentAgentId: "c" });
      await settle();
      const fetchesBefore = mockSettingsApi.get.mock.calls.length;

      target<HTMLButtonElement>("agentPicker.provider.c.moveUp").click();
      await settle();

      expect(mockSettingsApi.moveCodingAgent).toHaveBeenCalledTimes(1);
      expect(mockSettingsApi.get.mock.calls.length).toBe(fetchesBefore + 1);
      expect(cardIds()).toEqual(["a", "b", "c", "d", "e"]);
      const error = target("agentPicker.moveError");
      expect(error.getAttribute("aria-live")).toBe("polite");
      expect(error.textContent).toContain("settings lock busy");
      expect(text("agentPicker.moveStatus")).toBe("");

      dispose();
    });

    it.each([
      { label: "a shorter order", returned: ["b", "a"] },
      {
        label: "a same-length order that is not the requested swap",
        returned: ["a", "b", "c", "d", "e"],
      },
    ])("rejects $label as a successful move", async ({ returned }) => {
      currentSettings = orderedSnapshot(FIVE);
      mockSettingsApi.get.mockResolvedValue(currentSettings);
      mockSettingsApi.moveCodingAgent.mockResolvedValue(returned);
      const { dispose } = renderPicker({ currentAgentId: "c" });
      await settle();

      target<HTMLButtonElement>("agentPicker.provider.c.moveUp").click();
      await settle();

      expect(cardIds()).toEqual(["a", "b", "c", "d", "e"]);
      expect(text("agentPicker.moveError")).toContain("unexpected agent order");

      dispose();
    });

    it("treats a refetch failure as an error and keeps the last authoritative order", async () => {
      currentSettings = orderedSnapshot(FIVE);
      mockSettingsApi.get.mockResolvedValueOnce(currentSettings);
      mockSettingsApi.get.mockImplementation(() => Promise.reject(new Error("offline")));
      mockSettingsApi.moveCodingAgent.mockResolvedValue(["a", "c", "b", "d", "e"]);
      const { dispose } = renderPicker({ currentAgentId: "c" });
      await settle();

      target<HTMLButtonElement>("agentPicker.provider.c.moveUp").click();
      await settle();

      expect(cardIds()).toEqual(["a", "b", "c", "d", "e"]);
      expect(text("agentPicker.moveError")).toContain("offline");

      dispose();
    });

    it("restores focus to the same control after an interior move", async () => {
      currentSettings = orderedSnapshot(FIVE);
      mockSettingsApi.get.mockResolvedValue(currentSettings);
      installMove();
      const { dispose } = renderPicker({ currentAgentId: "c" });
      await settle();

      target<HTMLButtonElement>("agentPicker.provider.c.moveUp").click();
      await settle();

      expect(document.activeElement).toBe(target("agentPicker.provider.c.moveUp"));

      dispose();
    });

    it("focuses the remaining direction when the moved control hits a new boundary", async () => {
      currentSettings = orderedSnapshot(FIVE);
      mockSettingsApi.get.mockResolvedValue(currentSettings);
      installMove();
      const { dispose } = renderPicker({ currentAgentId: "c" });
      await settle();

      target<HTMLButtonElement>("agentPicker.provider.b.moveUp").click();
      await settle();

      expect(cardIds()).toEqual(["b", "a", "c", "d", "e"]);
      expect(target<HTMLButtonElement>("agentPicker.provider.b.moveUp").disabled).toBe(true);
      expect(document.activeElement).toBe(target("agentPicker.provider.b.moveDown"));

      dispose();
    });

    it("gives each move button a distinct tool-and-direction accessible name and no nested buttons", async () => {
      currentSettings = orderedSnapshot(FIVE);
      mockSettingsApi.get.mockResolvedValue(currentSettings);
      const { dispose } = renderPicker({ currentAgentId: "c" });
      await settle();

      const up = target<HTMLButtonElement>("agentPicker.provider.c.moveUp");
      expect(up.tagName).toBe("BUTTON");
      expect(up.getAttribute("type")).toBe("button");
      expect(up.getAttribute("aria-label")).toBe("Move Agent C up");
      expect(target("agentPicker.provider.c.moveDown").getAttribute("aria-label")).toBe(
        "Move Agent C down",
      );
      expect(target("agentPicker.provider.d.moveUp").getAttribute("aria-label")).toBe(
        "Move Agent D up",
      );
      // The card stays a single button: the move controls are siblings, not children.
      expect(target("agentPicker.provider.c").contains(up)).toBe(false);

      dispose();
    });

    it("refetches on coding_agent_settings_updated and keeps a surviving ID at its new position", async () => {
      currentSettings = orderedSnapshot(FIVE);
      mockSettingsApi.get.mockResolvedValue(currentSettings);
      const fire = fireSettingsUpdate();
      const { dispose } = renderPicker({ currentAgentId: "c" });
      await settle();

      const reordered = orderOf(["e", "d", "c", "b", "a"]);
      currentSettings = orderedSnapshot(reordered);
      mockSettingsApi.get.mockResolvedValue(currentSettings);
      fire();
      await settle();

      expect(cardIds()).toEqual(["e", "d", "c", "b", "a"]);
      expect(target("agentPicker.provider.c").getAttribute("data-ac-state")).toBe("active");
      expect(target("agentPicker.provider.c").getAttribute("aria-pressed")).toBe("true");

      dispose();
    });

    it("selects the survivor at the removed tool's old position when it disappears", async () => {
      currentSettings = orderedSnapshot(FIVE);
      mockSettingsApi.get.mockResolvedValue(currentSettings);
      const fire = fireSettingsUpdate();
      const { dispose } = renderPicker({ currentAgentId: "c" });
      await settle();

      currentSettings = orderedSnapshot(orderOf(["a", "b", "d", "e"]));
      mockSettingsApi.get.mockResolvedValue(currentSettings);
      fire();
      await settle();

      expect(cardIds()).toEqual(["a", "b", "d", "e"]);
      expect(target("agentPicker.provider.d").getAttribute("data-ac-state")).toBe("active");

      dispose();
    });

    it("clamps a disappeared selection to the new final item when the list shrinks past it", async () => {
      currentSettings = orderedSnapshot(FIVE);
      mockSettingsApi.get.mockResolvedValue(currentSettings);
      const fire = fireSettingsUpdate();
      const { dispose } = renderPicker({ currentAgentId: "e" });
      await settle();

      currentSettings = orderedSnapshot(orderOf(["a", "b", "c"]));
      mockSettingsApi.get.mockResolvedValue(currentSettings);
      fire();
      await settle();

      expect(cardIds()).toEqual(["a", "b", "c"]);
      expect(target("agentPicker.provider.c").getAttribute("data-ac-state")).toBe("active");

      dispose();
    });

    it("keeps a surviving selection across a live add and renders the incoming ID in place", async () => {
      currentSettings = orderedSnapshot(orderOf(["a", "b"]));
      mockSettingsApi.get.mockResolvedValue(currentSettings);
      const fire = fireSettingsUpdate();
      const { dispose } = renderPicker({ currentAgentId: "b" });
      await settle();

      currentSettings = orderedSnapshot([
        byId("a"),
        agent({ id: "x", label: "Agent X", command: "cmd-x", order: 1 }),
        byId("b"),
      ]);
      mockSettingsApi.get.mockResolvedValue(currentSettings);
      fire();
      await settle();

      expect(cardIds()).toEqual(["a", "x", "b"]);
      expect(target("agentPicker.provider.b").getAttribute("data-ac-state")).toBe("active");

      dispose();
    });

    it("coalesces concurrent settings events into one queued refetch and unsubscribes on cleanup", async () => {
      currentSettings = orderedSnapshot(FIVE);
      const unlisten = vi.fn();
      let fire!: () => void;
      mockSettingsApi.onCodingAgentSettingsUpdated.mockImplementation((cb: () => void) => {
        fire = cb;
        return Promise.resolve(unlisten);
      });
      let getCalls = 0;
      let releaseDeferred!: () => void;
      mockSettingsApi.get.mockImplementation(() => {
        getCalls += 1;
        if (getCalls === 2) {
          return new Promise<AppSettings>((resolve) => {
            releaseDeferred = () => resolve(currentSettings);
          });
        }
        return Promise.resolve(currentSettings);
      });
      const { dispose } = renderPicker({ currentAgentId: "c" });
      await settle();
      expect(getCalls).toBe(1);

      fire();
      fire();
      await settle();
      // The first event's fetch is still in flight; the second only queued.
      expect(getCalls).toBe(2);

      releaseDeferred();
      await settle();
      expect(getCalls).toBe(3);

      dispose();
      await settle();
      expect(unlisten).toHaveBeenCalledTimes(1);
    });

    it("keeps the environment-key sort while the tool list stays in vector order", async () => {
      const base = settings();
      currentSettings = orderedSnapshot([byId("b"), byId("a")], false, {
        codingAgentProfiles: {
          ...base.codingAgentProfiles,
          profilesByAgent: {
            ...base.codingAgentProfiles.profilesByAgent,
            b: {
              A: {
                enabled: true,
                command: "",
                env: { ZETA: "1", ALPHA: "2" },
                notes: "",
              },
            },
          },
        },
      });
      mockSettingsApi.get.mockResolvedValue(currentSettings);
      const { dispose } = renderPicker({ agentPath: REPO_PATH, currentAgentId: "b" });
      await settle();

      expect(cardIds()).toEqual(["b", "a"]);
      const envKeys = [
        ...target("agentPicker.profile.A.env").querySelectorAll<HTMLElement>(
          ".agent-profile-declared-env-key",
        ),
      ].map((node) => node.textContent);
      expect(envKeys).toEqual(["ALPHA", "ZETA"]);

      dispose();
    });
  });
  describe("#2475 picker open runs one preview round (#2484)", () => {
    function backendTotal(): number {
      return (
        mockSettingsApi.previewCodingAgentProfileSelection.mock.calls.length +
        mockSettingsApi.previewSelectionLockRemoval.mock.calls.length +
        mockSettingsApi.getReplicaSelectionDefault.mock.calls.length
      );
    }

    function renderWgPicker(overrides: Parameters<typeof renderPicker>[0] = {}) {
      return renderPicker({
        agentPath: WG_REPLICA_PATH,
        scopeContext: WG_SCOPE_CONTEXT,
        currentRequestedProfile: "A",
        ...overrides,
      });
    }

    it("issue_2475_duplicate_triggering_open_collapses_to_one_batch", async () => {
      // Fixture B: the snapshot selects agent index 1 and the requested profile
      // differs from the initial "A", so both writes feed the preview effect.
      mockSettingsApi.resolveCodingAgentProfile.mockImplementation(() =>
        Promise.resolve(resolution({ requestedProfile: "C", effectiveProfile: "A" })),
      );
      const { dispose } = renderWgPicker({ currentAgentId: "claude", currentRequestedProfile: "C" });
      await settle();

      expect(mockSettingsApi.previewCodingAgentProfileSelection).toHaveBeenCalledTimes(3);
      expect(backendTotal()).toBe(7);

      dispose();
    });

    it("issue_2475_already_clean_open_keeps_seven_backend_calls", async () => {
      // Fixture A: requested "A" equals the initial selection, one round before and after.
      const { dispose } = renderWgPicker({ currentAgentId: "codex", currentRequestedProfile: "A" });
      await settle();

      expect(mockSettingsApi.previewCodingAgentProfileSelection).toHaveBeenCalledTimes(3);
      expect(backendTotal()).toBe(7);

      dispose();
    });

    async function microtasks(count = 8): Promise<void> {
      // Microtask flushes only: a regression that defers to a timer task stays unobserved.
      for (let index = 0; index < count; index += 1) await Promise.resolve();
    }

    function controlledSettings(): () => void {
      let resolveSettings: (value: AppSettings) => void = () => {};
      mockSettingsApi.get.mockImplementation(
        () => new Promise<AppSettings>((resolve) => { resolveSettings = resolve; }),
      );
      return () => resolveSettings(currentSettings);
    }

    it("issue_2475_loading_message_appears_in_the_same_tick", async () => {
      mockSettingsApi.previewCodingAgentProfileSelection.mockImplementation(
        () => new Promise<PreviewCodingAgentProfileSelectionResult>(() => {}),
      );
      const releaseSettings = controlledSettings();
      const { dispose } = renderWgPicker();
      await microtasks();
      expect(maybe("agentPicker.previewBusy")).toBeNull();
      expect(mockSettingsApi.previewCodingAgentProfileSelection).not.toHaveBeenCalled();

      releaseSettings();
      await microtasks();

      // No timer task ran and no preview promise resolved: the message comes from the open itself.
      expect(mockSettingsApi.previewCodingAgentProfileSelection).toHaveBeenCalledTimes(3);
      expect(text("agentPicker.previewBusy")).toBe("Loading targets…");

      dispose();
    });

    it("issue_2475_scope_counts_are_never_absent_longer_than_today", async () => {
      const removeIds = [
        "agentPicker.removeScopeCount.replica",
        "agentPicker.removeScopeCount.kind",
        "agentPicker.removeScopeCount.workgroup",
      ];
      const assignIds = ["agentPicker.scope.kind", "agentPicker.scope.workgroup"];
      const ids = [...removeIds, ...assignIds];
      const samples: Record<string, string[]> = Object.fromEntries(ids.map((id) => [id, []]));
      // One sample per microtask boundary; absence is a recorded value, not a skip.
      const sample = () => {
        for (const id of ids) {
          const element = maybe(id);
          samples[id].push(element ? element.textContent?.replace(/\s+/g, " ").trim() ?? "" : "absent");
        }
      };
      const releaseSettings = controlledSettings();
      const { dispose } = renderWgPicker();
      sample();
      releaseSettings();
      for (let index = 0; index < 12; index += 1) {
        await Promise.resolve();
        sample();
      }

      const finals: Record<string, string> = {
        "agentPicker.removeScopeCount.replica": "0 protected",
        "agentPicker.removeScopeCount.kind": "2 of 3 protected",
        "agentPicker.removeScopeCount.workgroup": "3 of 4 protected",
      };
      for (const id of removeIds) {
        const seen = samples[id];
        // "—" is the pre-settings placeholder: allowed only in the sample taken before release.
        const phases = seen.map((value, index) =>
          value === "absent" || (index === 0 && value === "—") ? 0 : value === "…" ? 1 : value === finals[id] ? 2 : -1,
        );
        expect(seen[1], `${id}: first tick after settings`).toBe("…");
        expect(phases, `${id}: ${seen.join(" | ")}`).not.toContain(-1);
        expect(phases, `${id}: ${seen.join(" | ")}`).toEqual([...phases].sort((a, b) => a - b));
        expect(seen[seen.length - 1], id).toBe(finals[id]);
      }
      const assignFinals: Record<string, string> = {
        "agentPicker.scope.kind": "3 replicas",
        "agentPicker.scope.workgroup": "4 replicas",
      };
      for (const id of assignIds) {
        const seen = samples[id];
        const phases = seen.map((value) =>
          value === "absent" ? 0 : value.includes(" 0 replicas") ? 1 : value.includes(assignFinals[id]) ? 2 : -1,
        );
        expect(phases, `${id}: ${seen.join(" | ")}`).not.toContain(-1);
        expect(phases, `${id}: ${seen.join(" | ")}`).toEqual([...phases].sort((a, b) => a - b));
        expect(seen[seen.length - 1], id).toContain(assignFinals[id]);
      }

      dispose();
    });

    it("issue_2475_preview_error_text_is_unchanged", async () => {
      const message = "preview exploded: backend said no";
      mockSettingsApi.previewCodingAgentProfileSelection.mockImplementation(
        (req: { scope: string }) =>
          req.scope === "replica"
            ? Promise.reject(new Error(message))
            : Promise.resolve(previewResult({ scope: req.scope as ProfileAssignmentScope })),
      );
      const { dispose } = renderWgPicker();
      await settle();

      expect(text("agentPicker.previewError")).toBe(message);

      dispose();
    });
  });
});
