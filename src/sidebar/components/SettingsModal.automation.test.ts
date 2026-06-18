// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { render } from "solid-js/web";
import SettingsModal from "./SettingsModal";
import type { AppSettings } from "../../shared/types";
import { SettingsAPI } from "../../shared/ipc";

vi.mock("../../shared/ipc", () => ({
  SettingsAPI: {
    get: vi.fn(() => Promise.resolve(settings())),
    update: vi.fn(() => Promise.resolve()),
    saveDraft: vi.fn(() => Promise.resolve()),
    updateCodingAgentProfiles: vi.fn(() => Promise.resolve()),
    updateCodingAgentEnvSettings: vi.fn(() => Promise.resolve()),
    getWebServerStatus: vi.fn(() => Promise.resolve(false)),
    openWebRemote: vi.fn(() => Promise.resolve()),
    startWebServer: vi.fn(() => Promise.resolve(false)),
    stopWebServer: vi.fn(() => Promise.resolve(false)),
    sweepRtkHook: vi.fn(() => Promise.resolve({ total: 0, updated: 0, errors: [] })),
  },
  TelegramAPI: {
    sendTest: vi.fn(() => Promise.resolve(0)),
  },
  ReposAPI: {
    search: vi.fn(() => Promise.resolve([])),
  },
}));

vi.mock("../../shared/stores/settings", () => ({
  settingsStore: {
    get current() {
      return settings();
    },
    refresh: vi.fn(),
  },
}));

vi.mock("../../shared/sound", () => ({
  setSoundsEnabled: vi.fn(),
}));

vi.mock("../stores/sessions", () => ({
  sessionsStore: {
    setRepos: vi.fn(),
  },
}));

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
      {
        id: "codex",
        label: "Codex",
        command: "codex",
        color: "#10b981",
        gitPullBefore: false,
        excludeGlobalClaudeMd: true,
        envs: [],
        isolatedHome: false,
      },
    ],
    codingAgentProfiles: {
      schemaVersion: 2,
      profileSlots: { A: { label: "" } },
      defaultProfileByAgent: {},
      profilesByAgent: {},
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
    ...overrides,
  };
}

async function settle(): Promise<void> {
  await Promise.resolve();
  await new Promise<void>((resolve) => setTimeout(resolve, 0));
}

function byTestId<T extends Element = Element>(testId: string): T {
  const element = document.querySelector<T>(`[data-ac-testid="${testId}"]`);
  if (!element) throw new Error(`missing selector ${testId}`);
  return element;
}

// #526: the unified Coding Agents screen keeps each agent's config editor
// collapsed by default (the resting screen reads as the prototype). Clicking the
// row head expands the inline editor that holds label/command/env/isolation.
function expandAgentRow(index = 0): void {
  const toggle = document.querySelector<HTMLElement>(
    `[data-ac-testid="settings.agentRow.${index}.toggle"]`,
  );
  if (!toggle) throw new Error(`missing agent row toggle ${index}`);
  toggle.click();
}

describe("SettingsModal automation hooks", () => {
  afterEach(() => {
    document.body.innerHTML = "";
    vi.clearAllMocks();
  });

  it("keeps row parent and disabled Codex preset selectors addressable", async () => {
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {}, section: "agents" }),
      root,
    );
    await settle();
    expandAgentRow(0);
    await settle();

    const row = document.querySelector('[data-ac-testid="settings.agentRow.0"]');
    const label = document.querySelector('[data-ac-testid="settings.agentRow.0.label"]');
    const codexPreset = document.querySelector<HTMLButtonElement>(
      '[data-ac-testid="settings.agentPreset.codex"]',
    );

    expect(row).toBeTruthy();
    expect(label).toBeTruthy();
    expect(codexPreset).toBeTruthy();
    expect(codexPreset?.disabled).toBe(true);
    expect(codexPreset?.getAttribute("data-ac-state")).toBe("disabled");

    dispose();
  });

  it("renders agents selectors after clicking the Coding Agents tab", async () => {
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {} }),
      root,
    );
    await settle();

    const agentsTab = document.querySelector<HTMLButtonElement>(
      '[data-ac-testid="settings.tab.agents"]',
    );
    agentsTab?.click();
    await settle();
    expandAgentRow(0);
    await settle();

    expect(agentsTab?.getAttribute("data-ac-state")).toBe("active");
    expect(document.querySelector('[data-ac-testid="settings.agentRow.0"]')).toBeTruthy();
    expect(document.querySelector('[data-ac-testid="settings.agentRow.0.label"]')).toBeTruthy();
    expect(document.querySelector('[data-ac-testid="settings.agentPreset.codex"]')).toBeTruthy();
    expect(document.querySelector('[data-ac-testid="settings.agent.addCustom"]')).toBeTruthy();

    dispose();
  });

  it("exposes environment and Codex isolation controls for automation", async () => {
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {}, section: "agents" }),
      root,
    );
    await settle();
    expandAgentRow(0);
    await settle();

    expect(byTestId("settings.agentRow.0.env")).toBeTruthy();
    expect(byTestId("settings.agentRow.0.env.empty")).toBeTruthy();
    const addEnv = byTestId<HTMLButtonElement>("settings.agentRow.0.env.add");
    addEnv.click();
    await settle();

    expect(byTestId("settings.agentRow.0.envRow.0").getAttribute("data-ac-state")).toBe("enabled");
    expect(byTestId<HTMLInputElement>("settings.agentRow.0.envRow.0.key")).toBeTruthy();
    expect(byTestId<HTMLInputElement>("settings.agentRow.0.envRow.0.value")).toBeTruthy();
    expect(byTestId<HTMLInputElement>("settings.agentRow.0.envRow.0.enabled")).toBeTruthy();
    expect(byTestId("settings.agentRow.0.envRow.0.source").textContent).toContain("user");
    expect(byTestId("settings.agentRow.0.envRow.0.source").getAttribute("data-ac-env-source")).toBe("user");
    expect(byTestId<HTMLButtonElement>("settings.agentRow.0.envRow.0.delete").disabled).toBe(false);

    const isolation = byTestId<HTMLInputElement>("settings.agentRow.0.codexHomeIsolation");
    expect(isolation.checked).toBe(false);
    expect(isolation.getAttribute("data-ac-state")).toBe("unchecked");
    isolation.checked = true;
    isolation.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();

    expect(byTestId<HTMLInputElement>("settings.agentRow.0.codexHomeIsolation").checked).toBe(true);
    expect(byTestId("settings.agentRow.0.codexHomeIsolation.preview").getAttribute("data-ac-state")).toBe("isolated");

    dispose();
  });

  it("keeps an early custom agent draft when the fresh load resolves late", async () => {
    let resolveLoadedSettings: (value: AppSettings) => void = () => {};
    vi.mocked(SettingsAPI.get).mockReturnValueOnce(
      new Promise<AppSettings>((resolve) => {
        resolveLoadedSettings = resolve;
      }),
    );

    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {}, section: "agents" }),
      root,
    );
    await settle();

    const addCustom = document.querySelector<HTMLButtonElement>(
      '[data-ac-testid="settings.agent.addCustom"]',
    );
    addCustom?.click();
    await settle();

    expect(document.querySelector('[data-ac-testid="settings.agentRow.1"]')).toBeTruthy();
    expect(document.querySelector('[data-ac-testid="settings.agentRow.1.label"]')).toBeTruthy();

    resolveLoadedSettings(settings());
    await settle();

    expect(document.querySelector('[data-ac-testid="settings.agentRow.1"]')).toBeTruthy();
    expect(document.querySelector('[data-ac-testid="settings.agentRow.1.label"]')).toBeTruthy();

    dispose();
  });

  it("keeps early environment edits when the fresh load resolves late", async () => {
    let resolveLoadedSettings: (value: AppSettings) => void = () => {};
    vi.mocked(SettingsAPI.get).mockReturnValueOnce(
      new Promise<AppSettings>((resolve) => {
        resolveLoadedSettings = resolve;
      }),
    );

    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {}, section: "agents" }),
      root,
    );
    await settle();
    expandAgentRow(0);
    await settle();

    const addEnv = document.querySelector<HTMLButtonElement>('[data-ac-testid="settings.agentRow.0.env.add"]');
    if (!addEnv) throw new Error("missing environment add button");
    addEnv.click();
    await settle();

    const keyInput = document.querySelector<HTMLInputElement>('[data-ac-testid="settings.agentRow.0.envRow.0.key"]');
    const valueInput = document.querySelector<HTMLInputElement>('[data-ac-testid="settings.agentRow.0.envRow.0.value"]');
    if (!keyInput || !valueInput) throw new Error("missing environment inputs");
    keyInput.value = "USER_TOKEN";
    keyInput.dispatchEvent(new Event("input", { bubbles: true }));
    valueInput.value = "secret";
    valueInput.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();

    resolveLoadedSettings(settings());
    await settle();

    document.querySelector<HTMLButtonElement>('[data-ac-testid="settings.save"]')?.click();
    await settle();

    const saved = vi.mocked(SettingsAPI.saveDraft).mock.calls[0]?.[0];
    expect(saved?.agents[0]?.envs).toEqual([
      {
        key: "USER_TOKEN",
        value: "secret",
        source: "user",
        enabled: true,
      },
    ]);

    dispose();
  });

  it("renders coding-agent rails, profile cards, command inputs, env rows, and badges", async () => {
    vi.mocked(SettingsAPI.get).mockResolvedValueOnce(settings({
      agents: [
        {
          id: "codex",
          label: "Codex",
          command: "codex",
          color: "#10b981",
          gitPullBefore: false,
          excludeGlobalClaudeMd: true,
          envs: [],
          isolatedHome: false,
        },
        {
          id: "claude",
          label: "Claude Code",
          command: "claude",
          color: "#d97706",
          gitPullBefore: false,
          excludeGlobalClaudeMd: false,
          envs: [],
          isolatedHome: false,
        },
      ],
      codingAgentProfiles: {
        schemaVersion: 2,
        profileSlots: {
          A: { label: "" },
          B: { label: "fast" },
        },
        defaultProfileByAgent: {},
        profilesByAgent: {
          codex: {
            A: {
              enabled: true,
              command: "codex --model gpt-5-codex",
              env: { OPENAI_ORG: "ac-prod" },
              notes: "",
            },
          },
          // Claude configures B; codex does not → codex B renders as MISSING.
          claude: {
            A: { enabled: true, command: "claude", env: {}, notes: "" },
            B: { enabled: true, command: "claude --model opus", env: {}, notes: "" },
          },
        },
      },
    }));
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {}, section: "profiles" }),
      root,
    );
    await settle();

    // Rail per coding agent, subtitle = binary basename (not model/sandbox).
    expect(byTestId("settings.profiles.section")).toBeTruthy();
    expect(byTestId("settings.profiles.rails")).toBeTruthy();
    expect(byTestId("settings.profileRail.0").getAttribute("data-ac-agent-id")).toBe("codex");
    expect(byTestId("settings.profileRail.0.subtitle").textContent).toContain("codex");

    // Profile A card: configured → MATCH, one command input, an env row.
    expect(byTestId("settings.profileCard.0.A").getAttribute("data-ac-state")).toBe("match");
    expect(byTestId("settings.profileCard.0.A.badge").textContent).toContain("MATCH");
    expect(byTestId<HTMLInputElement>("settings.profileCard.0.A.command").value).toBe("codex --model gpt-5-codex");
    expect(byTestId<HTMLInputElement>("settings.profileCard.0.A.label")).toBeTruthy();
    expect(byTestId("settings.profileCard.0.A.env")).toBeTruthy();
    expect(byTestId<HTMLInputElement>("settings.profileCard.0.A.envRow.0.key").value).toBe("OPENAI_ORG");
    expect(byTestId<HTMLInputElement>("settings.profileCard.0.A.envRow.0.value").value).toBe("ac-prod");

    // #538: codex has no B cell, but B no longer shows an "Add cell" / missing
    // box. The badge still honestly reports MISSING (codex's B falls back to its
    // base command until a command is typed).
    expect(byTestId("settings.profileCard.0.B").getAttribute("data-ac-state")).toBe("missing");
    expect(byTestId("settings.profileCard.0.B.badge").textContent).toContain("MISSING");
    // #538: the sub-line states what the cell launches via fallback (consistent
    // with the MISSING badge) instead of a contradictory bare "Configured".
    expect(
      byTestId("settings.profileCard.0.B").querySelector(".settings-profile-card-sub")?.textContent,
    ).toBe("Launches A");
    expect(document.querySelector('[data-ac-testid="settings.profileCard.0.B.missing"]')).toBeNull();
    expect(document.querySelector('[data-ac-testid="settings.profileCard.0.B.add"]')).toBeNull();
    // Instead it exposes the same expand/edit affordance as every cell: a chevron
    // toggle that reveals an (empty) command input the user can fill in.
    expect(byTestId<HTMLButtonElement>("settings.profileCard.0.B.toggle")).toBeTruthy();
    byTestId<HTMLButtonElement>("settings.profileCard.0.B.toggle").click();
    await settle();
    expect(byTestId<HTMLInputElement>("settings.profileCard.0.B.command").value).toBe("");
    // Claude rail: B has its own enabled cell (non-A, direct match) → CONFIGURED,
    // NOT MATCH. MATCH is reserved for the A baseline (#526).
    expect(byTestId("settings.profileCard.1.B").getAttribute("data-ac-state")).toBe("configured");
    expect(byTestId("settings.profileCard.1.B.badge").textContent).toContain("CONFIGURED");
    // Claude A is the baseline → MATCH.
    expect(byTestId("settings.profileCard.1.A").getAttribute("data-ac-state")).toBe("match");
    expect(byTestId("settings.profileCard.1.A.badge").textContent).toContain("MATCH");
    expect(byTestId<HTMLButtonElement>("settings.profiles.add").disabled).toBe(false);

    dispose();
  });

  it("collapses non-A profile cards by default and expands them on toggle", async () => {
    vi.mocked(SettingsAPI.get).mockResolvedValueOnce(settings({
      agents: [
        {
          id: "claude",
          label: "Claude Code",
          command: "claude",
          color: "#d97706",
          gitPullBefore: false,
          excludeGlobalClaudeMd: false,
          envs: [],
          isolatedHome: false,
        },
      ],
      codingAgentProfiles: {
        schemaVersion: 2,
        profileSlots: { A: { label: "" }, B: { label: "fast" } },
        defaultProfileByAgent: {},
        profilesByAgent: {
          claude: {
            A: { enabled: true, command: "claude", env: {}, notes: "" },
            B: { enabled: true, command: "claude --model opus", env: {}, notes: "" },
          },
        },
      },
    }));
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {}, section: "profiles" }),
      root,
    );
    await settle();

    // The "A" slot is expanded by default → its command input is present.
    expect(byTestId("settings.profileCard.0.A.command")).toBeTruthy();
    // A non-A configured slot is collapsed → command hidden until expanded.
    expect(document.querySelector('[data-ac-testid="settings.profileCard.0.B.command"]')).toBeNull();

    byTestId<HTMLButtonElement>("settings.profileCard.0.B.toggle").click();
    await settle();
    expect(byTestId<HTMLInputElement>("settings.profileCard.0.B.command").value).toBe("claude --model opus");

    dispose();
  });

  it("clears the right comparison rail when the right agent's Remove is clicked", async () => {
    vi.mocked(SettingsAPI.get).mockResolvedValueOnce(settings({
      agents: [
        {
          id: "codex",
          label: "Codex",
          command: "codex",
          color: "#10b981",
          gitPullBefore: false,
          excludeGlobalClaudeMd: true,
          envs: [],
          isolatedHome: false,
        },
        {
          id: "claude",
          label: "Claude Code",
          command: "claude",
          color: "#d97706",
          gitPullBefore: false,
          excludeGlobalClaudeMd: false,
          envs: [],
          isolatedHome: false,
        },
      ],
    }));
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {}, section: "agents" }),
      root,
    );
    await settle();

    // The comparison pair seeds left=codex, right=claude (second agent).
    expect(byTestId("settings.profileRail.1").getAttribute("data-ac-agent-id")).toBe("claude");
    // Removing the right-rail agent actually empties the rail (no positional re-fill).
    byTestId<HTMLButtonElement>("settings.agentRow.1.unuse").click();
    await settle();
    expect(byTestId("settings.profileRail.1").getAttribute("data-ac-agent-id")).toBeNull();
    // Claude is now available again and offers "Use" to re-add it.
    expect(document.querySelector('[data-ac-testid="settings.agentRow.1.use"]')).toBeTruthy();

    dispose();
  });

  it("shows the red invalid badge for a bad command string and blocks save", async () => {
    vi.mocked(SettingsAPI.get).mockResolvedValueOnce(settings({
      codingAgentProfiles: {
        schemaVersion: 2,
        profileSlots: { A: { label: "" } },
        defaultProfileByAgent: {},
        profilesByAgent: {
          codex: { A: { enabled: true, command: "codex", env: {}, notes: "" } },
        },
      },
    }));
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {}, section: "profiles" }),
      root,
    );
    await settle();

    const command = byTestId<HTMLInputElement>("settings.profileCard.0.A.command");
    command.value = 'codex --review --prompt "missing close';
    command.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();

    expect(byTestId("settings.profileCard.0.A").getAttribute("data-ac-state")).toBe("invalid");
    expect(byTestId("settings.profileCard.0.A.command.error")).toBeTruthy();

    byTestId<HTMLButtonElement>("settings.save").click();
    await settle();

    // Save is blocked by the parse error; no draft is persisted.
    expect(SettingsAPI.saveDraft).not.toHaveBeenCalled();
    expect(document.querySelector('[data-ac-testid="settings.modal"] .modal-save-error')).toBeTruthy();

    dispose();
  });

  it("renders the %AC_ROOT% template preview for env rows", async () => {
    vi.mocked(SettingsAPI.get).mockResolvedValueOnce(settings({
      codingAgentProfiles: {
        schemaVersion: 2,
        profileSlots: { A: { label: "" } },
        defaultProfileByAgent: {},
        profilesByAgent: {
          codex: {
            A: {
              enabled: true,
              command: "codex",
              env: { CODEX_HOME: "%AC_ROOT%\\.codex\\agents\\codex" },
              notes: "",
            },
          },
        },
      },
    }));
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {}, section: "profiles" }),
      root,
    );
    await settle();

    expect(byTestId("settings.profileCard.0.A.envRow.0.placeholder")).toBeTruthy();

    dispose();
  });

  it("keeps early profile cell edits when the fresh load resolves late", async () => {
    let resolveLoadedSettings: (value: AppSettings) => void = () => {};
    vi.mocked(SettingsAPI.get).mockReturnValueOnce(
      new Promise<AppSettings>((resolve) => {
        resolveLoadedSettings = resolve;
      }),
    );

    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {}, section: "profiles" }),
      root,
    );
    await settle();

    const commandInput = document.querySelector<HTMLInputElement>('[data-ac-testid="settings.profileCard.0.A.command"]');
    if (!commandInput) throw new Error("missing profile command input");
    commandInput.value = "codex --fast";
    commandInput.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();

    resolveLoadedSettings(settings());
    await settle();

    document.querySelector<HTMLButtonElement>('[data-ac-testid="settings.save"]')?.click();
    await settle();

    const saved = vi.mocked(SettingsAPI.saveDraft).mock.calls[0]?.[0];
    expect(saved?.codingAgentProfiles.profilesByAgent.codex?.A?.command).toBe("codex --fast");

    dispose();
  });

  it("uses the seeded RTK baseline when saving before the fresh load resolves", async () => {
    let resolveLoadedSettings: (value: AppSettings) => void = () => {};
    vi.mocked(SettingsAPI.get).mockReturnValueOnce(
      new Promise<AppSettings>((resolve) => {
        resolveLoadedSettings = resolve;
      }),
    );

    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {} }),
      root,
    );
    await settle();

    const rtkField = Array.from(
      document.querySelectorAll<HTMLLabelElement>(".settings-checkbox-field"),
    ).find((label) => label.textContent?.includes("Inject RTK hook"));
    const rtkCheckbox = rtkField?.querySelector<HTMLInputElement>('input[type="checkbox"]');
    if (!rtkCheckbox) throw new Error("missing RTK checkbox");
    rtkCheckbox.checked = true;
    rtkCheckbox.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();

    document.querySelector<HTMLButtonElement>('[data-ac-testid="settings.save"]')?.click();
    await settle();

    expect(SettingsAPI.saveDraft).toHaveBeenCalledWith(
      expect.objectContaining({ injectRtkHook: true }),
    );
    expect(SettingsAPI.update).not.toHaveBeenCalled();
    expect(SettingsAPI.updateCodingAgentProfiles).not.toHaveBeenCalled();
    expect(SettingsAPI.updateCodingAgentEnvSettings).not.toHaveBeenCalled();
    expect(SettingsAPI.sweepRtkHook).toHaveBeenCalledWith(true);

    resolveLoadedSettings(settings());
    dispose();
  });

  it("distinguishes MATCH (A), CONFIGURED (own non-A cell), FALLBACK, and MISSING", async () => {
    vi.mocked(SettingsAPI.get).mockResolvedValueOnce(settings({
      agents: [
        {
          id: "codex",
          label: "Codex",
          command: "codex",
          color: "#10b981",
          gitPullBefore: false,
          excludeGlobalClaudeMd: true,
          envs: [],
          isolatedHome: false,
        },
        {
          id: "claude",
          label: "Claude Code",
          command: "claude",
          color: "#d97706",
          gitPullBefore: false,
          excludeGlobalClaudeMd: false,
          envs: [],
          isolatedHome: false,
        },
      ],
      codingAgentProfiles: {
        schemaVersion: 2,
        profileSlots: {
          A: { label: "" },
          B: { label: "balanced" },
          C: { label: "review" },
          D: { label: "full-auto" },
        },
        defaultProfileByAgent: {},
        profilesByAgent: {
          codex: {
            A: { enabled: true, command: "codex --sandbox workspace-write", env: {}, notes: "" },
            // Own enabled non-A cell → CONFIGURED (direct match on a non-baseline slot).
            B: { enabled: true, command: "codex --model gpt-5-codex --effort medium", env: {}, notes: "" },
          },
          // D is configured on claude but not on codex → codex D is MISSING.
          claude: {
            A: { enabled: true, command: "claude", env: {}, notes: "" },
            D: { enabled: true, command: "claude --dangerously-skip-permissions", env: {}, notes: "" },
          },
        },
      },
    }));
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {}, section: "profiles" }),
      root,
    );
    await settle();

    // A baseline → MATCH (green).
    expect(byTestId("settings.profileCard.0.A").getAttribute("data-ac-state")).toBe("match");
    expect(byTestId("settings.profileCard.0.A.badge").textContent).toContain("MATCH");
    // B has its own cell on codex → CONFIGURED (cyan), NOT MATCH.
    expect(byTestId("settings.profileCard.0.B").getAttribute("data-ac-state")).toBe("configured");
    expect(byTestId("settings.profileCard.0.B.badge").textContent).toContain("CONFIGURED");
    // C has no cell anywhere → resolves through the configured B → FALLBACK.
    expect(byTestId("settings.profileCard.0.C").getAttribute("data-ac-state")).toBe("fallback");
    expect(byTestId("settings.profileCard.0.C.badge").textContent).toContain("FALLBACK");
    // D is configured on claude but absent on codex → MISSING.
    expect(byTestId("settings.profileCard.0.D").getAttribute("data-ac-state")).toBe("missing");
    expect(byTestId("settings.profileCard.0.D.badge").textContent).toContain("MISSING");

    dispose();
  });

  it("deletes a whole profile slot from every agent via Delete Profile", async () => {
    vi.mocked(SettingsAPI.get).mockResolvedValueOnce(settings({
      agents: [
        {
          id: "codex",
          label: "Codex",
          command: "codex",
          color: "#10b981",
          gitPullBefore: false,
          excludeGlobalClaudeMd: true,
          envs: [],
          isolatedHome: false,
        },
        {
          id: "claude",
          label: "Claude Code",
          command: "claude",
          color: "#d97706",
          gitPullBefore: false,
          excludeGlobalClaudeMd: false,
          envs: [],
          isolatedHome: false,
        },
      ],
      codingAgentProfiles: {
        schemaVersion: 2,
        profileSlots: { A: { label: "" }, B: { label: "fast" } },
        defaultProfileByAgent: {},
        profilesByAgent: {
          codex: {
            A: { enabled: true, command: "codex", env: {}, notes: "" },
            B: { enabled: true, command: "codex --profile fast", env: {}, notes: "" },
          },
          claude: {
            A: { enabled: true, command: "claude", env: {}, notes: "" },
            B: { enabled: true, command: "claude --model opus", env: {}, notes: "" },
          },
        },
      },
    }));
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {}, section: "profiles" }),
      root,
    );
    await settle();

    // The B card renders on both rails before deletion.
    expect(byTestId("settings.profileCard.0.B")).toBeTruthy();
    expect(byTestId("settings.profileCard.1.B")).toBeTruthy();

    // Expand codex's B card to reach the slot-level "Delete Profile" affordance.
    byTestId<HTMLButtonElement>("settings.profileCard.0.B.toggle").click();
    await settle();
    byTestId<HTMLButtonElement>("settings.profileCard.0.B.deleteProfile").click();
    await settle();

    // Slot B is gone from BOTH rails (it was a whole-slot delete, not per-agent).
    expect(document.querySelector('[data-ac-testid="settings.profileCard.0.B"]')).toBeNull();
    expect(document.querySelector('[data-ac-testid="settings.profileCard.1.B"]')).toBeNull();

    byTestId<HTMLButtonElement>("settings.save").click();
    await settle();

    const saved = vi.mocked(SettingsAPI.saveDraft).mock.calls[0]?.[0];
    expect(saved?.codingAgentProfiles.profileSlots.B).toBeUndefined();
    expect(saved?.codingAgentProfiles.profilesByAgent.codex?.B).toBeUndefined();
    expect(saved?.codingAgentProfiles.profilesByAgent.claude?.B).toBeUndefined();
    // The A baseline is untouched.
    expect(saved?.codingAgentProfiles.profileSlots.A).toBeTruthy();

    dispose();
  });

  it("keeps the left rail pinned when swapping with an empty right rail", async () => {
    vi.mocked(SettingsAPI.get).mockResolvedValueOnce(settings({
      agents: [
        {
          id: "codex",
          label: "Codex",
          command: "codex",
          color: "#10b981",
          gitPullBefore: false,
          excludeGlobalClaudeMd: true,
          envs: [],
          isolatedHome: false,
        },
        {
          id: "claude",
          label: "Claude Code",
          command: "claude",
          color: "#d97706",
          gitPullBefore: false,
          excludeGlobalClaudeMd: false,
          envs: [],
          isolatedHome: false,
        },
      ],
    }));
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {}, section: "agents" }),
      root,
    );
    await settle();

    // Pin the LEFT rail to claude (a non-first agent). The right rail was showing
    // claude, so it empties out — left=claude (pinned), right=empty.
    const leftSelect = byTestId<HTMLSelectElement>("settings.profileRail.0.agentSelect");
    leftSelect.value = "claude";
    leftSelect.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    expect(byTestId("settings.profileRail.0").getAttribute("data-ac-agent-id")).toBe("claude");
    expect(byTestId("settings.profileRail.1").getAttribute("data-ac-agent-id")).toBeNull();

    // Swap with an empty right rail must be a no-op — NOT drop the left pin to the
    // positional fallback (codex). The left stays claude; the right stays empty.
    byTestId<HTMLButtonElement>("settings.agents.swapRails").click();
    await settle();
    expect(byTestId("settings.profileRail.0").getAttribute("data-ac-agent-id")).toBe("claude");
    expect(byTestId("settings.profileRail.1").getAttribute("data-ac-agent-id")).toBeNull();

    dispose();
  });

  it("exposes the OpenCode quick-add preset and the instructions-file input (#529)", async () => {
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {}, section: "agents" }),
      root,
    );
    await settle();

    // Quick-add OpenCode button is present and enabled (seeded agent is codex).
    const opencodePreset = byTestId<HTMLButtonElement>("settings.agentPreset.opencode");
    expect(opencodePreset).toBeTruthy();
    expect(opencodePreset.disabled).toBe(false);
    expect(opencodePreset.getAttribute("data-ac-state")).toBe("available");

    // The per-agent instructions-file input lives in the collapsed editor (3-tab
    // rework) — expand the row first. Placeholder = command-derived default
    // (seeded command `codex` → AGENTS.md).
    expandAgentRow(0);
    await settle();
    const input = byTestId<HTMLInputElement>("settings.agentRow.0.instructionsFilename");
    expect(input).toBeTruthy();
    expect(input.value).toBe("");
    expect(input.placeholder).toBe("AGENTS.md");

    dispose();
  });

  it("disables the OpenCode preset when an opencode agent already exists (#529)", async () => {
    vi.mocked(SettingsAPI.get).mockResolvedValueOnce(settings({
      agents: [
        {
          id: "opencode",
          label: "OpenCode",
          command: "opencode",
          color: "#64748b",
          gitPullBefore: false,
          excludeGlobalClaudeMd: false,
          envs: [],
          isolatedHome: false,
          instructionsFilename: "AGENTS.md",
        },
      ],
    }));
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {}, section: "agents" }),
      root,
    );
    await settle();

    const opencodePreset = byTestId<HTMLButtonElement>("settings.agentPreset.opencode");
    expect(opencodePreset.disabled).toBe(true);
    expect(opencodePreset.getAttribute("data-ac-state")).toBe("disabled");

    dispose();
  });

  it("round-trips a typed instructions filename through save (#529)", async () => {
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {}, section: "agents" }),
      root,
    );
    await settle();
    expandAgentRow(0);
    await settle();

    const input = byTestId<HTMLInputElement>("settings.agentRow.0.instructionsFilename");
    input.value = "TEAM.md";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();

    byTestId<HTMLButtonElement>("settings.save").click();
    await settle();

    const saved = vi.mocked(SettingsAPI.saveDraft).mock.calls[0]?.[0];
    expect(saved?.agents[0]?.instructionsFilename).toBe("TEAM.md");

    dispose();
  });

  it("omits a cleared instructions filename from the saved draft (#529 G3)", async () => {
    vi.mocked(SettingsAPI.get).mockResolvedValueOnce(settings({
      agents: [
        {
          id: "claude",
          label: "Claude Code",
          command: "claude",
          color: "#d97706",
          gitPullBefore: false,
          excludeGlobalClaudeMd: false,
          envs: [],
          isolatedHome: false,
          instructionsFilename: "CLAUDE.md",
        },
      ],
    }));
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {}, section: "agents" }),
      root,
    );
    await settle();
    expandAgentRow(0);
    await settle();

    const input = byTestId<HTMLInputElement>("settings.agentRow.0.instructionsFilename");
    expect(input.value).toBe("CLAUDE.md");
    input.value = "";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();

    byTestId<HTMLButtonElement>("settings.save").click();
    await settle();

    const saved = vi.mocked(SettingsAPI.saveDraft).mock.calls[0]?.[0];
    expect(saved?.agents[0]).not.toHaveProperty("instructionsFilename");

    dispose();
  });
});
