// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "solid-js/web";
import AgentPickerModal, { type AgentPickerSelection } from "./AgentPickerModal";
import type { AgentConfig, AppSettings, CodingAgentProfileResolution } from "../../shared/types";
import { SettingsAPI } from "../../shared/ipc";
import { resolveProfilePreview } from "../../shared/profile-utils";

const mockSettingsApi = vi.hoisted(() => ({
  get: vi.fn(),
  resolveCodingAgentProfile: vi.fn(),
  setAgentDefaultProfile: vi.fn(),
  setInstanceProfileOverride: vi.fn(),
}));

vi.mock("../../shared/ipc", () => ({
  SettingsAPI: {
    get: mockSettingsApi.get,
    resolveCodingAgentProfile: mockSettingsApi.resolveCodingAgentProfile,
    setAgentDefaultProfile: mockSettingsApi.setAgentDefaultProfile,
    setInstanceProfileOverride: mockSettingsApi.setInstanceProfileOverride,
  },
}));

const AC_AGENT_PATH = "C:\\Users\\maria\\0_repos\\AgentsCommander_ac\\.ac\\__agent_architect";
const REPO_PATH = "C:\\work\\repo";

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
    isolateCodexHome: false,
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
      schemaVersion: 1,
      letters: {
        A: { name: "" },
        B: { name: "fast" },
        C: { name: "review" },
      },
      agentDefaults: { architect: "B" },
      matrix: {
        codex: {
          A: {
            enabled: true,
            argv: ["--model", "gpt-5"],
            env: { OPENAI_MODEL: "gpt-5" },
            notes: "baseline",
          },
          B: {
            enabled: true,
            argv: ["--profile", "fast"],
            env: { CODEX_PROFILE: "fast" },
            notes: "fast lane",
          },
        },
        claude: {
          A: {
            enabled: true,
            argv: ["--dangerously-skip-permissions"],
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
  const requested = requestedProfile ?? currentSettings.codingAgentProfiles.agentDefaults.architect ?? "A";
  const preview = resolveProfilePreview(currentSettings.codingAgentProfiles, agentId, requested);
  return Promise.resolve(
    resolution({
      ...preview,
      requestedProfileInput: requestedProfile ?? null,
      agentDefaultProfile: currentSettings.codingAgentProfiles.agentDefaults.architect ?? null,
    }),
  );
}

async function settle(times = 3): Promise<void> {
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

function text(testId: string): string {
  return target(testId).textContent?.replace(/\s+/g, " ").trim() ?? "";
}

function renderPicker(
  overrides: Partial<{
    sessionName: string;
    agentPath: string | null;
    currentAgentId: string | null;
    currentRequestedProfile: string | null;
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
        agentPath={overrides.agentPath === undefined ? AC_AGENT_PATH : overrides.agentPath}
        currentAgentId={overrides.currentAgentId ?? "codex"}
        currentRequestedProfile={overrides.currentRequestedProfile}
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
    mockSettingsApi.setAgentDefaultProfile.mockReset();
    mockSettingsApi.setInstanceProfileOverride.mockReset();
    mockSettingsApi.get.mockResolvedValue(currentSettings);
    mockSettingsApi.resolveCodingAgentProfile.mockImplementation(defaultBackendResolve);
    mockSettingsApi.setAgentDefaultProfile.mockResolvedValue(undefined);
    mockSettingsApi.setInstanceProfileOverride.mockResolvedValue(undefined);
  });

  afterEach(() => {
    document.body.innerHTML = "";
    vi.clearAllMocks();
  });

  it("renders the Variant C regions and footer actions", async () => {
    const { dispose } = renderPicker();
    await settle();

    expect(target("agentPicker.providers")).toBeTruthy();
    expect(target("agentPicker.profiles")).toBeTruthy();
    expect(target("agentPicker.projected")).toBeTruthy();
    expect(target("agentPicker.cancel")).toBeTruthy();
    expect(target("agentPicker.setDefault")).toBeTruthy();
    expect(target("agentPicker.setInstance")).toBeTruthy();
    expect(target("agentPicker.provider.codex").getAttribute("data-ac-agent-id")).toBe("codex");
    expect(target("agentPicker.profile.A").getAttribute("data-ac-profile-letter")).toBe("A");
    expect(target("agentPicker.profile.A").getAttribute("data-ac-agent-id")).toBe("codex");
    expect(document.querySelector('[data-component="Coding Agents selector panel"]')).toBeTruthy();
    expect(document.querySelector('[data-component="Selected profile projected parameters panel"]')).toBeTruthy();

    dispose();
  });

  it("preserves the no-agent empty state", async () => {
    currentSettings = settings({ agents: [] });
    mockSettingsApi.get.mockResolvedValue(currentSettings);
    const { dispose } = renderPicker({ currentAgentId: null });
    await settle();

    expect(document.body.textContent).toContain("No agents configured. Add agents in Settings.");
    expect(target<HTMLButtonElement>("agentPicker.setInstance").disabled).toBe(true);

    dispose();
  });

  it("updates active provider state and projected parameters when a coding agent is clicked", async () => {
    const { dispose } = renderPicker();
    await settle();

    target<HTMLButtonElement>("agentPicker.provider.claude").click();
    await settle();

    expect(target("agentPicker.provider.claude").getAttribute("data-ac-state")).toBe("active");
    expect(text("agentPicker.projected")).toContain("Selected coding agent: Claude Code");
    expect(text("agentPicker.profile.B")).toContain("missing; launches A");

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
    expect(text("agentPicker.projected")).toContain("C-REVIEW resolves to B-FAST via fallback");

    dispose();
  });

  it("commits instance selections with requested and effective profiles", async () => {
    const { dispose, onSelect } = renderPicker({ agentPath: REPO_PATH });
    await settle();

    target<HTMLButtonElement>("agentPicker.profile.C").click();
    await settle();
    target<HTMLButtonElement>("agentPicker.setInstance").click();
    await settle();

    expect(onSelect).toHaveBeenCalledWith(
      expect.objectContaining({
        requestedProfile: "C",
        effectiveProfile: "B",
        scope: "instance",
      }),
    );

    dispose();
  });

  it("keeps normal repo paths from inheriting matching AC agent defaults", async () => {
    currentSettings = settings({
      codingAgentProfiles: {
        ...settings().codingAgentProfiles,
        agentDefaults: { repo: "C" },
      },
    });
    mockSettingsApi.get.mockResolvedValue(currentSettings);
    const { dispose, onSelect } = renderPicker({
      sessionName: "repo",
      agentPath: REPO_PATH,
    });
    await settle();

    expect(target<HTMLButtonElement>("agentPicker.setDefault").disabled).toBe(true);
    expect(text("agentPicker.projected")).toContain("Default: A");
    target<HTMLButtonElement>("agentPicker.setInstance").click();
    await settle();

    expect(onSelect).toHaveBeenCalledWith(
      expect.objectContaining({
        requestedProfile: null,
        effectiveProfile: "A",
        scope: "instance",
      }),
    );

    dispose();
  });

  it("keeps explicit current requested profiles for normal repo paths", async () => {
    const { dispose, onSelect } = renderPicker({
      agentPath: REPO_PATH,
      currentRequestedProfile: "B",
    });
    await settle();

    target<HTMLButtonElement>("agentPicker.setInstance").click();
    await settle();

    expect(onSelect).toHaveBeenCalledWith(
      expect.objectContaining({
        requestedProfile: "B",
        effectiveProfile: "B",
        scope: "instance",
      }),
    );

    dispose();
  });

  it("ignores stale backend preview results after the selected coding agent changes", async () => {
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
      resolution({
        requestedProfile: "C",
        effectiveProfile: "C",
        fallbackChain: ["C"],
        warnings: ["old warning"],
      }),
    );
    await settle();
    expect(text("agentPicker.fallback")).not.toContain("old warning");
    expect(text("agentPicker.projected")).toContain("Selected coding agent: Claude Code");

    pending[1].resolve(
      resolution({
        requestedProfile: "A",
        effectiveProfile: "A",
        fallbackChain: ["A"],
      }),
    );
    await settle();
    expect(text("agentPicker.projected")).toContain("Requested A resolves to A as configured");

    dispose();
  });

  it("clears existing instance override when setting a new default", async () => {
    const { dispose, onSelect } = renderPicker({ currentRequestedProfile: "C" });
    await settle();

    target<HTMLButtonElement>("agentPicker.profile.B").click();
    await settle();
    target<HTMLButtonElement>("agentPicker.setDefault").click();
    await settle();

    expect(SettingsAPI.setAgentDefaultProfile).toHaveBeenCalledWith(AC_AGENT_PATH, "B");
    expect(SettingsAPI.setInstanceProfileOverride).toHaveBeenCalledWith(AC_AGENT_PATH, null);
    expect(onSelect).toHaveBeenCalledWith(
      expect.objectContaining({
        requestedProfile: "B",
        effectiveProfile: "B",
        scope: "default",
      }),
    );

    dispose();
  });

  it("does not project argv or env from disabled profile cells", async () => {
    currentSettings = settings({
      codingAgentProfiles: {
        ...settings().codingAgentProfiles,
        agentDefaults: {},
        matrix: {
          codex: {
            A: {
              enabled: false,
              argv: ["--stale"],
              env: { STALE_ENV: "1" },
              notes: "stale",
            },
          },
        },
      },
    });
    mockSettingsApi.get.mockResolvedValue(currentSettings);
    const { dispose } = renderPicker({ agentPath: REPO_PATH });
    await settle();

    expect(text("agentPicker.projected")).toContain("Profile args none");
    expect(text("agentPicker.projected")).toContain("Profile env none");
    expect(text("agentPicker.projected")).not.toContain("--stale");
    expect(text("agentPicker.projected")).not.toContain("STALE_ENV");

    dispose();
  });

  it("does not convert focused footer Enter into the instance action", async () => {
    const { dispose, onSelect } = renderPicker();
    await settle();

    const defaultButton = target<HTMLButtonElement>("agentPicker.setDefault");
    defaultButton.focus();
    defaultButton.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    await settle();
    expect(onSelect).not.toHaveBeenCalled();

    defaultButton.click();
    await settle();
    expect(onSelect).toHaveBeenCalledWith(expect.objectContaining({ scope: "default" }));

    dispose();
  });

  it("surfaces backend profile-resolution warnings without disabling instance launch", async () => {
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
    expect(target<HTMLButtonElement>("agentPicker.setInstance").disabled).toBe(false);

    dispose();
  });

  it("keeps fallback explanation visible when backend warnings are also present", async () => {
    mockSettingsApi.resolveCodingAgentProfile.mockResolvedValue(
      resolution({
        requestedProfile: "C",
        effectiveProfile: "B",
        fallbackChain: ["C", "B"],
        fallbackApplied: true,
        warnings: ["persisted profile required fallback"],
      }),
    );
    const { dispose } = renderPicker({ currentRequestedProfile: "C" });
    await settle();

    expect(text("agentPicker.fallback")).toContain("Profile warning: persisted profile required fallback");
    expect(text("agentPicker.fallback")).toContain("C-REVIEW is not configured");
    expect(text("agentPicker.fallback")).toContain("A remains the final fallback");

    dispose();
  });

  it("shows Codex home isolation for command-based Codex agents with custom IDs", async () => {
    currentSettings = settings({
      agents: [
        agent({
          id: "codex-fast",
          label: "Codex Fast",
          command: "codex",
          isolateCodexHome: false,
        }),
      ],
      codingAgentProfiles: {
        ...settings().codingAgentProfiles,
        matrix: {
          "codex-fast": {
            A: {
              enabled: true,
              argv: [],
              env: {},
              notes: "",
            },
          },
        },
      },
    });
    mockSettingsApi.get.mockResolvedValue(currentSettings);
    const { dispose } = renderPicker({ currentAgentId: "codex-fast" });
    await settle();

    expect(text("agentPicker.projected")).toContain("Selected coding agent: Codex Fast");
    expect(text("agentPicker.projected")).toContain("Codex home isolation disabled");

    dispose();
  });
});
