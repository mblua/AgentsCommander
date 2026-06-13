// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { render } from "solid-js/web";
import SettingsModal from "./SettingsModal";
import type { AppSettings } from "../../shared/types";

vi.mock("../../shared/ipc", () => ({
  SettingsAPI: {
    get: vi.fn(() => Promise.resolve(settings())),
    update: vi.fn(() => Promise.resolve()),
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

function settings(): AppSettings {
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
      },
    ],
    telegramBots: [],
    onboardingDismissed: true,
    projectPaths: [],
    projectPath: null,
    rtkPromptDismissed: false,
    autoGenerateTaskTitle: true,
    agentTemplatesPath: null,
    specBoardEnabled: false,
  };
}

async function settle(): Promise<void> {
  await Promise.resolve();
  await new Promise<void>((resolve) => setTimeout(resolve, 0));
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
});
