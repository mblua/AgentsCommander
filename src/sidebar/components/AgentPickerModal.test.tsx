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
    gitPullBefore: false,
    excludeGlobalClaudeMd: true,
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
    injectRtkHook: false,
    informWhenRtkInstalled: true,
    webServerEnabled: false,
    webServerPort: 8765,
    webServerBind: "127.0.0.1",
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
        excludeGlobalClaudeMd: false,
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
    },
    telegramBots: [],
    onboardingDismissed: true,
    projectPaths: [],
    projectPath: null,
    rtkPromptDismissed: false,
    autoGenerateTaskTitle: true,
    agentTemplatesPath: null,
    specBoardEnabled: false,
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
    ...overrides,
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
    currentRequestedProfile: string | null;
    scopeContext: AgentPickerScopeContext | undefined;
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
        currentRequestedProfile={overrides.currentRequestedProfile}
        scopeContext={overrides.scopeContext}
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
    expect(target("agentPicker.projected")).toBeTruthy();
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

  it("updates active provider state and projected command when a coding agent is clicked", async () => {
    const { dispose } = renderPicker({ agentPath: REPO_PATH });
    await settle();

    target<HTMLButtonElement>("agentPicker.provider.claude").click();
    await settle();

    expect(target("agentPicker.provider.claude").getAttribute("data-ac-state")).toBe("active");
    // Effective Projection chosen-pair shows the selected coding agent + resolved command.
    expect(text("agentPicker.projected")).toContain("Claude Code");
    expect(text("agentPicker.projected")).toContain("claude --dangerously-skip-permissions");

    dispose();
  });

  it("shows fallback on missing profile cards and in the projected panel", async () => {
    const { dispose } = renderPicker({ agentPath: REPO_PATH });
    await settle();

    target<HTMLButtonElement>("agentPicker.profile.C").click();
    await settle();

    expect(target("agentPicker.profile.C").getAttribute("data-ac-state")).toBe("active");
    expect(text("agentPicker.profile.C")).toContain("Fallback C->B");
    expect(text("agentPicker.fallback")).toContain("C-REVIEW is not configured");
    expect(text("agentPicker.fallback")).toContain("A remains the final fallback");
    // Effective Projection chosen-pair Resolution row reports the fallback hop.
    expect(text("agentPicker.projected")).toContain("C-REVIEW → B-FAST (fallback)");

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

  it("keeps kind apply disabled until preview succeeds and the typed phrase matches", async () => {
    const { dispose } = renderPicker({
      agentPath: WG_REPLICA_PATH,
      scopeContext: WG_SCOPE_CONTEXT,
      currentRequestedProfile: "A",
    });
    await settle();

    clickRadio("agentPicker.scope.kind");
    await settle();

    expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);
    const phrase = target<HTMLInputElement>("agentPicker.kindConfirm").getAttribute("placeholder")!;
    expect(phrase).toBe("APPLY codex:A TO 3 REPLICAS WITHOUT RESTART");

    const input = target<HTMLInputElement>("agentPicker.kindConfirm");
    input.value = "APPLY codex:A TO 3 REPLICAS";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);

    input.value = phrase;
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

    dispose();
  });

  it("resets the typed confirmation when the profile changes", async () => {
    const { dispose } = renderPicker({
      agentPath: WG_REPLICA_PATH,
      scopeContext: WG_SCOPE_CONTEXT,
      currentRequestedProfile: "A",
    });
    await settle();

    clickRadio("agentPicker.scope.kind");
    await settle();
    const input = target<HTMLInputElement>("agentPicker.kindConfirm");
    input.value = target<HTMLInputElement>("agentPicker.kindConfirm").getAttribute("placeholder")!;
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

    // Changing the profile must reset the typed confirmation and re-disable apply.
    target<HTMLButtonElement>("agentPicker.profile.B").click();
    await settle();
    expect(target<HTMLInputElement>("agentPicker.kindConfirm").value).toBe("");
    expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);

    dispose();
  });

  it("sends confirmedTargetFingerprint and typedConfirmation for a kind apply", async () => {
    const { dispose, onSelect } = renderPicker({
      agentPath: WG_REPLICA_PATH,
      scopeContext: WG_SCOPE_CONTEXT,
      currentRequestedProfile: "A",
    });
    await settle();

    clickRadio("agentPicker.scope.kind");
    await settle();
    const input = target<HTMLInputElement>("agentPicker.kindConfirm");
    const phrase = input.getAttribute("placeholder")!;
    input.value = phrase;
    input.dispatchEvent(new Event("input", { bubbles: true }));
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
        typedConfirmation: phrase,
      }),
    );
    expect(onSelect).toHaveBeenCalledWith(
      expect.objectContaining({ scope: "kind", restartSessions: false }),
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
    expect(text("agentPicker.targets")).toContain("3 replica(s) across 3 workgroup(s)");
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
    const input = target<HTMLInputElement>("agentPicker.kindConfirm");
    input.value = input.getAttribute("placeholder")!;
    input.dispatchEvent(new Event("input", { bubbles: true }));
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
    // Modal stays open; selection is not committed; typed confirmation is reset.
    expect(onSelect).not.toHaveBeenCalled();
    expect(maybe("agentPicker.modal")).toBeTruthy();

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
    expect(text("agentPicker.projected")).toContain("Claude Code");

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
});
