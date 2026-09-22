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

  it("#2306: overlayOwnsAgents survives the get_settings transport", async () => {
    const ipc = await import("./ipc");
    const fake = new FakeTransport();
    fake.resolve("get_settings", { ...settingsSnapshot(), overlayOwnsAgents: true });
    const restore = ipc.__setTransportForTests(fake);
    try {
      const result = await ipc.SettingsAPI.get();
      expect(result.overlayOwnsAgents).toBe(true);
    } finally {
      restore();
    }
  });

  it("#2306: moveCodingAgent invokes the narrow command with the exact payload", async () => {
    const ipc = await import("./ipc");
    const fake = new FakeTransport();
    fake.resolve("move_coding_agent", ["claude", "codex"]);
    const restore = ipc.__setTransportForTests(fake);
    try {
      await expect(
        ipc.SettingsAPI.moveCodingAgent({ id: "codex", neighborId: "claude", direction: "up" }),
      ).resolves.toEqual(["claude", "codex"]);
      // The invoke name and camelCase keys are the P2 contract; nothing else
      // travels with the move and the result is the authoritative id order.
      expect(fake.lastCall("move_coding_agent")?.args).toEqual({
        id: "codex",
        neighborId: "claude",
        direction: "up",
      });
    } finally {
      restore();
    }
  });

  it("#2306: derives and enforces the exact requested adjacent move order", async () => {
    const ipc = await import("./ipc");
    expect(ipc.expectedCodingAgentMoveOrder(["a", "b", "c", "d", "e"], "c", "up")).toEqual([
      "a",
      "c",
      "b",
      "d",
      "e",
    ]);
    expect(ipc.expectedCodingAgentMoveOrder(["a", "b", "c", "d", "e"], "c", "down")).toEqual([
      "a",
      "b",
      "d",
      "c",
      "e",
    ]);
    // A boundary or unknown id leaves the vector untouched for the caller's guard.
    expect(ipc.expectedCodingAgentMoveOrder(["a", "b"], "a", "up")).toEqual(["a", "b"]);
    // Same length and same ids is not enough: only the exact swap is accepted.
    expect(() =>
      ipc.assertCodingAgentMoveOrder(["a", "b", "c", "d", "e"], ["a", "c", "b", "d", "e"]),
    ).toThrow("unexpected agent order");
    expect(() =>
      ipc.assertCodingAgentMoveOrder(["a", "c", "b", "d", "e"], ["a", "c", "b", "d", "e"]),
    ).not.toThrow();
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

  // #2222 boundary pin: this is the only test that loads the real ipc module
  // for the new event. The listener suites swap `../shared/ipc` for a mocked
  // factory that never sees an event string, so F-T1..F-T4 stay green even if
  // this literal is misspelled here.
  it("binds onSessionViewRequested to the exact session_view_requested event name", async () => {
    const ipc = await import("./ipc");
    const fake = new FakeTransport();
    const restore = ipc.__setTransportForTests(fake);
    try {
      const viewRequested = vi.fn();
      const unlisten = await ipc.onSessionViewRequested(viewRequested);
      expect(fake.listensFor("session_view_requested")).toHaveLength(1);

      fake.emitFromBackend("session_view_requested", { id: SESSION_A });
      expect(viewRequested).toHaveBeenCalledTimes(1);
      expect(viewRequested).toHaveBeenCalledWith({ id: SESSION_A });

      unlisten();
      fake.emitFromBackend("session_view_requested", { id: SESSION_A });
      expect(viewRequested).toHaveBeenCalledTimes(1);
    } finally {
      restore();
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

describe("selection-lock transport contract (#1942)", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
    vi.resetModules();
  });

  function legacyTarget(overrides: Record<string, unknown> = {}): Record<string, unknown> {
    return {
      workgroupName: "wg-12-ac-dev-team-v4",
      workgroupPath: "C:\\proj\\.ac\\wg-12",
      replicaName: "dev-webpage-ui-v4",
      replicaPath: "C:\\proj\\.ac\\wg-12\\__agent_dev-webpage-ui-v4",
      identityPath: "C:\\proj\\.ac\\wg-12\\__agent_dev-webpage-ui-v4\\identity.json",
      originProject: "proj",
      liveSessionIds: ["sess-1"],
      ...overrides,
    };
  }

  it("sends both removal operations and both default operations as exact wire names inside {request}", async () => {
    const ipc = await import("./ipc");
    const fake = new FakeTransport();
    fake.resolve("preview_selection_lock_removal", {
      scope: "replica",
      targetFingerprint: "fp-removal-preview",
      candidateCount: 1,
      countsComplete: true,
      protectedCount: 1,
      alreadyUnlockedCount: 0,
      invalidCount: 0,
      targets: [],
      warnings: [],
    });
    fake.resolve("apply_selection_lock_removal", {
      scope: "replica",
      targetFingerprint: "fp-removal-preview",
      removedCount: 1,
      removedReplicaPaths: ["C:\\replica"],
      alreadyUnlockedPaths: [],
      failedReplicaPaths: [],
      remainingProtectedCount: 0,
      candidateCount: 1,
      countsComplete: true,
      invalidCount: 0,
      errors: [],
      warnings: [],
    });
    fake.resolve("get_replica_selection_default", {
      targetReplicaPath: "C:\\replica",
      matrixPath: "C:\\matrix",
      default: null,
      defaultFingerprint: "fp-default",
      warnings: [],
    });
    fake.resolve("set_replica_selection_default", {
      targetReplicaPath: "C:\\replica",
      matrixPath: "C:\\matrix",
      default: {
        codingAgentId: "codex",
        requestedProfile: "fast",
        selectionLocked: true,
      },
      defaultFingerprint: "fp-default-set",
      warnings: [],
    });
    const restore = ipc.__setTransportForTests(fake);
    try {
      const previewRemoval = {
        targetReplicaPath: "C:\\replica",
        scope: "replica",
      } as const;
      const applyRemoval = {
        targetReplicaPath: "C:\\replica",
        scope: "kind",
        confirmedTargetFingerprint: "fp-removal-preview",
      } as const;
      const getDefault = { targetReplicaPath: "C:\\replica" } as const;
      const setDefault = {
        targetReplicaPath: "C:\\replica",
        codingAgentId: "codex",
        requestedProfile: "fast",
        selectionLocked: true,
        confirmedDefaultFingerprint: "fp-default-cas",
      } as const;

      await ipc.SettingsAPI.previewSelectionLockRemoval(previewRemoval);
      await ipc.SettingsAPI.applySelectionLockRemoval(applyRemoval);
      await ipc.SettingsAPI.getReplicaSelectionDefault(getDefault);
      const setResult = await ipc.SettingsAPI.setReplicaSelectionDefault(setDefault);

      expect(fake.callsFor("preview_selection_lock_removal")).toEqual([
        { cmd: "preview_selection_lock_removal", args: { request: previewRemoval } },
      ]);
      expect(fake.callsFor("apply_selection_lock_removal")).toEqual([
        { cmd: "apply_selection_lock_removal", args: { request: applyRemoval } },
      ]);
      expect(fake.callsFor("get_replica_selection_default")).toEqual([
        { cmd: "get_replica_selection_default", args: { request: getDefault } },
      ]);
      expect(fake.callsFor("set_replica_selection_default")).toEqual([
        { cmd: "set_replica_selection_default", args: { request: setDefault } },
      ]);
      // The confirmed default fingerprint crosses verbatim; the backend owns the CAS.
      expect(fake.lastCall("set_replica_selection_default")?.args).toEqual({
        request: { ...setDefault },
      });
      expect(setResult.default).toEqual({
        codingAgentId: "codex",
        requestedProfile: "fast",
        selectionLocked: true,
      });
      expect(setResult.defaultFingerprint).toBe("fp-default-set");
    } finally {
      restore();
    }
  });

  it("keeps the legacy ordinary assignment payload unchanged and never manufactures new fields", async () => {
    const ipc = await import("./ipc");
    const fake = new FakeTransport();
    const bareTarget = legacyTarget();
    fake.resolve("preview_coding_agent_profile_selection", {
      scope: "replica",
      targetCount: 1,
      liveSessionCount: 1,
      targetFingerprint: "fp-legacy",
      requiresExplicitConfirmation: false,
      targets: [bareTarget],
      warnings: [],
    });
    fake.resolve("apply_coding_agent_profile_selection", {
      scope: "replica",
      updatedCount: 1,
      restartedCount: 0,
      updatedReplicaPaths: ["C:\\replica"],
      restartedSessionIds: [],
      destroyedButNotRecreatedSessionIds: [],
      targetFingerprint: "fp-legacy",
      warnings: [],
      errors: [],
    });
    const restore = ipc.__setTransportForTests(fake);
    try {
      const previewRequest = {
        targetReplicaPath: "C:\\replica",
        codingAgentId: "codex",
        profile: "fast",
        scope: "replica",
        restartSessions: false,
      } as const;
      const applyRequest = {
        targetReplicaPath: "C:\\replica",
        codingAgentId: "codex",
        profile: "fast",
        scope: "replica",
        restartSessions: true,
        confirmedTargetFingerprint: "fp-legacy",
        typedConfirmation: "dev-webpage-ui-v4",
      } as const;

      const preview =
        await ipc.SettingsAPI.previewCodingAgentProfileSelection(previewRequest);
      await ipc.SettingsAPI.applyCodingAgentProfileSelection(applyRequest);

      const previewArgs = fake.lastCall("preview_coding_agent_profile_selection")?.args;
      const applyArgs = fake.lastCall("apply_coding_agent_profile_selection")?.args;
      expect(previewArgs).toEqual({ request: previewRequest });
      expect(applyArgs).toEqual({ request: applyRequest });
      expect(Object.keys(previewArgs?.request as Record<string, unknown>)).toEqual([
        "targetReplicaPath",
        "codingAgentId",
        "profile",
        "scope",
        "restartSessions",
      ]);
      expect(Object.keys(applyArgs?.request as Record<string, unknown>)).toEqual([
        "targetReplicaPath",
        "codingAgentId",
        "profile",
        "scope",
        "restartSessions",
        "confirmedTargetFingerprint",
        "typedConfirmation",
      ]);

      // An older backend's target has no new fields; they stay absent, never
      // normalized to "unlocked" or a synthesized pair.
      expect(preview.targets[0]).toEqual(bareTarget);
      expect("savedPair" in preview.targets[0]).toBe(false);
      expect("selectionState" in preview.targets[0]).toBe(false);
      expect("selectionError" in preview.targets[0]).toBe(false);
    } finally {
      restore();
    }
  });

  it("forwards the reviewed decision and its fingerprint without recomputation", async () => {
    const ipc = await import("./ipc");
    const fake = new FakeTransport();
    const decisions = {
      unlockedOnly: {
        fingerprint: "fp-decision-unlocked",
        eligiblePaths: ["C:\\a"],
        eligibleCount: 1,
        skippedLockedCount: 1,
        liveSessionCount: 1,
      },
      forceReviewed: {
        fingerprint: "fp-decision-force",
        eligiblePaths: ["C:\\a", "C:\\b"],
        eligibleCount: 2,
        skippedLockedCount: 0,
        liveSessionCount: 1,
      },
    };
    fake.resolve("preview_coding_agent_profile_selection", {
      scope: "kind",
      targetCount: 2,
      liveSessionCount: 1,
      targetFingerprint: "fp-top",
      requiresExplicitConfirmation: true,
      targets: [],
      warnings: [],
      countsComplete: true,
      candidateCount: 2,
      protectedCount: 1,
      invalidCount: 0,
      conflictCount: 1,
      decisions,
    });
    fake.resolve("apply_coding_agent_profile_selection", {
      scope: "kind",
      updatedCount: 2,
      restartedCount: 1,
      updatedReplicaPaths: ["C:\\a", "C:\\b"],
      restartedSessionIds: ["sess-1"],
      destroyedButNotRecreatedSessionIds: [],
      targetFingerprint: "fp-top",
      warnings: [],
      errors: [],
      newlyProtectedPaths: ["C:\\a", "C:\\b"],
      skippedLockedPaths: [],
      invalidPaths: [],
      lockedAfterApplyCount: 2,
      forceApplied: true,
    });
    const restore = ipc.__setTransportForTests(fake);
    try {
      const preview = await ipc.SettingsAPI.previewCodingAgentProfileSelection({
        targetReplicaPath: "C:\\replica",
        codingAgentId: "codex",
        profile: "fast",
        scope: "kind",
        restartSessions: false,
        assignmentMode: "assignAndLock",
      });
      expect(preview.decisions?.unlockedOnly.fingerprint).toBe("fp-decision-unlocked");
      expect(preview.decisions?.forceReviewed.fingerprint).toBe("fp-decision-force");

      const applyRequest = {
        targetReplicaPath: "C:\\replica",
        codingAgentId: "codex",
        profile: "fast",
        scope: "kind",
        restartSessions: true,
        confirmedTargetFingerprint: "fp-decision-force",
        assignmentMode: "assignAndLock",
        conflictDecision: "forceReviewed",
      } as const;
      const applied =
        await ipc.SettingsAPI.applyCodingAgentProfileSelection(applyRequest);
      expect(fake.lastCall("apply_coding_agent_profile_selection")?.args).toEqual({
        request: applyRequest,
      });
      const forwarded = fake.lastCall("apply_coding_agent_profile_selection")?.args
        .request as Record<string, unknown>;
      expect(forwarded.confirmedTargetFingerprint).toBe("fp-decision-force");
      expect(forwarded.conflictDecision).toBe("forceReviewed");
      expect(applied.forceApplied).toBe(true);
    } finally {
      restore();
    }
  });

  it("propagates errors and preserves null/unknown diagnostics without synthesizing state", async () => {
    const ipc = await import("./ipc");
    const fake = new FakeTransport();
    const restore = ipc.__setTransportForTests(fake);
    try {
      fake.reject("apply_selection_lock_removal", "stalePreview");
      await expect(
        ipc.SettingsAPI.applySelectionLockRemoval({
          targetReplicaPath: "C:\\replica",
          scope: "workgroup",
          confirmedTargetFingerprint: "fp",
        }),
      ).rejects.toBe("stalePreview");

      fake.reject("get_replica_selection_default", "configLockTimeout");
      await expect(
        ipc.SettingsAPI.getReplicaSelectionDefault({
          targetReplicaPath: "C:\\replica",
        }),
      ).rejects.toBe("configLockTimeout");

      const bare = legacyTarget();
      const unknownState = legacyTarget({
        selectionState: "unsupported-by-this-build",
        selectionError: "identity unreadable",
      });
      const locked = legacyTarget({
        savedPair: { codingAgentId: "codex", requestedProfile: "fast" },
        selectionState: "locked",
        selectionError: null,
      });
      fake.resolve("preview_selection_lock_removal", {
        scope: "kind",
        targetFingerprint: "fp-removal",
        candidateCount: 3,
        countsComplete: false,
        protectedCount: 1,
        alreadyUnlockedCount: 1,
        invalidCount: 1,
        targets: [bare, unknownState, locked],
        warnings: ["directory walk incomplete"],
      });
      const preview = await ipc.SettingsAPI.previewSelectionLockRemoval({
        targetReplicaPath: "C:\\replica",
        scope: "kind",
      });
      expect(preview.countsComplete).toBe(false);
      expect(preview.targets[0]).toEqual(bare);
      expect("selectionState" in preview.targets[0]).toBe(false);
      expect(preview.targets[1]).toMatchObject({
        selectionState: "unsupported-by-this-build",
        selectionError: "identity unreadable",
      });
      expect(preview.targets[1]?.selectionState).not.toBe("unlocked");
      expect(preview.targets[2]).toEqual(locked);
      expect(preview.warnings).toEqual(["directory walk incomplete"]);

      fake.resolve("apply_selection_lock_removal", {
        scope: "kind",
        targetFingerprint: "fp-removal",
        removedCount: 1,
        removedReplicaPaths: ["C:\\replica"],
        alreadyUnlockedPaths: ["C:\\other"],
        failedReplicaPaths: [],
        remainingProtectedCount: null,
        candidateCount: 3,
        countsComplete: false,
        invalidCount: 1,
        errors: [
          {
            code: "invalidSelectionState",
            message: "identity unreadable",
            sessionIds: [],
            replicaPaths: ["C:\\broken"],
          },
        ],
        warnings: [],
      });
      const applied = await ipc.SettingsAPI.applySelectionLockRemoval({
        targetReplicaPath: "C:\\replica",
        scope: "kind",
        confirmedTargetFingerprint: "fp-removal",
      });
      // Unknown candidates keep `null`; never a fabricated zero.
      expect(applied.remainingProtectedCount).toBeNull();
      expect(applied.errors[0]).toEqual({
        code: "invalidSelectionState",
        message: "identity unreadable",
        sessionIds: [],
        replicaPaths: ["C:\\broken"],
      });

      fake.resolve("get_replica_selection_default", {
        targetReplicaPath: "C:\\replica",
        matrixPath: "C:\\matrix",
        default: null,
        defaultFingerprint: "fp-default",
        warnings: ["malformed default is not reported absent"],
      });
      const fetched = await ipc.SettingsAPI.getReplicaSelectionDefault({
        targetReplicaPath: "C:\\replica",
      });
      expect(fetched.default).toBeNull();
      expect(fetched.defaultFingerprint).toBe("fp-default");
      expect(fetched.warnings).toEqual(["malformed default is not reported absent"]);
    } finally {
      restore();
    }
  });
});

describe("loop ipc payload contract (#2289)", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    vi.resetModules();
  });

  it("pins create and update request payloads in both directions", async () => {
    const ipc = await import("./ipc");
    const fake = new FakeTransport();
    fake.resolve("create_loop", undefined);
    fake.resolve("update_loop", undefined);
    const restore = ipc.__setTransportForTests(fake);
    try {
      await ipc.LoopAPI.create("C:\\Project", {
        id: "nightly-audit",
        name: "Nightly audit",
        expr: "0 3 * * *",
        workgroup: "wg-17-dev-team",
        promptBody: "Audit the repository",
        busyCoordinator: "forceInject",
        sessionStart: "accumulate",
        enabled: false,
      });
      await ipc.LoopAPI.create("C:\\Project", {
        name: "Daily standup",
        expr: "0 9 * * 1-5",
        workgroup: "wg-17-dev-team",
        promptBody: "Post the standup digest",
      });
      await ipc.LoopAPI.update("C:\\Project", "nightly-audit", {
        sessionStart: "fresh",
        enabled: true,
      });
      await ipc.LoopAPI.update("C:\\Project", "weekly-audit", {
        name: "Weekly audit renamed",
        expr: "0 4 * * 1",
        workgroup: "wg-18-dev-team",
        promptBody: "Audit the repository weekly",
        busyCoordinator: "forceInject",
        sessionStart: "accumulate",
        enabled: false,
      });
      await ipc.LoopAPI.update("C:\\Project", "legacy-standup", {});
    } finally {
      restore();
    }
    expect(fake.callsFor("create_loop")).toEqual([
      {
        cmd: "create_loop",
        args: {
          request: {
            projectPath: "C:\\Project",
            id: "nightly-audit",
            name: "Nightly audit",
            expr: "0 3 * * *",
            workgroup: "wg-17-dev-team",
            promptBody: "Audit the repository",
            busyCoordinator: "forceInject",
            sessionStart: "accumulate",
            enabled: false,
          },
        },
      },
      {
        cmd: "create_loop",
        args: {
          request: {
            projectPath: "C:\\Project",
            id: null,
            name: "Daily standup",
            expr: "0 9 * * 1-5",
            workgroup: "wg-17-dev-team",
            promptBody: "Post the standup digest",
            busyCoordinator: null,
            sessionStart: null,
            enabled: null,
          },
        },
      },
    ]);
    expect(fake.callsFor("update_loop")).toEqual([
      {
        cmd: "update_loop",
        args: {
          request: {
            projectPath: "C:\\Project",
            id: "nightly-audit",
            name: null,
            expr: null,
            workgroup: null,
            promptBody: null,
            busyCoordinator: null,
            sessionStart: "fresh",
            enabled: true,
          },
        },
      },
      {
        cmd: "update_loop",
        args: {
          request: {
            projectPath: "C:\\Project",
            id: "weekly-audit",
            name: "Weekly audit renamed",
            expr: "0 4 * * 1",
            workgroup: "wg-18-dev-team",
            promptBody: "Audit the repository weekly",
            busyCoordinator: "forceInject",
            sessionStart: "accumulate",
            enabled: false,
          },
        },
      },
      {
        cmd: "update_loop",
        args: {
          request: {
            projectPath: "C:\\Project",
            id: "legacy-standup",
            name: null,
            expr: null,
            workgroup: null,
            promptBody: null,
            busyCoordinator: null,
            sessionStart: null,
            enabled: null,
          },
        },
      },
    ]);
  });
});
