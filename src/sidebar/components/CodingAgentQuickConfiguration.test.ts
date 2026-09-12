// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "solid-js/web";
import CodingAgentQuickConfiguration from "./CodingAgentQuickConfiguration";
import type {
  AppSettings,
  CatalogReport,
  CodingAgentDefinition,
  SettingsSnapshot,
} from "../../shared/types";
import { SettingsAPI, CodingAgentsAPI } from "../../shared/ipc";
import { settingsStore } from "../../shared/stores/settings";
import { codingAgentsStore } from "../stores/coding-agents";
import { input as inputValue } from "../../shared/testing/ui-harness";

// #1965 — the cards are driven by codingAgentsStore, which reads the catalog
// report (never a bundled fallback). Resolve a report carrying Codex, the
// preset this suite selects; tests override it per case.
function defaultReport(): CatalogReport {
  return {
    primaryProjectRoot: null,
    sourcePath: null,
    catalog: [
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
        updateCommands: [],
        autoUpdate: false,
      },
    ],
    warnings: [],
    unavailable: null,
  };
}

vi.mock("../../shared/ipc", () => ({
  SettingsAPI: {
    get: vi.fn(() => Promise.resolve(settings())),
    update: vi.fn(() => Promise.resolve()),
  },
  CodingAgentsAPI: {
    getCatalogReport: vi.fn(() => Promise.resolve(defaultReport())),
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
    selectedRowRailWidth: "9px",
    selectedRowRailColor: "#00ff5f",
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
    restartResumeWakeWorkingAgents: false,
    restartResumeOrchestratorPrompt:
      "AgentsCommander was restarted. Continue with the work that was in flight.",
    restartResumeAgentPrompt: ".",
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
    remoteBlockingMenusEnabled: true,
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

function catalogDef(key: string, label: string, command: string): CodingAgentDefinition {
  return {
    key,
    label,
    description: `Coding Agent ${label}`,
    color: "#334155",
    command,
    envs: [],
    isolatedHome: false,
    removable: true,
    updateCommands: [],
    autoUpdate: false,
  };
}

function report(overrides: Partial<CatalogReport> = {}): CatalogReport {
  return {
    primaryProjectRoot: null,
    sourcePath: null,
    catalog: [],
    warnings: [],
    unavailable: null,
    ...overrides,
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

function byTestId<T extends HTMLElement = HTMLElement>(testId: string): T | null {
  return document.querySelector<T>(`[data-ac-testid="${testId}"]`);
}

function presetCards(): HTMLElement[] {
  return Array.from(
    document.querySelectorAll<HTMLElement>('[data-ac-testid^="onboarding.agentPreset."]'),
  );
}

function confirmButton(): HTMLButtonElement | null {
  return byTestId<HTMLButtonElement>("onboarding.confirm");
}

function modalState(): string | null | undefined {
  return document
    .querySelector('[data-ac-testid="onboarding.modal"]')
    ?.getAttribute("data-ac-state");
}

async function selectCodexAndConfirm(): Promise<void> {
  document
    .querySelector<HTMLButtonElement>('[data-ac-testid="onboarding.agentPreset.codex"]')
    ?.click();
  await settle();
  confirmButton()?.click();
  await settle();
}

function renderModal(): () => void {
  const root = document.createElement("div");
  document.body.append(root);
  return render(
    () =>
      CodingAgentQuickConfiguration({
        title: "Add a Coding Agent",
        message: "Pick a Coding Agent to configure.",
        onClose: vi.fn(),
      }),
    root,
  );
}

describe("CodingAgentQuickConfiguration", () => {
  beforeEach(() => {
    codingAgentsStore.resetForTests();
  });

  afterEach(() => {
    document.body.innerHTML = "";
    vi.clearAllMocks();
    vi.mocked(CodingAgentsAPI.getCatalogReport).mockImplementation(() =>
      Promise.resolve(defaultReport()),
    );
    vi.mocked(CodingAgentsAPI.listReseedableCommands).mockImplementation(() =>
      Promise.resolve([]),
    );
    vi.mocked(SettingsAPI.get).mockImplementation(() =>
      Promise.resolve(settings() as SettingsSnapshot),
    );
    vi.mocked(SettingsAPI.update).mockImplementation(() => Promise.resolve());
    codingAgentsStore.resetForTests();
  });

  it("renders the consumer-provided title and message", async () => {
    const dispose = renderModal();
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

  it("shows the loading state until the report settles", async () => {
    let resolveReport!: (value: CatalogReport) => void;
    const pendingReport = new Promise<CatalogReport>((resolve) => {
      resolveReport = resolve;
    });
    vi.mocked(CodingAgentsAPI.getCatalogReport).mockReturnValueOnce(pendingReport);

    const dispose = renderModal();
    await settle();

    expect(byTestId("onboarding.catalog.loading")?.textContent).toContain("Loading catalog");

    resolveReport(
      report({ primaryProjectRoot: null, catalog: [catalogDef("codex", "Codex", "codex")] }),
    );
    await vi.waitFor(() => expect(byTestId("onboarding.agentPreset.codex")).toBeTruthy());
    expect(byTestId("onboarding.catalog.loading")).toBeNull();

    dispose();
  });

  it("renders no Cancel button when no cancel callback is supplied", async () => {
    const dispose = renderModal();
    await settle();

    expect(cancelButton()).toBeNull();
    // The confirm affordance still exists, so the modal is not a dead end.
    expect(confirmButton()).toBeTruthy();

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
    expect(byTestId("onboarding.modal")).toBeTruthy();

    dispose();
  });

  it("confirms an agent without any onboarding side effect", async () => {
    const dispose = renderModal();
    await settle();

    await selectCodexAndConfirm();

    // #975 — the reusable component persists the agent and nothing else: it
    // must never flip onboardingDismissed on a consumer's behalf.
    expect(SettingsAPI.update).toHaveBeenCalledTimes(1);
    expect(SettingsAPI.update).toHaveBeenCalledWith(
      expect.objectContaining({
        onboardingDismissed: false,
        agents: [expect.objectContaining({ label: "Codex", command: "codex" })],
      }),
    );
    expect(byTestId("onboarding.done")).toBeTruthy();

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

    await selectCodexAndConfirm();

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
    expect(byTestId("onboarding.done")).toBeTruthy();

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
    const dispose = renderModal();
    await settle();

    expect(modalState()).toBe("selecting");

    await selectCodexAndConfirm();

    // Automation state contract: consumers wait on done/selecting.
    expect(modalState()).toBe("done");
    // Without this refresh the sidebar keeps showing zero agents after setup.
    expect(settingsStore.refresh).toHaveBeenCalledTimes(1);

    dispose();
  });

  it("shows the report path/reason for an unavailable catalog and offers no fallback cards", async () => {
    vi.mocked(CodingAgentsAPI.getCatalogReport).mockResolvedValue(
      report({
        primaryProjectRoot: "C:/repo/app",
        sourcePath: "C:/repo/app/.ac/coding-agents/agents.json",
        unavailable: {
          code: "baseUnavailable",
          path: "C:/repo/app/.ac/coding-agents/agents.json",
          reason: "catalog bytes are corrupt",
        },
      }),
    );
    const dispose = renderModal();
    await settle();

    expect(byTestId("onboarding.catalog.error")?.textContent).toContain("Catalog unavailable");
    expect(byTestId("onboarding.catalog.error.path")?.textContent).toContain(
      "C:/repo/app/.ac/coding-agents/agents.json",
    );
    expect(byTestId("onboarding.catalog.error.reason")?.textContent).toContain(
      "catalog bytes are corrupt",
    );
    // Only the manual Custom Agent card exists; no selectable embedded defaults.
    expect(presetCards().map((el) => el.getAttribute("data-ac-testid"))).toEqual([
      "onboarding.agentPreset.custom",
    ]);

    dispose();
  });

  it("shows a warning's path/reason while the readable base card stays selectable", async () => {
    vi.mocked(CodingAgentsAPI.getCatalogReport).mockResolvedValue(
      report({
        primaryProjectRoot: "C:/repo/app",
        sourcePath: "C:/repo/app/.ac/coding-agents/agents.json",
        catalog: [catalogDef("codex", "Codex", "codex")],
        warnings: [
          {
            code: "local-overlay-invalid",
            path: "C:/repo/app/.ac/coding-agents/agents.local.json",
            reason: "unknown field",
          },
        ],
      }),
    );
    const dispose = renderModal();
    await settle();

    expect(byTestId("onboarding.catalog.warning.0.path")?.textContent).toContain(
      "agents.local.json",
    );
    expect(byTestId("onboarding.catalog.warning.0.reason")?.textContent).toContain("unknown field");
    expect(byTestId("onboarding.agentPreset.codex")).toBeTruthy();

    // The readable base row is usable.
    await selectCodexAndConfirm();
    expect(SettingsAPI.update).toHaveBeenCalledWith(
      expect.objectContaining({
        agents: [expect.objectContaining({ label: "Codex", command: "codex" })],
      }),
    );
    expect(byTestId("onboarding.done")).toBeTruthy();

    dispose();
  });

  it("keeps Manual Custom Agent creation usable when the catalog is unavailable", async () => {
    vi.mocked(CodingAgentsAPI.getCatalogReport).mockRejectedValue("config-dir failure");
    const dispose = renderModal();
    await settle();

    expect(byTestId("onboarding.catalog.error.reason")?.textContent).toContain(
      "config-dir failure",
    );
    expect(presetCards().map((el) => el.getAttribute("data-ac-testid"))).toEqual([
      "onboarding.agentPreset.custom",
    ]);

    byTestId<HTMLButtonElement>("onboarding.agentPreset.custom")!.click();
    await settle();
    const label = byTestId<HTMLInputElement>("onboarding.custom.label")!;
    const command = byTestId<HTMLInputElement>("onboarding.custom.command")!;
    inputValue(label, "My Agent");
    inputValue(command, "my-agent --flag");
    await settle();

    confirmButton()!.click();
    await settle();

    expect(SettingsAPI.update).toHaveBeenCalledWith(
      expect.objectContaining({
        agents: [expect.objectContaining({ label: "My Agent", command: "my-agent --flag" })],
      }),
    );
    expect(byTestId("onboarding.done")).toBeTruthy();

    dispose();
  });

  it("recovers through Reload after a catalog failure (no fallback in between)", async () => {
    vi.mocked(CodingAgentsAPI.getCatalogReport).mockRejectedValue("config-dir failure");
    const dispose = renderModal();
    await settle();

    expect(byTestId("onboarding.catalog.error")).toBeTruthy();
    expect(presetCards().map((el) => el.getAttribute("data-ac-testid"))).toEqual([
      "onboarding.agentPreset.custom",
    ]);

    vi.mocked(CodingAgentsAPI.getCatalogReport).mockResolvedValue(
      report({ primaryProjectRoot: null, catalog: [catalogDef("codex", "Codex", "codex")] }),
    );
    byTestId<HTMLButtonElement>("onboarding.catalog.reload")!.click();

    await vi.waitFor(() => expect(byTestId("onboarding.agentPreset.codex")).toBeTruthy());
    expect(byTestId("onboarding.catalog.error")).toBeNull();
    expect(confirmButton()!.disabled).toBe(true); // nothing selected yet

    dispose();
  });

  it("disables a stale preset confirmation before the settings fetch", async () => {
    const dispose = renderModal();
    await settle();

    byTestId<HTMLButtonElement>("onboarding.agentPreset.codex")!.click();
    await settle();
    expect(confirmButton()!.disabled).toBe(false);

    // A reload invalidates the selection made in the previous generation.
    vi.mocked(CodingAgentsAPI.getCatalogReport).mockResolvedValue(
      report({ primaryProjectRoot: null, catalog: [catalogDef("codex", "Codex", "codex")] }),
    );
    await codingAgentsStore.refresh();
    await settle();

    expect(confirmButton()!.disabled).toBe(true);
    expect(byTestId("onboarding.agentPreset.codex")!.getAttribute("data-ac-state")).toBe("idle");
    // Manual custom fields would survive; the catalog selection does not.

    dispose();
  });

  it("aborts a pending confirmation when the generation changes during the settings fetch", async () => {
    let resolveSettings!: (value: SettingsSnapshot) => void;
    const pendingSettings = new Promise<SettingsSnapshot>((resolve) => {
      resolveSettings = resolve;
    });
    vi.mocked(SettingsAPI.get).mockReturnValueOnce(pendingSettings);

    const dispose = renderModal();
    await settle();

    byTestId<HTMLButtonElement>("onboarding.agentPreset.codex")!.click();
    await settle();
    confirmButton()!.click();
    await settle();
    expect(confirmButton()!.getAttribute("data-ac-state")).toBe("saving");

    // The project switched while the settings read was still pending.
    vi.mocked(CodingAgentsAPI.getCatalogReport).mockResolvedValue(
      report({ primaryProjectRoot: null, catalog: [catalogDef("codex", "Codex", "codex")] }),
    );
    await codingAgentsStore.refresh();
    await settle();

    resolveSettings(settings() as SettingsSnapshot);
    await settle();
    await settle();

    // The preset from the old generation must not register.
    expect(SettingsAPI.update).not.toHaveBeenCalled();
    expect(byTestId("onboarding.done")).toBeNull();
    expect(confirmButton()!.disabled).toBe(true);

    dispose();
  });
});
