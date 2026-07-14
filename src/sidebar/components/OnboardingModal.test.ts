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
  // #769 — OnboardingModal now drives its cards from codingAgentsStore, which
  // fetches this. Resolve a catalog that includes Codex (the preset this suite
  // selects); the store's synchronous fallback also carries Codex regardless.
  CodingAgentsAPI: {
    getCatalog: vi.fn(() =>
      Promise.resolve([
        {
          key: "codex",
          label: "Codex",
          description: "Coding Agent by OpenAI",
          color: "#10b981",
          command: "codex",
          instructionsFilename: "AGENTS.md",
          envs: [],
          isolatedHome: false,
          removable: true,
        },
      ]),
    ),
    // #769 Phase 2 — the store also fetches this; onboarding does not use it.
    listReseedableCommands: vi.fn(() => Promise.resolve([])),
    reseedDefault: vi.fn(() => Promise.resolve({ dest: "", backupPath: "" })),
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
    webServerEnabled: false,
    webServerPort: 8765,
    webServerBind: "127.0.0.1",
    apiServerEnabled: false,
    apiServerPort: 8766,
    apiServerBind: "127.0.0.1",
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
    agents: [],
    codingAgentProfiles: {
      schemaVersion: 2,
      profileSlots: { A: { label: "" } },
      defaultProfileByAgent: {},
      profilesByAgent: {},
      profileLabelsByAgent: {},
    },
    telegramBots: [],
    onboardingDismissed: false,
    projectPaths: [],
    projectPath: null,
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
    coordinatorAutoCloseSkipTelegramAssigned: false,
    coordinatorCascadeCloseEnabled: true,
    npmUpdateNotificationsEnabled: true,
    autoSelfClearEnabled: true,
    autoSelfClearByAgent: {},
    containerCredentialsFromHost: true,
    logLevel: null,
    ...overrides,
    archivedProjectPaths: overrides.archivedProjectPaths ?? [],
  };
}

async function settle(): Promise<void> {
  await Promise.resolve();
  await new Promise<void>((resolve) => setTimeout(resolve, 0));
}

function pressEscape(): void {
  document
    .querySelector('[data-ac-testid="onboarding.overlay"]')
    ?.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
}

describe("OnboardingModal", () => {
  afterEach(() => {
    document.body.innerHTML = "";
    vi.clearAllMocks();
  });

  it("renders the Welcome title and intro message (#975)", async () => {
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(() => OnboardingModal({ onClose: vi.fn() }), root);
    await settle();

    expect(document.querySelector(".agent-modal-title")?.textContent).toBe("Welcome");
    expect(document.querySelector(".onboarding-welcome")?.textContent).toBe(
      "Welcome to AgentsCommander! Let's set up your first Coding Agent.",
    );
    // #975 — Welcome overrides the accessible name; it must not silently
    // degrade to the title.
    expect(
      document.querySelector('[data-ac-testid="onboarding.modal"]')?.getAttribute("aria-label"),
    ).toBe("Set up your first Coding Agent");

    dispose();
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

    // #975 — dismissal still rides along in the single agent-persisting write.
    expect(SettingsAPI.update).toHaveBeenCalledTimes(1);
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

  it("continues to persist dismissal when cancelled (#975)", async () => {
    const onClose = vi.fn();
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(() => OnboardingModal({ onClose }), root);
    await settle();

    const cancel = document.querySelector<HTMLButtonElement>('[data-ac-testid="onboarding.cancel"]');
    expect(cancel?.textContent).toBe("Cancel");
    cancel?.click();
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

  it("persists dismissal when Escape cancels the Welcome flow (#975)", async () => {
    const onClose = vi.fn();
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(() => OnboardingModal({ onClose }), root);
    await settle();

    pressEscape();
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

  it("closes on Escape in the success state with no second settings write (#975)", async () => {
    const onClose = vi.fn();
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(() => OnboardingModal({ onClose }), root);
    await settle();

    document.querySelector<HTMLButtonElement>(
      '[data-ac-testid="onboarding.agentPreset.codex"]',
    )?.click();
    await settle();

    document.querySelector<HTMLButtonElement>('[data-ac-testid="onboarding.confirm"]')?.click();
    await settle();
    expect(document.querySelector('[data-ac-testid="onboarding.done"]')).toBeTruthy();

    pressEscape();
    await settle();

    // #975 F1 — Escape on the "configured!" screen closes immediately. The
    // whole flow performs exactly one get + one update: no redundant settings
    // rewrite, session purge or global-hotkey re-registration, and no
    // read-modify-write clobber window after the agent was already saved.
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(SettingsAPI.get).toHaveBeenCalledTimes(1);
    expect(SettingsAPI.update).toHaveBeenCalledTimes(1);
    expect(SettingsAPI.update).toHaveBeenCalledWith(
      expect.objectContaining({
        onboardingDismissed: true,
        agents: [expect.objectContaining({ label: "Codex" })],
      }),
    );

    dispose();
  });
});
