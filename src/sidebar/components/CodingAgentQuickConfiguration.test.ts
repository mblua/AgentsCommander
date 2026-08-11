// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { render } from "solid-js/web";
import CodingAgentQuickConfiguration from "./CodingAgentQuickConfiguration";
import type { AppSettings } from "../../shared/types";
import { SettingsAPI } from "../../shared/ipc";
import { settingsStore } from "../../shared/stores/settings";

vi.mock("../../shared/ipc", () => ({
  SettingsAPI: {
    get: vi.fn(() => Promise.resolve(settings())),
    update: vi.fn(() => Promise.resolve()),
  },
  // #769 — the cards are driven by codingAgentsStore, which fetches this.
  // Resolve a catalog carrying Codex (the preset this suite selects).
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

function cancelButton(): HTMLButtonElement | null {
  return document.querySelector<HTMLButtonElement>('[data-ac-testid="onboarding.cancel"]');
}

function modalState(): string | null | undefined {
  return document
    .querySelector('[data-ac-testid="onboarding.modal"]')
    ?.getAttribute("data-ac-state");
}

async function selectCodexAndConfirm(): Promise<void> {
  document.querySelector<HTMLButtonElement>(
    '[data-ac-testid="onboarding.agentPreset.codex"]',
  )?.click();
  await settle();
  document.querySelector<HTMLButtonElement>('[data-ac-testid="onboarding.confirm"]')?.click();
  await settle();
}

describe("CodingAgentQuickConfiguration", () => {
  afterEach(() => {
    document.body.innerHTML = "";
    vi.clearAllMocks();
  });

  it("renders the consumer-provided title and message", async () => {
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () =>
        CodingAgentQuickConfiguration({
          title: "Add a Coding Agent",
          message: "Pick a Coding Agent to configure.",
          onClose: vi.fn(),
        }),
      root,
    );
    await settle();

    expect(document.querySelector(".agent-modal-title")?.textContent).toBe("Add a Coding Agent");
    expect(document.querySelector(".onboarding-welcome")?.textContent).toBe(
      "Pick a Coding Agent to configure.",
    );
    // Accessible name falls back to the title when no override is supplied.
    expect(
      document.querySelector('[data-ac-testid="onboarding.modal"]')?.getAttribute("aria-label"),
    ).toBe("Add a Coding Agent");

    dispose();
  });

  it("renders no Cancel button when no cancel callback is supplied", async () => {
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () =>
        CodingAgentQuickConfiguration({
          title: "Add a Coding Agent",
          message: "Pick a Coding Agent to configure.",
          onClose: vi.fn(),
        }),
      root,
    );
    await settle();

    expect(cancelButton()).toBeNull();
    // The confirm affordance still exists, so the modal is not a dead end.
    expect(document.querySelector('[data-ac-testid="onboarding.confirm"]')).toBeTruthy();

    dispose();
  });

  it("renders Cancel and invokes the callback when supplied", async () => {
    const onCancel = vi.fn();
    const onClose = vi.fn();
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () =>
        CodingAgentQuickConfiguration({
          title: "Add a Coding Agent",
          message: "Pick a Coding Agent to configure.",
          onCancel,
          onClose,
        }),
      root,
    );
    await settle();

    const cancel = cancelButton();
    expect(cancel?.textContent).toBe("Cancel");

    cancel?.click();
    await settle();

    // The callback owns closing; the component must not close on its own.
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(onClose).not.toHaveBeenCalled();
    expect(SettingsAPI.update).not.toHaveBeenCalled();

    dispose();
  });

  it("routes Escape to the cancel callback when one is supplied", async () => {
    const onCancel = vi.fn();
    const onClose = vi.fn();
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () =>
        CodingAgentQuickConfiguration({
          title: "Add a Coding Agent",
          message: "Pick a Coding Agent to configure.",
          onCancel,
          onClose,
        }),
      root,
    );
    await settle();

    pressEscape();
    await settle();

    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(onClose).not.toHaveBeenCalled();

    dispose();
  });

  it("leaves Escape inert when no cancel callback is supplied", async () => {
    const onClose = vi.fn();
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () =>
        CodingAgentQuickConfiguration({
          title: "Add a Coding Agent",
          message: "Pick a Coding Agent to configure.",
          onClose,
        }),
      root,
    );
    await settle();

    pressEscape();
    await settle();

    // No back-door dismissal: Escape must not close, and must not persist.
    expect(onClose).not.toHaveBeenCalled();
    expect(SettingsAPI.update).not.toHaveBeenCalled();
    expect(document.querySelector('[data-ac-testid="onboarding.modal"]')).toBeTruthy();

    dispose();
  });

  it("confirms an agent without any onboarding side effect", async () => {
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () =>
        CodingAgentQuickConfiguration({
          title: "Add a Coding Agent",
          message: "Pick a Coding Agent to configure.",
          onClose: vi.fn(),
        }),
      root,
    );
    await settle();

    document.querySelector<HTMLButtonElement>(
      '[data-ac-testid="onboarding.agentPreset.codex"]',
    )?.click();
    await settle();

    document.querySelector<HTMLButtonElement>('[data-ac-testid="onboarding.confirm"]')?.click();
    await settle();

    // #975 — the reusable component persists the agent and nothing else: it
    // must never flip onboardingDismissed on a consumer's behalf.
    expect(SettingsAPI.update).toHaveBeenCalledTimes(1);
    expect(SettingsAPI.update).toHaveBeenCalledWith(
      expect.objectContaining({
        onboardingDismissed: false,
        agents: [expect.objectContaining({ label: "Codex", command: "codex" })],
      }),
    );
    expect(document.querySelector('[data-ac-testid="onboarding.done"]')).toBeTruthy();

    dispose();
  });

  it("folds onBeforeSave changes into the single agent write", async () => {
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () =>
        CodingAgentQuickConfiguration({
          title: "Add a Coding Agent",
          message: "Pick a Coding Agent to configure.",
          onBeforeSave: (current) => ({ ...current, onboardingDismissed: true }),
          onClose: vi.fn(),
        }),
      root,
    );
    await settle();

    document.querySelector<HTMLButtonElement>(
      '[data-ac-testid="onboarding.agentPreset.codex"]',
    )?.click();
    await settle();

    document.querySelector<HTMLButtonElement>('[data-ac-testid="onboarding.confirm"]')?.click();
    await settle();

    expect(SettingsAPI.update).toHaveBeenCalledTimes(1);
    expect(SettingsAPI.update).toHaveBeenCalledWith(
      expect.objectContaining({
        onboardingDismissed: true,
        agents: [expect.objectContaining({ label: "Codex" })],
      }),
    );

    dispose();
  });

  it("closes on Escape in the success state without re-entering the cancel path (#975)", async () => {
    const onCancel = vi.fn();
    const onClose = vi.fn();
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () =>
        CodingAgentQuickConfiguration({
          title: "Add a Coding Agent",
          message: "Pick a Coding Agent to configure.",
          onCancel,
          onClose,
        }),
      root,
    );
    await settle();

    await selectCodexAndConfirm();
    expect(document.querySelector('[data-ac-testid="onboarding.done"]')).toBeTruthy();

    pressEscape();
    await settle();

    // #975 F1 — the success footer renders only "Get started", so Escape must
    // resolve to onClose. Routing it to onCancel would re-run the consumer's
    // cancel path and issue a second settings write after a completed save.
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(onCancel).not.toHaveBeenCalled();
    expect(SettingsAPI.get).toHaveBeenCalledTimes(1);
    expect(SettingsAPI.update).toHaveBeenCalledTimes(1);

    dispose();
  });

  it("advances data-ac-state selecting -> done and refreshes the settings store", async () => {
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () =>
        CodingAgentQuickConfiguration({
          title: "Add a Coding Agent",
          message: "Pick a Coding Agent to configure.",
          onClose: vi.fn(),
        }),
      root,
    );
    await settle();

    expect(modalState()).toBe("selecting");

    await selectCodexAndConfirm();

    // Automation state contract: consumers wait on done/selecting.
    expect(modalState()).toBe("done");
    // Without this refresh the sidebar keeps showing zero agents after setup.
    expect(settingsStore.refresh).toHaveBeenCalledTimes(1);

    dispose();
  });
});
