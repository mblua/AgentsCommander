import { describe, expect, it } from "vitest";
import type { AgentConfig, AppSettings } from "../../shared/types";
import { mergeSettingsForSavePreservingProjects } from "./settings-save";

function agent(overrides: Partial<AgentConfig> = {}): AgentConfig {
  return {
    id: "a1",
    label: "Agent",
    command: "claude",
    color: "#000000",
    gitPullBefore: false,
    excludeGlobalClaudeMd: false,
    envs: [],
    isolatedHome: false,
    ...overrides,
  };
}

function settings(overrides: Partial<AppSettings> = {}): AppSettings {
  return {
    defaultShell: "pwsh",
    defaultShellArgs: [],
    agents: [],
    codingAgentProfiles: {
      schemaVersion: 2,
      profileSlots: { A: { label: "" } },
      defaultProfileByAgent: {},
      profilesByAgent: {},
    },
    telegramBots: [],
    telegramNetworkPollErrorLogging: {
      firstFailureLevel: "warn",
      transientRepeatLevel: "debug",
      sustainedLevel: "warn",
      sustainedAfterSeconds: 60,
      sustainedRepeatSeconds: 300,
      recoveryLevel: "info",
    },
    restoreCoordinatorWakeState: true,
    sidebarAlwaysOnTop: false,
    raiseTerminalOnClick: true,
    soundsEnabled: true,
    teamIdleBeepEnabled: true,
    voiceToTextEnabled: false,
    geminiApiKey: "",
    geminiModel: "gemini-2.5-flash",
    voiceAutoExecute: false,
    voiceAutoExecuteDelay: 15,
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
    webServerEnabled: false,
    webServerPort: 8765,
    webServerBind: "127.0.0.1",
    projectPath: null,
    projectPaths: [],
    sidebarStyle: "noir-minimal",
    onboardingDismissed: true,
    coordSortByActivity: false,
    injectRtkHook: false,
    rtkPromptDismissed: false,
    informWhenRtkInstalled: true,
    autoGenerateTaskTitle: true,
    agentTemplatesPath: null,
    themeLight: true,
    specBoardEnabled: false,
    ...overrides,
  };
}

describe("mergeSettingsForSavePreservingProjects (#529 G3 normalization)", () => {
  it("drops an empty or whitespace-only instructionsFilename so the sentinel never persists", () => {
    const draft = settings({
      agents: [
        agent({ id: "empty", instructionsFilename: "" }),
        agent({ id: "spaces", instructionsFilename: "   " }),
        agent({ id: "absent", instructionsFilename: undefined }),
      ],
    });
    const merged = mergeSettingsForSavePreservingProjects(draft, settings());

    for (const a of merged.agents) {
      expect("instructionsFilename" in a).toBe(false);
    }
  });

  it("trims and keeps a non-empty instructionsFilename", () => {
    const draft = settings({
      agents: [agent({ id: "set", instructionsFilename: "  TEAM.md  " })],
    });
    const merged = mergeSettingsForSavePreservingProjects(draft, settings());

    expect(merged.agents[0]?.instructionsFilename).toBe("TEAM.md");
  });

  it("preserves a clean instructionsFilename and unrelated agent fields untouched", () => {
    const draft = settings({
      agents: [agent({ id: "claude", command: "claude", instructionsFilename: "CLAUDE.md" })],
    });
    const merged = mergeSettingsForSavePreservingProjects(draft, settings());

    expect(merged.agents[0]).toEqual(
      agent({ id: "claude", command: "claude", instructionsFilename: "CLAUDE.md" }),
    );
  });

  it("still adopts projectPaths/projectPath from the fresh settings", () => {
    const draft = settings({ projectPath: "C:/stale", projectPaths: ["C:/stale"] });
    const fresh = settings({ projectPath: "C:/fresh", projectPaths: ["C:/fresh", "C:/other"] });
    const merged = mergeSettingsForSavePreservingProjects(draft, fresh);

    expect(merged.projectPath).toBe("C:/fresh");
    expect(merged.projectPaths).toEqual(["C:/fresh", "C:/other"]);
  });
});
