// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { render } from "solid-js/web";
import OnboardingModal from "./OnboardingModal";
import type { AppSettings } from "../../shared/types";
import { SettingsAPI } from "../../shared/ipc";

vi.mock("../../shared/ipc", () => ({
  SettingsAPI: {
    get: vi.fn(() => Promise.resolve(settings())),
    update: vi.fn(() => Promise.resolve()),
  },
}));

vi.mock("../../shared/stores/settings", () => ({
  settingsStore: {
    refresh: vi.fn(),
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
    agents: [],
    codingAgentProfiles: {
      schemaVersion: 2,
      profileSlots: { A: { label: "" } },
      defaultProfileByAgent: {},
      profilesByAgent: {},
    },
    telegramBots: [],
    onboardingDismissed: false,
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

describe("OnboardingModal", () => {
  afterEach(() => {
    document.body.innerHTML = "";
    vi.clearAllMocks();
  });

  it("persists dismissal when preset setup completes", async () => {
    const onClose = vi.fn();
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(() => OnboardingModal({ onClose }), root);
    await settle();

    document.querySelector<HTMLButtonElement>(
      '[data-ac-testid="onboarding.agentPreset.codex"]',
    )?.click();
    await settle();

    document.querySelector<HTMLButtonElement>(
      '[data-ac-testid="onboarding.confirm"]',
    )?.click();
    await settle();

    expect(SettingsAPI.update).toHaveBeenCalledWith(
      expect.objectContaining({
        onboardingDismissed: true,
        agents: [
          expect.objectContaining({
            label: "Codex",
            command: "codex",
          }),
        ],
      }),
    );
    expect(document.querySelector('[data-ac-testid="onboarding.done.close"]')).toBeTruthy();

    document.querySelector<HTMLButtonElement>(
      '[data-ac-testid="onboarding.done.close"]',
    )?.click();
    expect(onClose).toHaveBeenCalledTimes(1);

    dispose();
  });

  it("continues to persist dismissal when skipped", async () => {
    const onClose = vi.fn();
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(() => OnboardingModal({ onClose }), root);
    await settle();

    document.querySelector<HTMLButtonElement>('[data-ac-testid="onboarding.skip"]')?.click();
    await settle();

    expect(SettingsAPI.update).toHaveBeenCalledWith(
      expect.objectContaining({
        onboardingDismissed: true,
        agents: [],
      }),
    );
    expect(onClose).toHaveBeenCalledTimes(1);

    dispose();
  });
});
