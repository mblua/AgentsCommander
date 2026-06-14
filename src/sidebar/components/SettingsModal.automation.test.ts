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
        isolateCodexHome: false,
      },
    ],
    codingAgentProfiles: {
      schemaVersion: 1,
      letters: { A: { name: "" } },
      agentDefaults: {},
      matrix: {},
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

async function settle(): Promise<void> {
  await Promise.resolve();
  await new Promise<void>((resolve) => setTimeout(resolve, 0));
}

function byTestId<T extends Element = Element>(testId: string): T {
  const element = document.querySelector<T>(`[data-ac-testid="${testId}"]`);
  if (!element) throw new Error(`missing selector ${testId}`);
  return element;
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

  it("exposes profile letter and matrix controls for automation", async () => {
    vi.mocked(SettingsAPI.get).mockResolvedValueOnce(settings({
      codingAgentProfiles: {
        schemaVersion: 1,
        letters: {
          A: { name: "" },
          B: { name: "fast" },
        },
        agentDefaults: {},
        matrix: {
          codex: {
            A: { enabled: true, argv: ["--baseline"], env: {}, notes: "" },
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

    expect(byTestId("settings.profiles.letters")).toBeTruthy();
    expect(byTestId("settings.profileLetter.A")).toBeTruthy();
    expect(byTestId<HTMLInputElement>("settings.profileLetter.A.name")).toBeTruthy();
    expect(byTestId<HTMLButtonElement>("settings.profileLetter.A.delete").disabled).toBe(true);
    expect(byTestId("settings.profileLetter.B")).toBeTruthy();
    expect(byTestId<HTMLInputElement>("settings.profileLetter.B.name")).toBeTruthy();
    expect(byTestId<HTMLButtonElement>("settings.profileLetter.B.delete").disabled).toBe(false);

    expect(byTestId("settings.profiles.matrix")).toBeTruthy();
    expect(byTestId("settings.profileMatrix.agent.0").getAttribute("data-ac-agent-id")).toBe("codex");
    expect(byTestId("settings.profileMatrix.row.A")).toBeTruthy();
    expect(byTestId("settings.profileCell.0.A").getAttribute("data-ac-state")).toBe("editable");
    expect(byTestId<HTMLInputElement>("settings.profileCell.0.A.argv").value).toBe("--baseline");
    expect(byTestId<HTMLButtonElement>("settings.profileCell.0.A.delete").disabled).toBe(true);

    expect(byTestId("settings.profileMatrix.row.B")).toBeTruthy();
    expect(byTestId("settings.profileCell.0.B").getAttribute("data-ac-state")).toBe("missing");
    expect(byTestId("settings.profileCell.0.B.missing")).toBeTruthy();
    expect(byTestId<HTMLButtonElement>("settings.profileCell.0.B.add")).toBeTruthy();
    expect(byTestId<HTMLButtonElement>("settings.profiles.add").disabled).toBe(false);

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

    const argvInput = document.querySelector<HTMLInputElement>('[data-ac-testid="settings.profileCell.0.A.argv"]');
    if (!argvInput) throw new Error("missing profile argv input");
    argvInput.value = "--fast";
    argvInput.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();

    resolveLoadedSettings(settings());
    await settle();

    document.querySelector<HTMLButtonElement>('[data-ac-testid="settings.save"]')?.click();
    await settle();

    const saved = vi.mocked(SettingsAPI.saveDraft).mock.calls[0]?.[0];
    expect(saved?.codingAgentProfiles.matrix.codex?.A?.argv).toEqual(["--fast"]);

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
});
