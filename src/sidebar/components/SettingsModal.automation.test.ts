// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { render } from "solid-js/web";
import SettingsModal from "./SettingsModal";
import type { AppSettings, SettingsSnapshot } from "../../shared/types";
import { SettingsAPI } from "../../shared/ipc";
import {
  AC_MATRIX_ROOT_PLACEHOLDER,
  AC_REPLICA_ROOT_PLACEHOLDER,
  AC_WORKSPACE_ROOT_PLACEHOLDER,
  PI_CONTEXT_REGEX,
} from "../../shared/profile-utils";

vi.mock("../../shared/ipc", async () => {
  // #769 — SettingsModal mounts codingAgentsStore.ensureLoaded(), so the ipc mock
  // must expose CodingAgentsAPI or the store hits an undefined export. Resolve the
  // catalog with the real built-in list so the preset quick-add buttons these
  // automation tests address (codex/opencode) still render; no reseed set.
  const { FALLBACK_CODING_AGENTS } = await vi.importActual<
    typeof import("../../shared/agent-presets")
  >("../../shared/agent-presets");
  return {
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
      startApiServer: vi.fn(() => Promise.resolve(false)),
      stopApiServer: vi.fn(() => Promise.resolve(false)),
      apiServerStatus: vi.fn(() => Promise.resolve(false)),
    },
    TelegramAPI: {
      sendTest: vi.fn(() => Promise.resolve(0)),
    },
    ReposAPI: {
      search: vi.fn(() => Promise.resolve([])),
    },
    CodingAgentsAPI: {
      getCatalog: vi.fn(() => Promise.resolve(FALLBACK_CODING_AGENTS)),
      listReseedableCommands: vi.fn(() => Promise.resolve([])),
      reseedDefault: vi.fn(() => Promise.resolve({ dest: "", backupPath: "" })),
    },
  };
});

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

function settings(overrides: Partial<AppSettings> = {}): SettingsSnapshot {
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
    agents: [
      {
        id: "codex",
        label: "Codex",
        command: "codex",
        color: "#10b981",
        envs: [],
        isolatedHome: false,
      },
    ],
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
    // #1077: get_settings returns a SettingsSnapshot; SettingsModal ignores the
    // report, so a pristine empty resolution keeps these mocks well-typed.
    projectPathResolution: {
      activeRegistrationCount: 0,
      archivedRegistrationCount: 0,
      issues: [],
      reconciliationError: null,
    },
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

// #526: the unified Coding Agents screen keeps each agent's config editor
// collapsed by default (the resting screen reads as the prototype). #895: the
// row head now assigns the rail, so the chevron button owns the expand of the
// inline editor that holds label/command/env/isolation.
function expandAgentRow(index = 0): void {
  const toggle = document.querySelector<HTMLElement>(
    `[data-ac-testid="settings.agentRow.${index}.toggle"]`,
  );
  if (!toggle) throw new Error(`missing agent row toggle ${index}`);
  toggle.click();
}

// #1095 — the coding-agent screen now opens with one rail; reveal the second
// via the top-right dropdown for tests that inspect the comparison rail.
async function enterTwoRails(): Promise<void> {
  const sel = byTestId<HTMLSelectElement>("settings.profiles.railCount");
  sel.value = "2";
  sel.dispatchEvent(new Event("change", { bubbles: true }));
  await settle();
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
    expandAgentRow(0);
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
    expandAgentRow(0);
    await settle();

    expect(agentsTab?.getAttribute("data-ac-state")).toBe("active");
    expect(document.querySelector('[data-ac-testid="settings.agentRow.0"]')).toBeTruthy();
    expect(document.querySelector('[data-ac-testid="settings.agentRow.0.label"]')).toBeTruthy();
    expect(document.querySelector('[data-ac-testid="settings.agentPreset.codex"]')).toBeTruthy();
    expect(document.querySelector('[data-ac-testid="settings.agent.addCustom"]')).toBeTruthy();

    dispose();
  });

  it("offers and saves the opt-in Pi context pattern without pre-seeding it", async () => {
    vi.mocked(SettingsAPI.get).mockResolvedValueOnce(settings({
      agents: [
        {
          id: "pi",
          label: "Pi",
          command: "pi",
          color: "#8b5cf6",
          envs: [],
          isolatedHome: false,
        },
      ],
    }));

    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {}, section: "agents" }),
      root,
    );
    await settle();
    expandAgentRow(0);
    await settle();

    const contextRegex = byTestId<HTMLInputElement>(
      "settings.agentRow.0.contextRegex",
    );
    expect(contextRegex.value).toBe("");
    expect(contextRegex.placeholder).toBe(PI_CONTEXT_REGEX);

    const suggest = byTestId<HTMLButtonElement>(
      "settings.agentRow.0.contextRegex.suggest",
    );
    expect(suggest.textContent?.trim()).toBe("Use suggested pattern");

    suggest.click();
    await settle();
    expect(contextRegex.value).toBe(PI_CONTEXT_REGEX);

    byTestId<HTMLButtonElement>("settings.save").click();
    await settle();

    const saved = vi.mocked(SettingsAPI.saveDraft).mock.calls[0]?.[0];
    expect(saved?.agents[0]?.contextRegex).toBe(PI_CONTEXT_REGEX);

    dispose();
  });

  it("starts the API server, refreshes live status, and persists the enabled preference", async () => {
    vi.mocked(SettingsAPI.apiServerStatus)
      .mockResolvedValueOnce(false)
      .mockResolvedValueOnce(true);
    vi.mocked(SettingsAPI.startApiServer).mockResolvedValueOnce(true);

    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {} }),
      root,
    );
    await settle();

    const toggle = byTestId<HTMLInputElement>("settings.general.apiServerEnabled");
    expect(toggle.checked).toBe(false);

    toggle.checked = true;
    toggle.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    await settle();

    expect(SettingsAPI.startApiServer).toHaveBeenCalledTimes(1);
    expect(byTestId<HTMLInputElement>("settings.general.apiServerEnabled").checked).toBe(true);
    expect(byTestId("settings.general.apiServerStatus").getAttribute("data-ac-state")).toBe("running");

    byTestId<HTMLButtonElement>("settings.save").click();
    await settle();

    const saved = vi.mocked(SettingsAPI.saveDraft).mock.calls[0]?.[0];
    expect(saved?.apiServerEnabled).toBe(true);

    dispose();
  });

  it("stops the API server, refreshes live status, and persists the disabled preference", async () => {
    vi.mocked(SettingsAPI.get).mockResolvedValueOnce(settings({ apiServerEnabled: true }));
    vi.mocked(SettingsAPI.apiServerStatus)
      .mockResolvedValueOnce(true)
      .mockResolvedValueOnce(false);
    vi.mocked(SettingsAPI.stopApiServer).mockResolvedValueOnce(true);

    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {} }),
      root,
    );
    await settle();

    const toggle = byTestId<HTMLInputElement>("settings.general.apiServerEnabled");
    expect(toggle.checked).toBe(true);

    toggle.checked = false;
    toggle.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    await settle();

    expect(SettingsAPI.stopApiServer).toHaveBeenCalledTimes(1);
    expect(byTestId<HTMLInputElement>("settings.general.apiServerEnabled").checked).toBe(false);
    expect(byTestId("settings.general.apiServerStatus").getAttribute("data-ac-state")).toBe("stopped");

    byTestId<HTMLButtonElement>("settings.save").click();
    await settle();

    const saved = vi.mocked(SettingsAPI.saveDraft).mock.calls[0]?.[0];
    expect(saved?.apiServerEnabled).toBe(false);

    dispose();
  });

  it("round-trips API server bind and port and warns for non-loopback binds", async () => {
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {} }),
      root,
    );
    await settle();

    const bind = byTestId<HTMLInputElement>("settings.general.apiServerBind");
    const port = byTestId<HTMLInputElement>("settings.general.apiServerPort");
    expect(bind.value).toBe("127.0.0.1");
    expect(port.value).toBe("8766");
    expect(
      document.querySelector('[data-ac-testid="settings.general.apiServerBind.warning"]'),
    ).toBeNull();

    bind.value = "0.0.0.0";
    bind.dispatchEvent(new Event("input", { bubbles: true }));
    port.value = "9000";
    port.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();

    expect(byTestId("settings.general.apiServerBind.warning").textContent).toContain(
      "firewall",
    );

    bind.value = "127.0.0.1";
    bind.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    expect(
      document.querySelector('[data-ac-testid="settings.general.apiServerBind.warning"]'),
    ).toBeNull();

    bind.value = "0.0.0.0";
    bind.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();

    byTestId<HTMLButtonElement>("settings.save").click();
    await settle();

    const saved = vi.mocked(SettingsAPI.saveDraft).mock.calls[0]?.[0];
    expect(saved?.apiServerBind).toBe("0.0.0.0");
    expect(saved?.apiServerPort).toBe(9000);

    dispose();
  });

  it("blocks save for empty API bind and out-of-range API port", async () => {
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {} }),
      root,
    );
    await settle();

    const bind = byTestId<HTMLInputElement>("settings.general.apiServerBind");
    const port = byTestId<HTMLInputElement>("settings.general.apiServerPort");
    const save = byTestId<HTMLButtonElement>("settings.save");

    bind.value = "";
    bind.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    expect(save.disabled).toBe(true);
    expect(document.querySelector(".modal-save-error")?.textContent).toContain(
      "IP/address must not be empty",
    );

    bind.value = "127.0.0.1";
    bind.dispatchEvent(new Event("input", { bubbles: true }));
    port.value = "70000";
    port.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    expect(save.disabled).toBe(true);
    expect(document.querySelector(".modal-save-error")?.textContent).toContain(
      "port must be between 1 and 65535",
    );

    dispose();
  });

  it("saves changed API endpoint before restarting a running API server", async () => {
    const runningSettings = settings({
      apiServerEnabled: true,
      apiServerBind: "127.0.0.1",
      apiServerPort: 8766,
    });
    vi.mocked(SettingsAPI.get)
      .mockResolvedValueOnce(runningSettings)
      .mockResolvedValueOnce(runningSettings);
    vi.mocked(SettingsAPI.apiServerStatus)
      .mockResolvedValueOnce(true)
      .mockResolvedValueOnce(true);
    vi.mocked(SettingsAPI.stopApiServer).mockResolvedValueOnce(true);
    vi.mocked(SettingsAPI.startApiServer).mockResolvedValueOnce(true);

    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {} }),
      root,
    );
    await settle();

    expect(byTestId("settings.general.apiServerStatus").textContent).toContain(
      "127.0.0.1:8766",
    );

    const bind = byTestId<HTMLInputElement>("settings.general.apiServerBind");
    bind.value = "0.0.0.0";
    bind.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();

    expect(byTestId("settings.general.apiServerStatus").textContent).toContain(
      "127.0.0.1:8766",
    );

    byTestId<HTMLButtonElement>("settings.save").click();
    await settle();
    await settle();
    await settle();

    const saved = vi.mocked(SettingsAPI.saveDraft).mock.calls[0]?.[0];
    expect(saved?.apiServerBind).toBe("0.0.0.0");
    expect(SettingsAPI.stopApiServer).toHaveBeenCalledTimes(1);
    expect(SettingsAPI.startApiServer).toHaveBeenCalledTimes(1);
    expect(SettingsAPI.apiServerStatus).toHaveBeenCalledTimes(2);

    const saveOrder = vi.mocked(SettingsAPI.saveDraft).mock.invocationCallOrder[0];
    const stopOrder = vi.mocked(SettingsAPI.stopApiServer).mock.invocationCallOrder[0];
    const startOrder = vi.mocked(SettingsAPI.startApiServer).mock.invocationCallOrder[0];
    const restartStatusOrder = vi.mocked(SettingsAPI.apiServerStatus).mock.invocationCallOrder[1];
    expect(saveOrder).toBeLessThan(stopOrder);
    expect(stopOrder).toBeLessThan(startOrder);
    expect(startOrder).toBeLessThan(restartStatusOrder);

    dispose();
  });

  it("surfaces API server start errors and reverts to live status", async () => {
    vi.mocked(SettingsAPI.apiServerStatus)
      .mockResolvedValueOnce(false)
      .mockResolvedValueOnce(false);
    vi.mocked(SettingsAPI.startApiServer).mockRejectedValueOnce("port unavailable");

    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {} }),
      root,
    );
    await settle();

    const toggle = byTestId<HTMLInputElement>("settings.general.apiServerEnabled");
    toggle.checked = true;
    toggle.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    await settle();

    expect(byTestId<HTMLInputElement>("settings.general.apiServerEnabled").checked).toBe(false);
    expect(document.querySelector(".modal-save-error")?.textContent).toContain(
      "API server start failed: port unavailable",
    );

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
    expandAgentRow(0);
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

  // #598 — the config-seed control renders for every agent (not Codex-only) and
  // its dest field is gated behind the enable toggle.
  it("exposes the config-seed toggle and reveals the dest field when enabled", async () => {
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {}, section: "agents" }),
      root,
    );
    await settle();
    expandAgentRow(0);
    await settle();

    // The seeded test agent has no configSeed, so the toggle starts unchecked
    // and the dest field is hidden until the seed is enabled.
    const toggle = byTestId<HTMLInputElement>("settings.agentRow.0.configSeedEnabled");
    expect(toggle.checked).toBe(false);
    expect(toggle.getAttribute("data-ac-state")).toBe("unchecked");
    expect(
      document.querySelector('[data-ac-testid="settings.agentRow.0.configSeedDest"]'),
    ).toBeNull();

    toggle.checked = true;
    toggle.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();

    expect(
      byTestId("settings.agentRow.0.configSeedEnabled").getAttribute("data-ac-state"),
    ).toBe("checked");
    const dest = byTestId<HTMLInputElement>("settings.agentRow.0.configSeedDest");
    expect(dest).toBeTruthy();

    dest.value = ".claude";
    dest.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();

    expect(byTestId<HTMLInputElement>("settings.agentRow.0.configSeedDest").value).toBe(".claude");

    dispose();
  });

  it("shows the runtime selector and reveals container-only fields with soft warnings", async () => {
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {}, section: "agents" }),
      root,
    );
    await settle();
    expandAgentRow(0);
    await settle();

    const runtime = byTestId<HTMLSelectElement>("settings.agentRow.0.runtimeKind");
    expect(runtime.value).toBe("localProcess");
    expect(document.querySelector('[data-ac-testid="settings.agentRow.0.containerImage"]')).toBeNull();
    expect(
      document.querySelector('[data-ac-testid="settings.agentRow.0.containerWarning.image"]'),
    ).toBeNull();
    expect(
      document.querySelector('[data-ac-testid="settings.agentRow.0.containerWarning.apiServer"]'),
    ).toBeNull();
    expect(
      document.querySelector('[data-ac-testid="settings.agentRow.0.containerWarning.bind"]'),
    ).toBeNull();
    expect(
      document.querySelector('[data-ac-testid="settings.agentRow.0.containerHint.env"]'),
    ).toBeNull();

    runtime.value = "containerTransport";
    runtime.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();

    const image = byTestId<HTMLInputElement>("settings.agentRow.0.containerImage");
    expect(image.placeholder).toBe(
      "Required unless AGENTSCOMMANDER_CONTAINER_IMAGE is set",
    );
    const imageHint = byTestId("settings.agentRow.0.containerHint.image").textContent ?? "";
    expect(imageHint).toContain(
      "Set a Docker image here, for example agentscommander/ac-claude:latest.",
    );
    expect(imageHint).toContain("AGENTSCOMMANDER_CONTAINER_IMAGE");
    expect(imageHint).toContain("no built-in image fallback");
    expect(imageHint).not.toContain("agentscommander/session-bridge:latest");
    const envHint = byTestId("settings.agentRow.0.containerHint.env").textContent ?? "";
    expect(envHint).toContain("Container sessions read CLAUDE_CONFIG_DIR");
    expect(envHint).toContain("%AC_REPLICA_ROOT%\\.claude");
    expect(envHint).toContain("auto-resume is skipped");
    expect(byTestId("settings.agentRow.0.containerWarning.image").textContent).toContain(
      "Saving this blank relies on AGENTSCOMMANDER_CONTAINER_IMAGE",
    );
    expect(byTestId("settings.agentRow.0.containerWarning.apiServer").textContent).toContain(
      "API server enabled",
    );
    expect(byTestId("settings.agentRow.0.containerWarning.bind").textContent).toContain(
      "loopback API bind",
    );

    runtime.value = "localProcess";
    runtime.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();

    expect(document.querySelector('[data-ac-testid="settings.agentRow.0.containerImage"]')).toBeNull();
    expect(
      document.querySelector('[data-ac-testid="settings.agentRow.0.containerWarning.image"]'),
    ).toBeNull();
    expect(
      document.querySelector('[data-ac-testid="settings.agentRow.0.containerWarning.apiServer"]'),
    ).toBeNull();
    expect(
      document.querySelector('[data-ac-testid="settings.agentRow.0.containerWarning.bind"]'),
    ).toBeNull();
    expect(
      document.querySelector('[data-ac-testid="settings.agentRow.0.containerHint.env"]'),
    ).toBeNull();

    dispose();
  });

  it("does not synthesize a container image when the field is left blank", async () => {
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {}, section: "agents" }),
      root,
    );
    await settle();
    expandAgentRow(0);
    await settle();

    const runtime = byTestId<HTMLSelectElement>("settings.agentRow.0.runtimeKind");
    runtime.value = "containerTransport";
    runtime.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();

    expect(document.body.textContent).not.toContain(
      "agentscommander/session-bridge:latest",
    );
    expect(byTestId<HTMLInputElement>("settings.agentRow.0.containerImage").value).toBe("");

    byTestId<HTMLButtonElement>("settings.save").click();
    await settle();

    const saved = vi.mocked(SettingsAPI.saveDraft).mock.calls[0]?.[0];
    expect(saved?.agents[0]?.backend).toEqual({
      kind: "containerTransport",
    });
    expect(saved?.agents[0]?.backend?.image).toBeUndefined();

    dispose();
  });

  it("saves a container runtime image through the settings draft", async () => {
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {}, section: "agents" }),
      root,
    );
    await settle();
    expandAgentRow(0);
    await settle();

    const runtime = byTestId<HTMLSelectElement>("settings.agentRow.0.runtimeKind");
    runtime.value = "containerTransport";
    runtime.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();

    const image = byTestId<HTMLInputElement>("settings.agentRow.0.containerImage");
    image.value = "  ghcr.io/ac/custom-agent:latest  ";
    image.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();

    byTestId<HTMLButtonElement>("settings.save").click();
    await settle();

    const saved = vi.mocked(SettingsAPI.saveDraft).mock.calls[0]?.[0];
    expect(saved?.agents[0]?.backend).toEqual({
      kind: "containerTransport",
      image: "ghcr.io/ac/custom-agent:latest",
    });

    dispose();
  });

  it("keeps an early custom agent draft when the fresh load resolves late", async () => {
    let resolveLoadedSettings: (value: SettingsSnapshot) => void = () => {};
    vi.mocked(SettingsAPI.get).mockReturnValueOnce(
      new Promise<SettingsSnapshot>((resolve) => {
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
    let resolveLoadedSettings: (value: SettingsSnapshot) => void = () => {};
    vi.mocked(SettingsAPI.get).mockReturnValueOnce(
      new Promise<SettingsSnapshot>((resolve) => {
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
    expandAgentRow(0);
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

  it("renders coding-agent rails, profile cards, command inputs, env rows, badges, and placeholder help", async () => {
    vi.mocked(SettingsAPI.get).mockResolvedValueOnce(settings({
      agents: [
        {
          id: "codex",
          label: "Codex",
          command: "codex",
          color: "#10b981",
          envs: [],
          isolatedHome: false,
        },
        {
          id: "claude",
          label: "Claude Code",
          command: "claude",
          color: "#d97706",
          envs: [],
          isolatedHome: false,
        },
      ],
      codingAgentProfiles: {
        schemaVersion: 2,
        profileSlots: {
          A: { label: "" },
          B: { label: "fast" },
        },
        defaultProfileByAgent: {},
        profileLabelsByAgent: {},
        profilesByAgent: {
          codex: {
            A: {
              enabled: true,
              command: "codex --model gpt-5-codex",
              env: { OPENAI_ORG: "ac-prod" },
              notes: "",
            },
          },
          // Claude configures B; codex does not → codex B renders as MISSING.
          claude: {
            A: { enabled: true, command: "claude", env: {}, notes: "" },
            B: { enabled: true, command: "claude --model opus", env: {}, notes: "" },
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
    await enterTwoRails();

    // Rail per coding agent, subtitle = binary basename (not model/sandbox).
    expect(byTestId("settings.profiles.section")).toBeTruthy();
    expect(byTestId("settings.profiles.rails")).toBeTruthy();
    expect(byTestId("settings.profileRail.0").getAttribute("data-ac-agent-id")).toBe("codex");
    expect(byTestId("settings.profileRail.0.subtitle").textContent).toContain("codex");

    // Profile A card: configured → MATCH, one command input, an env row.
    expect(byTestId("settings.profileCard.0.A").getAttribute("data-ac-state")).toBe("match");
    expect(byTestId("settings.profileCard.0.A.badge").textContent).toContain("MATCH");
    expect(byTestId<HTMLInputElement>("settings.profileCard.0.A.command").value).toBe("codex --model gpt-5-codex");
    expect(byTestId<HTMLInputElement>("settings.profileCard.0.A.label")).toBeTruthy();
    expect(byTestId("settings.profileCard.0.A.env")).toBeTruthy();
    expect(byTestId<HTMLInputElement>("settings.profileCard.0.A.envRow.0.key").value).toBe("OPENAI_ORG");
    expect(byTestId<HTMLInputElement>("settings.profileCard.0.A.envRow.0.value").value).toBe("ac-prod");

    // #576: an expanded profile card surfaces an always-visible "?" help button
    // whose native tooltip + aria-label list the three AC path placeholder
    // tokens. The expected tokens are the shared profile-utils constants (which
    // mirror the backend), so this asserts the byte-for-byte match end to end.
    const help = byTestId<HTMLButtonElement>("settings.profileCard.0.A.placeholderHelp");
    expect(help.tagName).toBe("BUTTON");
    expect(help.getAttribute("type")).toBe("button");
    for (const token of [
      AC_REPLICA_ROOT_PLACEHOLDER,
      AC_WORKSPACE_ROOT_PLACEHOLDER,
      AC_MATRIX_ROOT_PLACEHOLDER,
    ]) {
      expect(help.getAttribute("title")).toContain(token);
      expect(help.getAttribute("aria-label")).toContain(token);
    }

    // #538: codex has no B cell, but B no longer shows an "Add cell" / missing
    // box. The badge still honestly reports MISSING (codex's B falls back to its
    // base command until a command is typed).
    expect(byTestId("settings.profileCard.0.B").getAttribute("data-ac-state")).toBe("missing");
    expect(byTestId("settings.profileCard.0.B.badge").textContent).toContain("MISSING");
    // #597: the sub-line shows the agent base binary the cell params append to
    // (here `codex`), consistent with the MISSING badge (B has no params of its
    // own yet) instead of a contradictory bare "Configured".
    expect(
      byTestId("settings.profileCard.0.B").querySelector(".settings-profile-card-sub")?.textContent,
    ).toBe("codex");
    expect(document.querySelector('[data-ac-testid="settings.profileCard.0.B.missing"]')).toBeNull();
    expect(document.querySelector('[data-ac-testid="settings.profileCard.0.B.add"]')).toBeNull();
    // Instead it exposes the same expand/edit affordance as every cell: a chevron
    // toggle plus an expanded-by-default empty command input the user can fill in.
    expect(byTestId<HTMLButtonElement>("settings.profileCard.0.B.toggle")).toBeTruthy();
    expect(byTestId<HTMLInputElement>("settings.profileCard.0.B.command").value).toBe("");
    // Claude rail: B has its own enabled cell (non-A, direct match) → CONFIGURED,
    // NOT MATCH. MATCH is reserved for the A baseline (#526).
    expect(byTestId("settings.profileCard.1.B").getAttribute("data-ac-state")).toBe("configured");
    expect(byTestId("settings.profileCard.1.B.badge").textContent).toContain("CONFIGURED");
    // Claude A is the baseline → MATCH.
    expect(byTestId("settings.profileCard.1.A").getAttribute("data-ac-state")).toBe("match");
    expect(byTestId("settings.profileCard.1.A.badge").textContent).toContain("MATCH");
    expect(byTestId<HTMLButtonElement>("settings.profiles.add").disabled).toBe(false);

    dispose();
  });

  it("#548: a per-agent label edit isolates to that rail (no cross-rail, no legacy-slot write)", async () => {
    vi.mocked(SettingsAPI.get).mockResolvedValueOnce(settings({
      agents: [
        {
          id: "codex", label: "Codex", command: "codex", color: "#10b981",
          envs: [], isolatedHome: false,
        },
        {
          id: "claude", label: "Claude Code", command: "claude", color: "#d97706",
          envs: [], isolatedHome: false,
        },
      ],
      codingAgentProfiles: {
        schemaVersion: 2,
        profileSlots: { A: { label: "" }, B: { label: "fast" } },
        defaultProfileByAgent: {},
        profileLabelsByAgent: {},
        profilesByAgent: {
          codex: { A: { enabled: true, command: "codex", env: {}, notes: "" } },
          claude: { A: { enabled: true, command: "claude", env: {}, notes: "" } },
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
    await enterTwoRails();

    // Rail 0 = codex (primigenio), rail 1 = claude. Type a name into codex's B label.
    const codexB = byTestId<HTMLInputElement>("settings.profileCard.0.B.label");
    codexB.value = "turbo";
    codexB.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();

    // codex's own override shows the typed value; claude's OWN B override stays empty
    // (claude inherits via the placeholder, but its stored override is untouched).
    expect(byTestId<HTMLInputElement>("settings.profileCard.0.B.label").value).toBe("turbo");
    expect(byTestId<HTMLInputElement>("settings.profileCard.1.B.label").value).toBe("");

    document.querySelector<HTMLButtonElement>('[data-ac-testid="settings.save"]')?.click();
    await settle();

    const saved = vi.mocked(SettingsAPI.saveDraft).mock.calls[0]?.[0];
    // Only codex.B got the override; claude has none; the legacy slot label is never
    // written by the per-agent input (it stays the back-compat fallback).
    expect(saved?.codingAgentProfiles.profileLabelsByAgent).toEqual({ codex: { B: "turbo" } });
    expect(saved?.codingAgentProfiles.profileLabelsByAgent.claude).toBeUndefined();
    expect(saved?.codingAgentProfiles.profileSlots.B.label).toBe("fast");

    dispose();
  });

  it("expands non-A profile cards by default and toggles them closed and open", async () => {
    vi.mocked(SettingsAPI.get).mockResolvedValueOnce(settings({
      agents: [
        {
          id: "claude",
          label: "Claude Code",
          command: "claude",
          color: "#d97706",
          envs: [],
          isolatedHome: false,
        },
      ],
      codingAgentProfiles: {
        schemaVersion: 2,
        profileSlots: { A: { label: "" }, B: { label: "fast" } },
        defaultProfileByAgent: {},
        profileLabelsByAgent: {},
        profilesByAgent: {
          claude: {
            A: { enabled: true, command: "claude", env: {}, notes: "" },
            B: { enabled: true, command: "claude --model opus", env: {}, notes: "" },
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

    // All profile slots are expanded by default, including non-A slots.
    expect(byTestId("settings.profileCard.0.A.command")).toBeTruthy();
    expect(byTestId<HTMLInputElement>("settings.profileCard.0.B.command").value).toBe("claude --model opus");
    expect(byTestId<HTMLButtonElement>("settings.profileCard.0.B.toggle").getAttribute("aria-expanded")).toBe("true");

    byTestId<HTMLButtonElement>("settings.profileCard.0.B.toggle").click();
    await settle();
    expect(document.querySelector('[data-ac-testid="settings.profileCard.0.B.command"]')).toBeNull();
    expect(byTestId<HTMLButtonElement>("settings.profileCard.0.B.toggle").getAttribute("aria-expanded")).toBe("false");

    byTestId<HTMLButtonElement>("settings.profileCard.0.B.toggle").click();
    await settle();
    expect(byTestId<HTMLInputElement>("settings.profileCard.0.B.command").value).toBe("claude --model opus");

    dispose();
  });

  it("clears the right comparison rail from the rail's own Clear button", async () => {
    vi.mocked(SettingsAPI.get).mockResolvedValueOnce(settings({
      agents: [
        {
          id: "codex",
          label: "Codex",
          command: "codex",
          color: "#10b981",
          envs: [],
          isolatedHome: false,
        },
        {
          id: "claude",
          label: "Claude Code",
          command: "claude",
          color: "#d97706",
          envs: [],
          isolatedHome: false,
        },
      ],
    }));
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {}, section: "agents" }),
      root,
    );
    await settle();
    await enterTwoRails();

    // The dropdown reveals the comparison pair: left=codex, right=claude (second agent).
    expect(byTestId("settings.profileRail.1").getAttribute("data-ac-agent-id")).toBe("claude");
    // #895: clearing now lives on the rail itself, not on the agent row. It still
    // actually empties the rail (no positional re-fill). #1095: the emptied rail
    // now unmounts entirely, collapsing back to the 1-rail view.
    byTestId<HTMLButtonElement>("settings.profileRail.1.clear").click();
    await settle();
    expect(document.querySelector('[data-ac-testid="settings.profileRail.1"]')).toBeNull();
    // #895: Claude is available again, and the row head itself re-adds it. Neither
    // a "Use" nor a "Remove" button survives on any agent row — that per-row
    // button was what made one row taller than the rest.
    expect(document.querySelector('[data-ac-testid="settings.agentRow.1.use"]')).toBeNull();
    expect(document.querySelector('[data-ac-testid="settings.agentRow.1.unuse"]')).toBeNull();
    expect(byTestId("settings.agentRow.1").getAttribute("data-ac-rail")).toBe("available");
    // The emptied rail has no header, so no Clear button either.
    expect(document.querySelector('[data-ac-testid="settings.profileRail.1.clear"]')).toBeNull();

    dispose();
  });

  it("shows the red invalid badge for a bad command string and blocks save", async () => {
    vi.mocked(SettingsAPI.get).mockResolvedValueOnce(settings({
      codingAgentProfiles: {
        schemaVersion: 2,
        profileSlots: { A: { label: "" } },
        defaultProfileByAgent: {},
        profileLabelsByAgent: {},
        profilesByAgent: {
          codex: { A: { enabled: true, command: "codex", env: {}, notes: "" } },
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

    const command = byTestId<HTMLInputElement>("settings.profileCard.0.A.command");
    command.value = 'codex --review --prompt "missing close';
    command.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();

    expect(byTestId("settings.profileCard.0.A").getAttribute("data-ac-state")).toBe("invalid");
    expect(byTestId("settings.profileCard.0.A.command.error")).toBeTruthy();

    byTestId<HTMLButtonElement>("settings.save").click();
    await settle();

    // Save is blocked by the parse error; no draft is persisted.
    expect(SettingsAPI.saveDraft).not.toHaveBeenCalled();
    expect(document.querySelector('[data-ac-testid="settings.modal"] .modal-save-error')).toBeTruthy();

    dispose();
  });

  it("renders the %AC_REPLICA_ROOT% template preview for env rows", async () => {
    vi.mocked(SettingsAPI.get).mockResolvedValueOnce(settings({
      codingAgentProfiles: {
        schemaVersion: 2,
        profileSlots: { A: { label: "" } },
        defaultProfileByAgent: {},
        profileLabelsByAgent: {},
        profilesByAgent: {
          codex: {
            A: {
              enabled: true,
              command: "codex",
              env: { CODEX_HOME: "%AC_REPLICA_ROOT%\\.codex\\agents\\codex" },
              notes: "",
            },
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

    expect(byTestId("settings.profileCard.0.A.envRow.0.placeholder")).toBeTruthy();

    dispose();
  });

  it("#614: typing in a profile env value keeps the same input focused (no <For> recreate)", async () => {
    vi.mocked(SettingsAPI.get).mockResolvedValueOnce(settings({
      codingAgentProfiles: {
        schemaVersion: 2,
        profileSlots: { A: { label: "" } },
        defaultProfileByAgent: {},
        profileLabelsByAgent: {},
        profilesByAgent: {
          codex: {
            A: { enabled: true, command: "codex", env: { OPENAI_ORG: "ac" }, notes: "" },
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

    const valueInput = byTestId<HTMLInputElement>("settings.profileCard.0.A.envRow.0.value");
    expect(valueInput.value).toBe("ac");
    valueInput.focus();
    expect(document.activeElement).toBe(valueInput);

    // Two keystrokes. Pre-#614 (reference-keyed <For>), updateCellEnvRow replaced
    // the row object on each input, disposing this <input> and mounting a new
    // node — so focus reverted to <body> and a re-query returned a DIFFERENT
    // element. With <Index> the node is stable, so identity + focus survive.
    valueInput.value = "acX";
    valueInput.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    valueInput.value = "acXY";
    valueInput.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();

    const afterTyping = byTestId<HTMLInputElement>("settings.profileCard.0.A.envRow.0.value");
    expect(afterTyping).toBe(valueInput); // same DOM node — not recreated
    expect(document.activeElement).toBe(valueInput); // focus retained
    expect(afterTyping.value).toBe("acXY"); // edit applied

    dispose();
  });

  it("#614: typing in an agent-level env value also keeps focus (in-place store update)", async () => {
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {}, section: "agents" }),
      root,
    );
    await settle();
    expandAgentRow(0);
    await settle();

    byTestId<HTMLButtonElement>("settings.agentRow.0.env.add").click();
    await settle();

    const valueInput = byTestId<HTMLInputElement>("settings.agentRow.0.envRow.0.value");
    valueInput.focus();
    expect(document.activeElement).toBe(valueInput);

    valueInput.value = "v1";
    valueInput.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();

    const afterTyping = byTestId<HTMLInputElement>("settings.agentRow.0.envRow.0.value");
    expect(afterTyping).toBe(valueInput);
    expect(document.activeElement).toBe(valueInput);
    expect(afterTyping.value).toBe("v1");

    dispose();
  });

  it("keeps early profile cell edits when the fresh load resolves late", async () => {
    let resolveLoadedSettings: (value: SettingsSnapshot) => void = () => {};
    vi.mocked(SettingsAPI.get).mockReturnValueOnce(
      new Promise<SettingsSnapshot>((resolve) => {
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

    const commandInput = document.querySelector<HTMLInputElement>('[data-ac-testid="settings.profileCard.0.A.command"]');
    if (!commandInput) throw new Error("missing profile command input");
    commandInput.value = "codex --fast";
    commandInput.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();

    resolveLoadedSettings(settings());
    await settle();

    document.querySelector<HTMLButtonElement>('[data-ac-testid="settings.save"]')?.click();
    await settle();

    const saved = vi.mocked(SettingsAPI.saveDraft).mock.calls[0]?.[0];
    expect(saved?.codingAgentProfiles.profilesByAgent.codex?.A?.command).toBe("codex --fast");

    dispose();
  });

  it("distinguishes MATCH (A), CONFIGURED (own non-A cell), FALLBACK, and MISSING", async () => {
    vi.mocked(SettingsAPI.get).mockResolvedValueOnce(settings({
      agents: [
        {
          id: "codex",
          label: "Codex",
          command: "codex",
          color: "#10b981",
          envs: [],
          isolatedHome: false,
        },
        {
          id: "claude",
          label: "Claude Code",
          command: "claude",
          color: "#d97706",
          envs: [],
          isolatedHome: false,
        },
      ],
      codingAgentProfiles: {
        schemaVersion: 2,
        profileSlots: {
          A: { label: "" },
          B: { label: "balanced" },
          C: { label: "review" },
          D: { label: "full-auto" },
        },
        defaultProfileByAgent: {},
        profileLabelsByAgent: {},
        profilesByAgent: {
          codex: {
            A: { enabled: true, command: "codex --sandbox workspace-write", env: {}, notes: "" },
            // Own enabled non-A cell → CONFIGURED (direct match on a non-baseline slot).
            B: { enabled: true, command: "codex --model gpt-5-codex --effort medium", env: {}, notes: "" },
          },
          // D is configured on claude but not on codex → codex D is MISSING.
          claude: {
            A: { enabled: true, command: "claude", env: {}, notes: "" },
            D: { enabled: true, command: "claude --dangerously-skip-permissions", env: {}, notes: "" },
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

    // A baseline → MATCH (green).
    expect(byTestId("settings.profileCard.0.A").getAttribute("data-ac-state")).toBe("match");
    expect(byTestId("settings.profileCard.0.A.badge").textContent).toContain("MATCH");
    // B has its own cell on codex → CONFIGURED (cyan), NOT MATCH.
    expect(byTestId("settings.profileCard.0.B").getAttribute("data-ac-state")).toBe("configured");
    expect(byTestId("settings.profileCard.0.B.badge").textContent).toContain("CONFIGURED");
    // C has no cell anywhere → resolves through the configured B → FALLBACK.
    expect(byTestId("settings.profileCard.0.C").getAttribute("data-ac-state")).toBe("fallback");
    expect(byTestId("settings.profileCard.0.C.badge").textContent).toContain("FALLBACK");
    // D is configured on claude but absent on codex → MISSING.
    expect(byTestId("settings.profileCard.0.D").getAttribute("data-ac-state")).toBe("missing");
    expect(byTestId("settings.profileCard.0.D.badge").textContent).toContain("MISSING");

    dispose();
  });

  it("deletes a whole profile slot from every agent via Delete Profile", async () => {
    vi.mocked(SettingsAPI.get).mockResolvedValueOnce(settings({
      agents: [
        {
          id: "codex",
          label: "Codex",
          command: "codex",
          color: "#10b981",
          envs: [],
          isolatedHome: false,
        },
        {
          id: "claude",
          label: "Claude Code",
          command: "claude",
          color: "#d97706",
          envs: [],
          isolatedHome: false,
        },
      ],
      codingAgentProfiles: {
        schemaVersion: 2,
        profileSlots: { A: { label: "" }, B: { label: "fast" } },
        defaultProfileByAgent: {},
        profileLabelsByAgent: {},
        profilesByAgent: {
          codex: {
            A: { enabled: true, command: "codex", env: {}, notes: "" },
            B: { enabled: true, command: "codex --profile fast", env: {}, notes: "" },
          },
          claude: {
            A: { enabled: true, command: "claude", env: {}, notes: "" },
            B: { enabled: true, command: "claude --model opus", env: {}, notes: "" },
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
    await enterTwoRails();

    // The B card renders on both rails before deletion.
    expect(byTestId("settings.profileCard.0.B")).toBeTruthy();
    expect(byTestId("settings.profileCard.1.B")).toBeTruthy();

    // B starts expanded, so the slot-level "Delete Profile" affordance is visible.
    byTestId<HTMLButtonElement>("settings.profileCard.0.B.deleteProfile").click();
    await settle();

    // Slot B is gone from BOTH rails (it was a whole-slot delete, not per-agent).
    expect(document.querySelector('[data-ac-testid="settings.profileCard.0.B"]')).toBeNull();
    expect(document.querySelector('[data-ac-testid="settings.profileCard.1.B"]')).toBeNull();

    byTestId<HTMLButtonElement>("settings.save").click();
    await settle();

    const saved = vi.mocked(SettingsAPI.saveDraft).mock.calls[0]?.[0];
    expect(saved?.codingAgentProfiles.profileSlots.B).toBeUndefined();
    expect(saved?.codingAgentProfiles.profilesByAgent.codex?.B).toBeUndefined();
    expect(saved?.codingAgentProfiles.profilesByAgent.claude?.B).toBeUndefined();
    // The A baseline is untouched.
    expect(saved?.codingAgentProfiles.profileSlots.A).toBeTruthy();

    dispose();
  });

  it("keeps the left rail pinned when swapping with an empty right rail", async () => {
    vi.mocked(SettingsAPI.get).mockResolvedValueOnce(settings({
      agents: [
        {
          id: "codex",
          label: "Codex",
          command: "codex",
          color: "#10b981",
          envs: [],
          isolatedHome: false,
        },
        {
          id: "claude",
          label: "Claude Code",
          command: "claude",
          color: "#d97706",
          envs: [],
          isolatedHome: false,
        },
      ],
    }));
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {}, section: "agents" }),
      root,
    );
    await settle();

    // Pin the LEFT rail to claude (a non-first agent). #1095: the right rail is
    // empty by default — left=claude (pinned), right=empty (second rail unmounted).
    const leftSelect = byTestId<HTMLSelectElement>("settings.profileRail.0.agentSelect");
    leftSelect.value = "claude";
    leftSelect.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    expect(byTestId("settings.profileRail.0").getAttribute("data-ac-agent-id")).toBe("claude");
    expect(document.querySelector('[data-ac-testid="settings.profileRail.1"]')).toBeNull();

    // Swap with an empty right rail must be a no-op — NOT drop the left pin to the
    // positional fallback (codex). The left stays claude; the right stays empty.
    byTestId<HTMLButtonElement>("settings.agents.swapRails").click();
    await settle();
    expect(byTestId("settings.profileRail.0").getAttribute("data-ac-agent-id")).toBe("claude");
    expect(document.querySelector('[data-ac-testid="settings.profileRail.1"]')).toBeNull();

    dispose();
  });

  it("exposes the OpenCode quick-add preset and the instructions-file input (#529)", async () => {
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {}, section: "agents" }),
      root,
    );
    await settle();

    // Quick-add OpenCode button is present and enabled (seeded agent is codex).
    const opencodePreset = byTestId<HTMLButtonElement>("settings.agentPreset.opencode");
    expect(opencodePreset).toBeTruthy();
    expect(opencodePreset.disabled).toBe(false);
    expect(opencodePreset.getAttribute("data-ac-state")).toBe("available");

    // The per-agent instructions-file input lives in the collapsed editor (3-tab
    // rework) — expand the row first. Placeholder = command-derived default
    // (seeded command `codex` → AGENTS.md).
    expandAgentRow(0);
    await settle();
    const input = byTestId<HTMLInputElement>("settings.agentRow.0.instructionsFilename");
    expect(input).toBeTruthy();
    expect(input.value).toBe("");
    expect(input.placeholder).toBe("AGENTS.md");

    dispose();
  });

  it("disables the OpenCode preset when an opencode agent already exists (#529)", async () => {
    vi.mocked(SettingsAPI.get).mockResolvedValueOnce(settings({
      agents: [
        {
          id: "opencode",
          label: "OpenCode",
          command: "opencode",
          color: "#64748b",
          envs: [],
          isolatedHome: false,
          instructionsFilename: "AGENTS.md",
        },
      ],
    }));
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {}, section: "agents" }),
      root,
    );
    await settle();

    const opencodePreset = byTestId<HTMLButtonElement>("settings.agentPreset.opencode");
    expect(opencodePreset.disabled).toBe(true);
    expect(opencodePreset.getAttribute("data-ac-state")).toBe("disabled");

    dispose();
  });

  it("round-trips a typed instructions filename through save (#529)", async () => {
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {}, section: "agents" }),
      root,
    );
    await settle();
    expandAgentRow(0);
    await settle();

    const input = byTestId<HTMLInputElement>("settings.agentRow.0.instructionsFilename");
    input.value = "TEAM.md";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();

    byTestId<HTMLButtonElement>("settings.save").click();
    await settle();

    const saved = vi.mocked(SettingsAPI.saveDraft).mock.calls[0]?.[0];
    expect(saved?.agents[0]?.instructionsFilename).toBe("TEAM.md");

    dispose();
  });

  it("omits a cleared instructions filename from the saved draft (#529 G3)", async () => {
    vi.mocked(SettingsAPI.get).mockResolvedValueOnce(settings({
      agents: [
        {
          id: "claude",
          label: "Claude Code",
          command: "claude",
          color: "#d97706",
          envs: [],
          isolatedHome: false,
          instructionsFilename: "CLAUDE.md",
        },
      ],
    }));
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {}, section: "agents" }),
      root,
    );
    await settle();
    expandAgentRow(0);
    await settle();

    const input = byTestId<HTMLInputElement>("settings.agentRow.0.instructionsFilename");
    expect(input.value).toBe("CLAUDE.md");
    input.value = "";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();

    byTestId<HTMLButtonElement>("settings.save").click();
    await settle();

    const saved = vi.mocked(SettingsAPI.saveDraft).mock.calls[0]?.[0];
    expect(saved?.agents[0]).not.toHaveProperty("instructionsFilename");

    dispose();
  });

  it("round-trips coordinatorCascadeCloseEnabled through the General cascade checkbox (#588)", async () => {
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {} }),
      root,
    );
    await settle();

    const checkbox = byTestId<HTMLInputElement>("settings.general.coordinatorCascadeCloseEnabled");
    // The label is the issue's verbatim wording.
    expect(checkbox.closest("label")?.textContent).toContain(
      "Always close team members when manually closing Coordinator",
    );
    // Loaded default is true (the seeded settings have it on).
    expect(checkbox.checked).toBe(true);

    // Toggle OFF and save -> the persisted draft carries the new value, proving
    // updateField accepts the new AppSettings key end to end.
    checkbox.checked = false;
    checkbox.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();

    byTestId<HTMLButtonElement>("settings.save").click();
    await settle();

    const saved = vi.mocked(SettingsAPI.saveDraft).mock.calls[0]?.[0];
    expect(saved?.coordinatorCascadeCloseEnabled).toBe(false);

    dispose();
  });

  it("round-trips coordinatorAutoCloseSkipTelegramAssigned through the General auto-close checkbox (#817)", async () => {
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {} }),
      root,
    );
    await settle();

    const autoCloseEnabled = byTestId<HTMLInputElement>(
      "settings.general.coordinatorAutoCloseEnabled",
    );
    const checkbox = byTestId<HTMLInputElement>(
      "settings.general.coordinatorAutoCloseSkipTelegramAssigned",
    );
    expect(checkbox.closest("label")?.textContent).toContain(
      "Skip Telegram-assigned sessions during auto-close",
    );
    expect(checkbox.checked).toBe(false);
    expect(checkbox.disabled).toBe(false);

    autoCloseEnabled.checked = false;
    autoCloseEnabled.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    expect(checkbox.disabled).toBe(true);

    autoCloseEnabled.checked = true;
    autoCloseEnabled.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    expect(checkbox.disabled).toBe(false);

    checkbox.checked = true;
    checkbox.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();

    byTestId<HTMLButtonElement>("settings.save").click();
    await settle();

    const saved = vi.mocked(SettingsAPI.saveDraft).mock.calls[0]?.[0];
    expect(saved?.coordinatorAutoCloseSkipTelegramAssigned).toBe(true);

    dispose();
  });

  it("round-trips npmUpdateNotificationsEnabled through the General update-notify checkbox (#609)", async () => {
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {} }),
      root,
    );
    await settle();

    const checkbox = byTestId<HTMLInputElement>("settings.general.npmUpdateNotificationsEnabled");
    expect(checkbox.closest("label")?.textContent).toContain(
      "Notify me when a new version is available",
    );
    // Loaded default is true (the seeded settings have it on).
    expect(checkbox.checked).toBe(true);

    // Toggle OFF and save -> the persisted draft carries the new value, proving
    // updateField accepts the new AppSettings key end to end.
    checkbox.checked = false;
    checkbox.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();

    byTestId<HTMLButtonElement>("settings.save").click();
    await settle();

    const saved = vi.mocked(SettingsAPI.saveDraft).mock.calls[0]?.[0];
    expect(saved?.npmUpdateNotificationsEnabled).toBe(false);

    dispose();
  });

  it("preserves fresh titlebar webserver changes when saving a stale modal draft", async () => {
    const modalSeed = settings({
      webServerEnabled: false,
      webServerPort: 8765,
      webServerBind: "127.0.0.1",
      npmUpdateNotificationsEnabled: true,
    });
    const freshAfterTitlebarChange = settings({
      webServerEnabled: true,
      webServerPort: 9000,
      webServerBind: "127.0.0.1",
      npmUpdateNotificationsEnabled: true,
    });
    vi.mocked(SettingsAPI.get)
      .mockResolvedValueOnce(modalSeed)
      .mockResolvedValueOnce(freshAfterTitlebarChange);

    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {} }),
      root,
    );
    await settle();

    const checkbox = byTestId<HTMLInputElement>("settings.general.npmUpdateNotificationsEnabled");
    checkbox.checked = false;
    checkbox.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();

    byTestId<HTMLButtonElement>("settings.save").click();
    await settle();

    const saved = vi.mocked(SettingsAPI.saveDraft).mock.calls[0]?.[0];
    expect(saved?.webServerEnabled).toBe(true);
    expect(saved?.webServerPort).toBe(9000);
    expect(saved?.webServerBind).toBe("127.0.0.1");
    expect(saved?.npmUpdateNotificationsEnabled).toBe(false);

    dispose();
  });

  it("round-trips autoSelfClearEnabled through the General auto-self-clear checkbox (#640)", async () => {
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {} }),
      root,
    );
    await settle();

    const checkbox = byTestId<HTMLInputElement>("settings.general.autoSelfClearEnabled");
    expect(checkbox.closest("label")?.textContent).toContain(
      "Auto-clear and hand off context after 3 closed topics",
    );
    // Loaded default is true (the global master defaults on).
    expect(checkbox.checked).toBe(true);

    // Toggle OFF and save -> the persisted draft carries the new value, proving
    // updateField accepts the new AppSettings key end to end.
    checkbox.checked = false;
    checkbox.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();

    byTestId<HTMLButtonElement>("settings.save").click();
    await settle();

    const saved = vi.mocked(SettingsAPI.saveDraft).mock.calls[0]?.[0];
    expect(saved?.autoSelfClearEnabled).toBe(false);

    dispose();
  });

  it("round-trips containerCredentialsFromHost through the General reuse checkbox (#930)", async () => {
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {} }),
      root,
    );
    await settle();

    const checkbox = byTestId<HTMLInputElement>("settings.general.containerCredentialsFromHost");
    expect(checkbox.closest("label")?.textContent).toContain(
      "Reuse host login for container coding agents",
    );
    // Loaded default is true (copy-in is the default auth path for #930).
    expect(checkbox.checked).toBe(true);

    // Toggle OFF and save -> the persisted draft carries the new value, proving
    // updateField accepts the new AppSettings key end to end.
    checkbox.checked = false;
    checkbox.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();

    byTestId<HTMLButtonElement>("settings.save").click();
    await settle();

    const saved = vi.mocked(SettingsAPI.saveDraft).mock.calls[0]?.[0];
    expect(saved?.containerCredentialsFromHost).toBe(false);

    dispose();
  });

  it("shows the host-login-on hint when Container runtime is selected (default #930)", async () => {
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {}, section: "agents" }),
      root,
    );
    await settle();
    expandAgentRow(0);
    await settle();

    // Local runtime: no credential hint, and no container caveat.
    expect(
      document.querySelector('[data-ac-testid="settings.agentRow.0.containerHint.hostLogin"]'),
    ).toBeNull();
    expect(
      document.querySelector('[data-ac-testid="settings.agentRow.0.containerHint.hostLoginOff"]'),
    ).toBeNull();
    expect(
      document.querySelector('[data-ac-testid="settings.agentRow.0.containerHint.inProgress"]'),
    ).toBeNull();

    const runtime = byTestId<HTMLSelectElement>("settings.agentRow.0.runtimeKind");
    runtime.value = "containerTransport";
    runtime.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();

    // Setting defaults ON -> the on-branch hint renders; the off-branch stays absent.
    const onHint = byTestId("settings.agentRow.0.containerHint.hostLogin").textContent ?? "";
    expect(onHint).toContain("Host login reuse is on");
    expect(onHint).toContain("removes them when the session stops");
    // #930 grinch review: AC pre-answers a safety dialog, so the hint must disclose
    // the first-run state it stamps inside the container.
    expect(onHint).toContain("onboarding is marked complete");
    expect(onHint).toContain("/workspace is marked as trusted");
    expect(onHint).toContain("folder-trust safety prompt");
    expect(
      document.querySelector('[data-ac-testid="settings.agentRow.0.containerHint.hostLoginOff"]'),
    ).toBeNull();

    // #930 landing condition: the feature is in progress and a container agent still
    // cannot reach the repos (#935). The caveat is a property of the RUNTIME, not of
    // credential reuse, so it lives outside the reuse Show - here it must render with
    // reuse ON, and it must NOT be duplicated inside the reuse hint.
    const caveat = byTestId("settings.agentRow.0.containerHint.inProgress").textContent ?? "";
    expect(caveat).toContain("In progress");
    expect(caveat).toContain("cannot reach your repos yet (#935)");
    expect(caveat).toContain("Local runtime for repo work");
    expect(onHint).not.toContain("#935");

    // Switch back to local -> both the credential hint and the caveat die with the block.
    runtime.value = "localProcess";
    runtime.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    expect(
      document.querySelector('[data-ac-testid="settings.agentRow.0.containerHint.hostLogin"]'),
    ).toBeNull();
    expect(
      document.querySelector('[data-ac-testid="settings.agentRow.0.containerHint.inProgress"]'),
    ).toBeNull();

    dispose();
  });

  it("shows the host-login-off hint when reuse is disabled and Container is selected (#930)", async () => {
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {} }),
      root,
    );
    await settle();

    // Turn the global reuse setting OFF from the General tab.
    const toggle = byTestId<HTMLInputElement>("settings.general.containerCredentialsFromHost");
    expect(toggle.checked).toBe(true);
    toggle.checked = false;
    toggle.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();

    // Move to Coding Agents and select Container runtime for the row. The draft
    // store persists across tabs, so the OFF value carries over.
    byTestId<HTMLButtonElement>("settings.tab.agents").click();
    await settle();
    expandAgentRow(0);
    await settle();
    const runtime = byTestId<HTMLSelectElement>("settings.agentRow.0.runtimeKind");
    runtime.value = "containerTransport";
    runtime.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();

    // Off-branch (warning) hint renders; the on-branch hint is absent.
    const offHint = byTestId("settings.agentRow.0.containerHint.hostLoginOff").textContent ?? "";
    expect(offHint).toContain("Host login reuse is off");
    expect(offHint).toContain("CLAUDE_CODE_OAUTH_TOKEN");
    // The OFF branch stamps nothing, so it must NOT claim any first-run state.
    expect(offHint).not.toContain("onboarding");
    expect(
      document.querySelector('[data-ac-testid="settings.agentRow.0.containerHint.hostLogin"]'),
    ).toBeNull();

    // The reason the caveat was hoisted out of the reuse Show: a user with reuse OFF is
    // still picking the Container runtime, and must still be told it cannot reach repos.
    const caveat = byTestId("settings.agentRow.0.containerHint.inProgress").textContent ?? "";
    expect(caveat).toContain("In progress");
    expect(caveat).toContain("cannot reach your repos yet (#935)");
    expect(caveat).toContain("Local runtime for repo work");

    dispose();
  });

  it("discloses the container first-run stamping in the General hint (#930)", async () => {
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => SettingsModal({ onClose: () => {} }),
      root,
    );
    await settle();

    const hint =
      byTestId("settings.general.containerCredentialsFromHost.hint").textContent ?? "";

    // Copy + teardown (what the backend does with the credential itself).
    expect(hint).toContain("deletes it when the session stops");
    // grinch review: AC answers a safety dialog for the user, so the hint must say
    // so - onboarding stamped complete + /workspace trusted inside the container.
    expect(hint).toContain("onboarding is marked complete");
    expect(hint).toContain("/workspace folder is marked as trusted");
    expect(hint).toContain("on your behalf");
    // The host side is untouched, and the refresh-token drift clause survives (14.5).
    expect(hint).toContain("Your host config is never modified");
    expect(hint).toContain("token refresh in one place can require re-login");
    // #930 lands as in-progress: the repo access limitation (#935) is stated up front.
    expect(hint).toContain("In progress");
    expect(hint).toContain("cannot reach your repos yet (#935)");
    expect(hint).toContain("Local runtime");

    dispose();
  });
});
