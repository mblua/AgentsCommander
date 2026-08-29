// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "solid-js/web";
import AgentPickerModal, {
  type AgentPickerScopeContext,
  type AgentPickerSelection,
} from "./AgentPickerModal";
import type {
  AgentConfig,
  AppSettings,
  CodingAgentProfileResolution,
  PreviewCodingAgentProfileSelectionResult,
  ApplyCodingAgentProfileSelectionResult,
  ProfileAssignmentTarget,
} from "../../shared/types";
import { resolveProfilePreview } from "../../shared/profile-utils";

const mockSettingsApi = vi.hoisted(() => ({
  get: vi.fn(),
  resolveCodingAgentProfile: vi.fn(),
  previewCodingAgentProfileSelection: vi.fn(),
  applyCodingAgentProfileSelection: vi.fn(),
}));

vi.mock("../../shared/ipc", () => ({
  SettingsAPI: {
    get: mockSettingsApi.get,
    resolveCodingAgentProfile: mockSettingsApi.resolveCodingAgentProfile,
    previewCodingAgentProfileSelection: mockSettingsApi.previewCodingAgentProfileSelection,
    applyCodingAgentProfileSelection: mockSettingsApi.applyCodingAgentProfileSelection,
  },
}));

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
  return {
    defaultShell: "pwsh",
    defaultShellArgs: [],
    sidebarAlwaysOnTop: false,
    sidebarStyle: "noir-minimal",
    themeLight: true,
    telegramNetworkPollErrorLogging: {
      firstFailureLevel: "warn",
      transientRepeatLevel: "debug",
      sustainedLevel: "warn",
      sustainedAfterSeconds: 60,
      sustainedRepeatSeconds: 300,
      recoveryLevel: "info",
    },
    raiseTerminalOnClick: true,
    coordSortByActivity: false,
    alwaysShowSelectedWorkgroup: true,
    restoreCoordinatorWakeState: true,
    soundsEnabled: true,
    teamIdleBeepEnabled: true,
    webServerEnabled: false,
    webServerPort: 8765,
    webServerBind: "127.0.0.1",
    apiServerEnabled: false,
    apiServerPort: 8766,
    apiServerBind: "127.0.0.1",
    terminalSnapshotsEnabled: false,
    voiceToTextEnabled: false,
    voiceAutoExecute: false,
    voiceAutoExecuteDelay: 15,
    geminiApiKey: "",
    geminiModel: "gemini-2.5-flash",
    sidebarZoom: 1,
    terminalZoom: 1,
    guideZoom: 1,
    mainZoom: 1,
    sidebarGeometry: null,
    terminalGeometry: null,
    mainGeometry: null,
    mainSidebarWidth: 360,
    mainSidebarSide: "right",
    mainAlwaysOnTop: false,
    mainResourceMonitorAttached: false,
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
    telegramBots: [],
    onboardingDismissed: true,
    projectPaths: [],
    projectPath: null,
    autoGenerateTaskTitle: true,
    agentTemplatesPath: null,
    specBoardEnabled: false,
    gitSweepConcurrency: 1,
    gitSweepMinIntervalSecs: 10,
    resourceMonitorEnabled: true,
    maxConcurrentAgentProcesses: 3,
    resourceWatchdogAction: "warn",
    agentGroupWarnPrivateBytes: 8 * 1024 ** 3,
    agentGroupKillPrivateBytes: 12 * 1024 ** 3,
    agentProcessKillPrivateBytes: 12 * 1024 ** 3,
    resourceKeepLastSnapshot: true,
    resourceBackoffPolling: true,
    coordinatorIdleBadgeYellowMinutes: 30,
    coordinatorIdleBadgeRedMinutes: 60,
    coordinatorAutoCloseEnabled: true,
    coordinatorAutoCloseMinutes: 60,
    coordinatorAutoCloseSkipTelegramAssigned: false,
    coordinatorCascadeCloseEnabled: true,
    npmUpdateNotificationsEnabled: true,
    autoSelfClearEnabled: true,
    autoSelfClearByAgent: {},
    agentAutoUpdateByCommand: {},
    containerCredentialsFromHost: true,
    logLevel: null,
    activityLogEnabled: false,
    ...overrides,
    archivedProjectPaths: overrides.archivedProjectPaths ?? [],
  };
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

function makeTarget(name: string, wg: string, liveSessions: string[]): ProfileAssignmentTarget {
  return {
    workgroupName: wg,
    workgroupPath: `C:\\repos\\proj\\.ac\\${wg}`,
    replicaName: name,
    replicaPath: `C:\\repos\\proj\\.ac\\${wg}\\__agent_${name}`,
    identityPath: `C:\\repos\\proj\\.ac\\${wg}\\__agent_${name}\\identity.json`,
    originProject: "proj",
    liveSessionIds: liveSessions,
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
  mockSettingsApi.previewCodingAgentProfileSelection.mockImplementation((req: { scope: string }) => {
    if (req.scope === "kind") {
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
  });
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
    mockSettingsApi.get.mockResolvedValue(currentSettings);
    mockSettingsApi.resolveCodingAgentProfile.mockImplementation(defaultBackendResolve);
    scopeAwarePreview();
    mockSettingsApi.applyCodingAgentProfileSelection.mockResolvedValue(applyResult());
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
    expect(text("agentPicker.comparison.row.claude")).not.toContain("claude --dangerously-skip-permissions");

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
    expect(text("agentPicker.comparison.row.claude")).not.toContain("claude --dangerously-skip-permissions");

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
});
