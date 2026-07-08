import { describe, expect, it } from "vitest";
import type { AgentConfig, AppSettings } from "../../shared/types";
import { mergeSettingsForSavePreservingProjects } from "./settings-save";

function agent(overrides: Partial<AgentConfig> = {}): AgentConfig {
  return {
    id: "a1",
    label: "Agent",
    command: "claude",
    color: "#000000",
    envs: [],
    isolatedHome: false,
    ...overrides,
  };
}

// Mirrors the fix/526 AppSettings shape (e.g. requires alwaysShowSelectedWorkgroup),
// not the 4-tab branch's shape — kept in sync with the SettingsModal automation test.
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
    coordinatorIdleBadgeYellowMinutes: 30,
    coordinatorIdleBadgeRedMinutes: 60,
    coordinatorAutoCloseEnabled: true,
    coordinatorAutoCloseMinutes: 60,
    coordinatorAutoCloseSkipTelegramAssigned: false,
    coordinatorCascadeCloseEnabled: true,
    npmUpdateNotificationsEnabled: true,
    autoSelfClearEnabled: true,
    autoSelfClearByAgent: {},
    logLevel: null,
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

describe("mergeSettingsForSavePreservingProjects (#598 configSeed normalization)", () => {
  it("drops an inactive configSeed so it never persists as a sentinel", () => {
    const draft = settings({
      agents: [
        agent({ id: "absent", configSeed: undefined }),
        agent({ id: "disabled", configSeed: { enabled: false, dest: ".claude" } }),
        agent({ id: "emptyDest", configSeed: { enabled: true, dest: "" } }),
        agent({ id: "spacesDest", configSeed: { enabled: true, dest: "   " } }),
      ],
    });
    const merged = mergeSettingsForSavePreservingProjects(draft, settings());

    for (const a of merged.agents) {
      expect("configSeed" in a).toBe(false);
    }
  });

  it("keeps an active configSeed, trims dest, and forces enabled true", () => {
    const draft = settings({
      agents: [agent({ id: "set", configSeed: { enabled: true, dest: "  .claude  " } })],
    });
    const merged = mergeSettingsForSavePreservingProjects(draft, settings());

    expect(merged.agents[0]?.configSeed).toEqual({ enabled: true, dest: ".claude" });
  });

  it("normalizes configSeed and instructionsFilename together without cross-talk", () => {
    const draft = settings({
      agents: [
        agent({
          id: "both",
          instructionsFilename: "  CLAUDE.md  ",
          configSeed: { enabled: true, dest: ".claude" },
        }),
      ],
    });
    const merged = mergeSettingsForSavePreservingProjects(draft, settings());

    expect(merged.agents[0]?.instructionsFilename).toBe("CLAUDE.md");
    expect(merged.agents[0]?.configSeed).toEqual({ enabled: true, dest: ".claude" });
  });
});

describe("mergeSettingsForSavePreservingProjects (#868 backend normalization)", () => {
  it("drops absent, empty, and explicit Local backend objects", () => {
    const draft = settings({
      agents: [
        agent({ id: "absent" }),
        agent({ id: "empty", backend: {} }),
        agent({ id: "local", backend: { kind: "localProcess" } }),
      ],
    });
    const merged = mergeSettingsForSavePreservingProjects(draft, settings());

    for (const a of merged.agents) {
      expect("backend" in a).toBe(false);
    }
  });

  it("keeps a container backend kind and trims the image", () => {
    const draft = settings({
      agents: [
        agent({
          backend: {
            kind: "containerTransport",
            image: "  agentscommander/ac-claude:latest  ",
          },
        }),
      ],
    });
    const merged = mergeSettingsForSavePreservingProjects(draft, settings());

    expect(merged.agents[0]?.backend).toEqual({
      kind: "containerTransport",
      image: "agentscommander/ac-claude:latest",
    });
  });

  it("omits empty container images while preserving the container kind", () => {
    const draft = settings({
      agents: [
        agent({ id: "empty", backend: { kind: "containerTransport", image: "" } }),
        agent({ id: "spaces", backend: { kind: "containerTransport", image: "   " } }),
      ],
    });
    const merged = mergeSettingsForSavePreservingProjects(draft, settings());

    expect(merged.agents[0]?.backend).toEqual({ kind: "containerTransport" });
    expect(merged.agents[1]?.backend).toEqual({ kind: "containerTransport" });
  });

  it("drops a Local backend even when the live draft has a hidden image", () => {
    const draft = settings({
      agents: [
        agent({
          backend: {
            kind: "localProcess",
            image: "agentscommander/ac-claude:latest",
          },
        }),
      ],
    });
    const merged = mergeSettingsForSavePreservingProjects(draft, settings());

    expect(merged.agents[0]).not.toHaveProperty("backend");
  });
});

describe("mergeSettingsForSavePreservingProjects webserver seed rebasing", () => {
  it("rebases unchanged stale modal webserver fields from fresh settings", () => {
    const modalSeed = settings({
      webServerEnabled: false,
      webServerPort: 8765,
      webServerBind: "127.0.0.1",
      npmUpdateNotificationsEnabled: true,
    });
    const draft = settings({
      webServerEnabled: false,
      webServerPort: 8765,
      webServerBind: "127.0.0.1",
      npmUpdateNotificationsEnabled: false,
    });
    const fresh = settings({
      webServerEnabled: true,
      webServerPort: 9000,
      webServerBind: "0.0.0.0",
      npmUpdateNotificationsEnabled: true,
    });

    const merged = mergeSettingsForSavePreservingProjects(draft, fresh, modalSeed);

    expect(merged.webServerEnabled).toBe(true);
    expect(merged.webServerPort).toBe(9000);
    expect(merged.webServerBind).toBe("0.0.0.0");
    expect(merged.npmUpdateNotificationsEnabled).toBe(false);
  });

  it("keeps an intentional modal edit to one webserver field while rebasing unchanged fields", () => {
    const modalSeed = settings({
      webServerEnabled: false,
      webServerPort: 8765,
      webServerBind: "127.0.0.1",
    });
    const draft = settings({
      webServerEnabled: true,
      webServerPort: 8765,
      webServerBind: "127.0.0.1",
    });
    const fresh = settings({
      webServerEnabled: false,
      webServerPort: 9000,
      webServerBind: "0.0.0.0",
    });

    const merged = mergeSettingsForSavePreservingProjects(draft, fresh, modalSeed);

    expect(merged.webServerEnabled).toBe(true);
    expect(merged.webServerPort).toBe(9000);
    expect(merged.webServerBind).toBe("0.0.0.0");
  });

  it("keeps two-argument behavior for backwards-compatible callers", () => {
    const draft = settings({
      webServerEnabled: false,
      webServerPort: 8765,
      webServerBind: "127.0.0.1",
    });
    const fresh = settings({
      webServerEnabled: true,
      webServerPort: 9000,
      webServerBind: "0.0.0.0",
    });

    const merged = mergeSettingsForSavePreservingProjects(draft, fresh);

    expect(merged.webServerEnabled).toBe(false);
    expect(merged.webServerPort).toBe(8765);
    expect(merged.webServerBind).toBe("127.0.0.1");
  });
});
