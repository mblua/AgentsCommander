// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { FakeTransport } from "./testing/fake-transport";
import { baseSettings, settingsSnapshot } from "./testing/ui-harness";
import { liveSelection, SESSION_A } from "./testing/session-selection";

describe("shared ipc transport seam", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
    vi.resetModules();
  });

  it("does not construct WebSocket transport on jsdom import before fake install", async () => {
    vi.resetModules();
    const websocketCtor = vi.fn(() => {
      throw new Error("real WebSocket should not be constructed");
    });
    vi.stubGlobal("WebSocket", websocketCtor);
    const setTimeoutSpy = vi.spyOn(globalThis, "setTimeout");

    const ipc = await import("./ipc");

    expect(websocketCtor).not.toHaveBeenCalled();
    expect(setTimeoutSpy).not.toHaveBeenCalled();

    const fake = new FakeTransport();
    fake.resolve("get_settings", baseSettings());
    const restore = ipc.__setTransportForTests(fake);
    try {
      await expect(ipc.SettingsAPI.get()).resolves.toMatchObject({
        defaultShell: "pwsh",
      });
    } finally {
      restore();
    }

    expect(fake.lastCall("get_settings")?.args).toEqual({});
    expect(websocketCtor).not.toHaveBeenCalled();
    expect(setTimeoutSpy).not.toHaveBeenCalled();
  });

  it("returns the structured SettingsSnapshot from get_settings without text parsing", async () => {
    const ipc = await import("./ipc");
    const fake = new FakeTransport();
    const snapshot = settingsSnapshot(
      { projectPaths: ["C:\\bundle\\projects\\alpha"] },
      {
        activeRegistrationCount: 2,
        issues: [
          {
            kind: "conflict",
            id: "a".repeat(64),
            source: "projectPaths",
            index: 0,
            absoluteCandidate: "C:\\bundle\\projects\\alpha",
            instanceRelativeCandidate: "..\\projects\\beta",
            absoluteResolvedPath: "C:\\abs\\alpha",
            instanceRelativeResolvedPath: "C:\\rel\\beta",
            message: "backend message",
          },
        ],
      },
    );
    fake.resolve("get_settings", snapshot);
    const restore = ipc.__setTransportForTests(fake);
    try {
      const result = await ipc.SettingsAPI.get();
      // The structured report is returned verbatim: no error-string parsing, no
      // reshaping. AppSettings fields are flattened alongside it.
      expect(result.projectPaths).toEqual(["C:\\bundle\\projects\\alpha"]);
      expect(result.projectPathResolution.activeRegistrationCount).toBe(2);
      expect(result.projectPathResolution.issues).toHaveLength(1);
      expect(result.projectPathResolution.issues[0]).toMatchObject({
        kind: "conflict",
        source: "projectPaths",
        absoluteResolvedPath: "C:\\abs\\alpha",
        instanceRelativeResolvedPath: "C:\\rel\\beta",
      });
      expect(fake.lastCall("get_settings")?.args).toEqual({});
    } finally {
      restore();
    }
  });

  it("decodes selection hydration and events before invoking consumers", async () => {
    vi.resetModules();
    const ipc = await import("./ipc");
    const fake = new FakeTransport();
    const raw = liveSelection(SESSION_A);
    fake.resolve("get_active_session", raw);
    fake.setConnectionState({ state: "connected", generation: 3 });
    const restore = ipc.__setTransportForTests(fake);
    const callback = vi.fn();
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => undefined);
    try {
      const decoded = await ipc.SessionAPI.getSelection();
      expect(decoded).toEqual(raw);
      expect(decoded).not.toBe(raw);

      const unlisten = await ipc.onSessionSwitched(callback);
      fake.emitFromBackend("session_switched", { ...raw, displayable: false });
      expect(callback).not.toHaveBeenCalled();
      fake.emitFromBackend("session_switched", raw);
      expect(callback).toHaveBeenCalledWith(expect.objectContaining({ id: SESSION_A }), 3);
      unlisten();
    } finally {
      restore();
      errorSpy.mockRestore();
    }
  });

  it("rejects malformed hydration and exposes local connection snapshots", async () => {
    vi.resetModules();
    const ipc = await import("./ipc");
    const fake = new FakeTransport();
    fake.resolve("get_active_session", { id: SESSION_A });
    const restore = ipc.__setTransportForTests(fake);
    const states: unknown[] = [];
    try {
      await expect(ipc.SessionAPI.getSelection()).rejects.toThrow(/Invalid session selection/);
      expect(ipc.getTransportConnectionState()).toEqual({ state: "connected", generation: 0 });
      const unlisten = await ipc.onTransportConnectionState((state) => states.push(state));
      fake.setConnectionState({ state: "disconnected", generation: 2 });
      expect(states).toEqual([{ state: "disconnected", generation: 2 }]);
      unlisten();
    } finally {
      restore();
    }
  });

  it("classifies only the exact coordinator busy string", async () => {
    const ipc = await import("./ipc");
    expect(ipc.isSelectionCoordinatorBusyError("selectionCoordinatorBusy")).toBe(true);
    for (const error of [
      "selectionCoordinatorUnavailable",
      "selectionCoordinatorBusy ",
      { message: "selectionCoordinatorBusy" },
      new Error("selectionCoordinatorBusy"),
      null,
    ]) {
      expect(ipc.isSelectionCoordinatorBusyError(error)).toBe(false);
    }
  });

  it("sends exact create and update team objects, including explicit empty arrays", async () => {
    const ipc = await import("./ipc");
    const fake = new FakeTransport();
    fake.resolve("create_team", undefined);
    fake.resolve("update_team", undefined);
    const restore = ipc.__setTransportForTests(fake);
    try {
      await ipc.EntityAPI.createTeam(
        "C:\\Project",
        "dev-team",
        ["_agent_one"],
        "_agent_one",
        [{ url: "https://example.test/repo.git", agents: ["_agent_one"] }],
        [],
      );
      await ipc.EntityAPI.createTeam(
        "C:\\Project",
        "ops-team",
        ["_agent_two"],
        "_agent_two",
        [],
        [50, 75, 90],
      );
      await ipc.EntityAPI.updateTeam(
        "C:\\Project",
        "dev-team",
        ["_agent_one"],
        "_agent_one",
        [],
        [25],
      );
      await ipc.EntityAPI.updateTeam(
        "C:\\Project",
        "dev-team",
        ["_agent_one"],
        "_agent_one",
        [],
        [],
      );
    } finally {
      restore();
    }

    expect(fake.callsFor("create_team")).toEqual([
      {
        cmd: "create_team",
        args: {
          projectPath: "C:\\Project",
          name: "dev-team",
          agents: ["_agent_one"],
          coordinator: "_agent_one",
          repos: [{ url: "https://example.test/repo.git", agents: ["_agent_one"] }],
          contextAlertPercentages: [],
        },
      },
      {
        cmd: "create_team",
        args: {
          projectPath: "C:\\Project",
          name: "ops-team",
          agents: ["_agent_two"],
          coordinator: "_agent_two",
          repos: [],
          contextAlertPercentages: [50, 75, 90],
        },
      },
    ]);
    expect(fake.callsFor("update_team")).toEqual([
      {
        cmd: "update_team",
        args: {
          projectPath: "C:\\Project",
          teamName: "dev-team",
          agents: ["_agent_one"],
          coordinator: "_agent_one",
          repos: [],
          contextAlertPercentages: [25],
        },
      },
      {
        cmd: "update_team",
        args: {
          projectPath: "C:\\Project",
          teamName: "dev-team",
          agents: ["_agent_one"],
          coordinator: "_agent_one",
          repos: [],
          contextAlertPercentages: [],
        },
      },
    ]);
  });

  it("adapts only missing or undefined alert fields to fresh empty arrays", async () => {
    const ipc = await import("./ipc");
    const fake = new FakeTransport();
    const legacy = { agents: ["_agent_one"], coordinator: "_agent_one", repos: [] };
    fake.resolve("get_team_config", legacy);
    const restore = ipc.__setTransportForTests(fake);
    try {
      const first = await ipc.EntityAPI.getTeamConfig("C:\\Project", "dev-team");
      const second = await ipc.EntityAPI.getTeamConfig("C:\\Project", "dev-team");
      expect(first.contextAlertPercentages).toEqual([]);
      expect(first.contextAlertPercentages).not.toBe(second.contextAlertPercentages);
      expect(Object.prototype.hasOwnProperty.call(legacy, "contextAlertPercentages")).toBe(false);

      const explicitUndefined = {
        ...legacy,
        contextAlertPercentages: undefined,
      };
      fake.resolve("get_team_config", explicitUndefined);
      const decoded = await ipc.EntityAPI.getTeamConfig("C:\\Project", "dev-team");
      expect(decoded.contextAlertPercentages).toEqual([]);
      expect(explicitUndefined.contextAlertPercentages).toBeUndefined();
    } finally {
      restore();
    }

    expect(fake.callsFor("get_team_config")[0]?.args).toEqual({
      projectPath: "C:\\Project",
      teamName: "dev-team",
    });
  });

  it("preserves product-invalid numeric rows in order and deep-clones valid transport data", async () => {
    const ipc = await import("./ipc");
    const fake = new FakeTransport();
    const raw = {
      agents: ["_agent_one"],
      coordinator: "_agent_one",
      repos: [{ url: "https://example.test/repo.git", agents: ["_agent_one"] }],
      contextAlertPercentages: [101, 50.5, 50.5, 25],
    };
    fake.resolve("get_team_config", raw);
    const restore = ipc.__setTransportForTests(fake);
    try {
      const decoded = await ipc.EntityAPI.getTeamConfig("C:\\Project", "dev-team");
      expect(decoded).toEqual(raw);
      expect(decoded).not.toBe(raw);
      expect(decoded.agents).not.toBe(raw.agents);
      expect(decoded.repos).not.toBe(raw.repos);
      expect(decoded.repos[0]).not.toBe(raw.repos[0]);
      expect(decoded.repos[0]?.agents).not.toBe(raw.repos[0]?.agents);
      expect(decoded.contextAlertPercentages).not.toBe(raw.contextAlertPercentages);

      decoded.agents.push("_agent_two");
      decoded.repos[0]?.agents.push("_agent_two");
      decoded.contextAlertPercentages.reverse();
      expect(raw.agents).toEqual(["_agent_one"]);
      expect(raw.repos[0]?.agents).toEqual(["_agent_one"]);
      expect(raw.contextAlertPercentages).toEqual([101, 50.5, 50.5, 25]);
    } finally {
      restore();
    }
  });

  it("rejects present unrepresentable alert data with the exact decoder error", async () => {
    const ipc = await import("./ipc");
    const fake = new FakeTransport();
    const restore = ipc.__setTransportForTests(fake);
    const message =
      "Invalid get_team_config response: contextAlertPercentages must be an array of finite numbers";
    try {
      for (const contextAlertPercentages of [
        null,
        {},
        "50",
        [50, "75"],
        [Number.NaN],
        [Number.POSITIVE_INFINITY],
      ]) {
        fake.resolve("get_team_config", {
          agents: [],
          coordinator: "_agent_one",
          repos: [],
          contextAlertPercentages,
        });
        await expect(
          ipc.EntityAPI.getTeamConfig("C:\\Project", "dev-team"),
        ).rejects.toThrow(message);
      }
    } finally {
      restore();
    }
  });

  it("rejects malformed roots and legacy fields with their exact boundary errors", async () => {
    const ipc = await import("./ipc");
    const fake = new FakeTransport();
    const restore = ipc.__setTransportForTests(fake);
    const cases: { response: unknown; message: string }[] = [
      {
        response: null,
        message: "Invalid get_team_config response: expected an object",
      },
      {
        response: [],
        message: "Invalid get_team_config response: expected an object",
      },
      {
        response: { agents: [1], coordinator: "_agent_one", repos: [] },
        message: "Invalid get_team_config response: agents must be an array of strings",
      },
      {
        response: { agents: [], coordinator: null, repos: [] },
        message: "Invalid get_team_config response: coordinator must be a string",
      },
      {
        response: { agents: [], coordinator: "_agent_one", repos: {} },
        message:
          "Invalid get_team_config response: repos must be an array of { url: string; agents: string[] }",
      },
      {
        response: {
          agents: [],
          coordinator: "_agent_one",
          repos: [{ url: 7, agents: [] }],
        },
        message:
          "Invalid get_team_config response: repos must be an array of { url: string; agents: string[] }",
      },
      {
        response: {
          agents: [],
          coordinator: "_agent_one",
          repos: [{ url: "https://example.test/repo.git", agents: [7] }],
        },
        message:
          "Invalid get_team_config response: repos must be an array of { url: string; agents: string[] }",
      },
    ];
    try {
      for (const testCase of cases) {
        fake.resolve("get_team_config", testCase.response);
        await expect(
          ipc.EntityAPI.getTeamConfig("C:\\Project", "dev-team"),
        ).rejects.toThrow(testCase.message);
      }
    } finally {
      restore();
    }
  });
});

describe("agent-update ipc contract (#1551)", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    vi.resetModules();
  });

  const NODE = {
    command: "codex",
    label: "Codex",
    updateCommands: ["codex update"],
    installBefore: { status: "installed", version: "1.0", path: "C:\\bin\\codex.cmd", seq: 0 },
  };

  it("AgentUpdateAPI.getOverview invokes get_agent_update_overview with no args", async () => {
    const ipc = await import("./ipc");
    const fake = new FakeTransport();
    const rows = [
      {
        key: "codex",
        label: "Codex",
        command: "codex",
        color: "#10b981",
        updateCommands: ["codex update"],
        install: { status: "checking", seq: 0 },
      },
    ];
    fake.resolve("get_agent_update_overview", rows);
    const restore = ipc.__setTransportForTests(fake);
    try {
      await expect(ipc.AgentUpdateAPI.getOverview()).resolves.toEqual(rows);
    } finally {
      restore();
    }
    expect(fake.callsFor("get_agent_update_overview")).toEqual([
      { cmd: "get_agent_update_overview", args: {} },
    ]);
  });

  it("the per-command, skip and install listeners subscribe to the exact event names and pass the payload through", async () => {
    const ipc = await import("./ipc");
    const fake = new FakeTransport();
    const restore = ipc.__setTransportForTests(fake);
    try {
      const started = vi.fn();
      const skipped = vi.fn();
      const finished = vi.fn();
      const install = vi.fn();
      const unlisteners = await Promise.all([
        ipc.onAgentUpdateCommandStarted(started),
        ipc.onAgentUpdateCommandSkipped(skipped),
        ipc.onAgentUpdateCommandFinished(finished),
        ipc.onAgentInstallStateChanged(install),
      ]);
      expect(fake.listensFor("agent_update_command_started")).toHaveLength(1);
      expect(fake.listensFor("agent_update_command_skipped")).toHaveLength(1);
      expect(fake.listensFor("agent_update_command_finished")).toHaveLength(1);
      expect(fake.listensFor("agent_install_state_changed")).toHaveLength(1);

      fake.emitFromBackend("agent_update_command_started", NODE);
      expect(started).toHaveBeenCalledTimes(1);
      expect(started).toHaveBeenCalledWith(NODE);

      fake.emitFromBackend("agent_update_command_skipped", { command: "pi", label: "Pi" });
      expect(skipped).toHaveBeenCalledWith({ command: "pi", label: "Pi" });

      const result = { command: "codex", label: "Codex", ok: false, error: "exit code 1" };
      fake.emitFromBackend("agent_update_command_finished", result);
      expect(finished).toHaveBeenCalledWith(result);

      const changed = { command: "codex", install: { status: "missing", detail: "'codex' was not found on PATH", seq: 3 } };
      fake.emitFromBackend("agent_install_state_changed", changed);
      expect(install).toHaveBeenCalledWith(changed);

      for (const unlisten of unlisteners) unlisten();
      fake.emitFromBackend("agent_update_command_started", NODE);
      expect(started).toHaveBeenCalledTimes(1);
    } finally {
      restore();
    }
  });

  it("onAgentUpdatePromptClosed and onAgentUpdatesStarted pass the payload through and map an absent payload to null", async () => {
    const ipc = await import("./ipc");
    const fake = new FakeTransport();
    const restore = ipc.__setTransportForTests(fake);
    try {
      const closed = vi.fn();
      const started = vi.fn();
      await ipc.onAgentUpdatePromptClosed(closed);
      await ipc.onAgentUpdatesStarted(started);
      expect(fake.listensFor("agent_update_prompt_closed")).toHaveLength(1);
      expect(fake.listensFor("agent_updates_started")).toHaveLength(1);

      fake.emitFromBackend("agent_update_prompt_closed", { command: "claude", label: "Claude" });
      expect(closed).toHaveBeenLastCalledWith({ command: "claude", label: "Claude" });
      fake.emitFromBackend("agent_update_prompt_closed", null);
      expect(closed).toHaveBeenLastCalledWith(null);
      fake.emitFromBackend("agent_update_prompt_closed", undefined);
      expect(closed).toHaveBeenLastCalledWith(null);

      fake.emitFromBackend("agent_updates_started", { nodes: [NODE] });
      expect(started).toHaveBeenLastCalledWith({ nodes: [NODE] });
      fake.emitFromBackend("agent_updates_started", null);
      expect(started).toHaveBeenLastCalledWith(null);
      fake.emitFromBackend("agent_updates_started", undefined);
      expect(started).toHaveBeenLastCalledWith(null);
    } finally {
      restore();
    }
  });
});
