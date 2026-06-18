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
      profileLabelsByAgent: {},
    },
    telegramBots: [],
    onboardingDismissed: false,
    projectPaths: [],
    projectPath: null,
    rtkPromptDismissed: false,
    autoGenerateTaskTitle: true,
    agentTemplatesPath: null,
    specBoardEnabled: false,
    ...overrides,
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
});
