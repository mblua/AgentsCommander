import { describe, expect, it } from "vitest";
import type { AppSettings } from "../../shared/types";
import { mergeSettingsForSavePreservingProjects } from "./settings-save";

function settings(overrides: Partial<AppSettings>): AppSettings {
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
    activityLogEnabled: false,
    ...overrides,
    archivedProjectPaths: overrides.archivedProjectPaths ?? [],
  };
}

describe("mergeSettingsForSavePreservingProjects", () => {
  it("preserves fresh project registration fields from disk", () => {
    const draft = settings({
      soundsEnabled: false,
      projectPaths: ["C:\\Stale"],
      projectPath: "C:\\Stale",
    });
    const fresh = settings({
      soundsEnabled: true,
      projectPaths: ["C:\\Fresh", "D:\\Other"],
      projectPath: "C:\\Fresh",
    });

    expect(mergeSettingsForSavePreservingProjects(draft, fresh)).toMatchObject({
      soundsEnabled: false,
      projectPaths: ["C:\\Fresh", "D:\\Other"],
      projectPath: "C:\\Fresh",
    });
  });

  it("keeps terminal snapshots under the dedicated setting owner", () => {
    const draft = settings({
      soundsEnabled: false,
      terminalSnapshotsEnabled: true,
    });
    const fresh = settings({
      soundsEnabled: true,
      terminalSnapshotsEnabled: false,
    });

    expect(mergeSettingsForSavePreservingProjects(draft, fresh)).toMatchObject({
      soundsEnabled: false,
      terminalSnapshotsEnabled: false,
    });
  });
});
