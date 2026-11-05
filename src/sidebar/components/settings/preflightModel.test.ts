import { describe, expect, it } from "vitest";
import {
  ALL_RUNNING_TEXT, NO_TARGETS_TEXT, activeLine, buildPreflightDraft, draftKey, formatDuration,
  isStale, isTimeoutKeyOfAgent, nextEnabledIndex, parseTimeoutInput, rowsForLetter,
  searchTargets, targetBasename, targetListText, targetProfileText,
  preflightFingerprint, stageStatusText, timeoutKey,
} from "./preflightModel";
import { baseSettings } from "../../../shared/testing/base-settings";
import type {
  PreflightStageReport, PreflightStageStatus, PreflightTestSnapshot, PreflightTestTarget,
} from "../../../shared/types";

function report(over: Partial<PreflightStageReport> = {}): PreflightStageReport {
  return {
    source: "codingAgent", label: "Coding Agent", effectiveProfile: "A", commandText: "x",
    timeoutSeconds: 60, status: "passed", exitCode: 0, durationMs: 5, stdout: "", stderr: "",
    stdoutTruncated: false, stderrTruncated: false, detail: null, cleanupDefect: null, ...over,
  };
}

function snapshot(over: Partial<PreflightTestSnapshot> = {}): PreflightTestSnapshot {
  return {
    runId: "r1", agentId: "codex", replicaPath: "C:/p", effectiveProfile: "A",
    settingsRevision: "rev", phase: "running", outcome: null, message: null, stages: [], ...over,
  };
}

describe("parseTimeoutInput (#3516)", () => {
  it.each([
    ["", { ok: true, value: undefined }],
    ["   ", { ok: true, value: undefined }],
    ["1", { ok: true, value: 1 }],
    ["4294967295", { ok: true, value: 4294967295 }],
    [" 7 ", { ok: true, value: 7 }],
    ["0", { ok: false }],
    ["4294967296", { ok: false }],
    ["1.5", { ok: false }],
    ["-3", { ok: false }],
    ["1e3", { ok: false }],
  ])("parses %j", (text, expected) => {
    expect(parseTimeoutInput(text)).toEqual(expected);
  });
});

describe("timeoutKey (#3516)", () => {
  it("keys the agent row and each cell, and scopes keys to their agent", () => {
    expect(timeoutKey("codex", null)).toBe("agent:codex");
    expect(timeoutKey("codex", "B")).toBe("cell:codex:B");
    expect(isTimeoutKeyOfAgent("agent:codex", "codex")).toBe(true);
    expect(isTimeoutKeyOfAgent("cell:codex:B", "codex")).toBe(true);
    expect(isTimeoutKeyOfAgent("cell:codex2:B", "codex")).toBe(false);
    expect(isTimeoutKeyOfAgent("agent:codex2", "codex")).toBe(false);
  });
});

describe("isStale (#3516)", () => {
  it("is stale on another revision, and the edited flag stays sticky after a revert", () => {
    const result = { settingsRevision: "rev" };
    expect(isStale(result, "rev", false)).toBe(false);
    expect(isStale(result, "other", false)).toBe(true);
    // The caller sets the flag on the first edit and keeps it after a revert.
    let edited = false;
    const settings = baseSettings();
    const base = preflightFingerprint(settings, "codex");
    const changed = { ...settings, defaultShellArgs: ["-NoLogo"] };
    for (const next of [changed, settings]) {
      if (preflightFingerprint(next, "codex") !== base) edited = true;
      expect(isStale(result, "rev", edited)).toBe(true);
    }
  });
});

describe("preflightFingerprint (#3516)", () => {
  const settings = baseSettings({
    agents: [{
      id: "codex", label: "Codex", command: "codex", color: "#000", envs: [],
      isolatedHome: false, preflight: "check",
    }],
  });

  it("ignores the theme and other agents' unrelated fields", () => {
    const base = preflightFingerprint(settings, "codex");
    expect(preflightFingerprint({ ...settings, theme: "light" } as typeof settings, "codex"))
      .toBe(base);
    const relabelled = { ...settings, agents: [{ ...settings.agents[0], label: "Other" }] };
    expect(preflightFingerprint(relabelled, "codex")).toBe(base);
  });

  it("changes with the agent preflight and its timeout", () => {
    const base = preflightFingerprint(settings, "codex");
    const edit = (over: object) => preflightFingerprint(
      { ...settings, agents: [{ ...settings.agents[0], ...over }] }, "codex");
    expect(edit({ preflight: "other" })).not.toBe(base);
    expect(edit({ preflightTimeoutSeconds: 9 })).not.toBe(base);
  });
});

describe("stageStatusText (#3516)", () => {
  const cases: [PreflightStageStatus, Partial<PreflightStageReport>, string[]][] = [
    ["pending", {}, ["Waiting"]],
    ["running", {}, ["Running…"]],
    ["passed", { exitCode: 0 }, ["Passed — exit code 0"]],
    ["failed", { exitCode: 3 }, ["Failed — exit code 3"]],
    ["spawnFailed", { exitCode: null, detail: "not found" },
      ["Command could not be started", "not found"]],
    ["timedOut", { exitCode: null }, ["Timed out"]],
    ["signaled", { exitCode: null }, ["Terminated by a signal"]],
    ["cancelled", { exitCode: null }, ["Cancelled"]],
    ["notRun", { exitCode: null }, ["Not run — previous stage failed"]],
  ];
  it.each(cases)("renders %s", (status, over, lines) => {
    expect(stageStatusText(report({ status, ...over }))).toEqual(lines);
  });

  it("adds the cleanup line and never renders a null exit code as 0", () => {
    expect(stageStatusText(report({ status: "passed", cleanupDefect: "kill failed" })))
      .toEqual(["Passed — exit code 0", "Cleanup reported an error: kill failed"]);
    const failed = stageStatusText(report({ status: "failed", exitCode: null }));
    expect(failed.join(" ")).not.toContain("0");
  });
});

describe("activeLine (#3516)", () => {
  it("uses the stage source, not its label", () => {
    const stages = [report({ source: "profile", label: "Profile A", status: "running" })];
    expect(activeLine(snapshot({ stages }), false)).toBe("Running Profile preflight…");
    const agent = [report({ status: "running" })];
    expect(activeLine(snapshot({ stages: agent }), false)).toBe("Running Coding Agent preflight…");
  });

  it("follows the stated order", () => {
    const running = [report({ status: "running" })];
    expect(activeLine(snapshot({ stages: running }), true)).toBe("Cancelling…");
    expect(activeLine(snapshot({ stages: running, message: "Cancelling…" }), false))
      .toBe("Cancelling…");
    expect(activeLine(snapshot({ phase: "preparing", stages: running }), false))
      .toBe("Preparing test context…");
    const between = [
      report({ status: "passed" }), report({ source: "profile", status: "pending" }),
    ];
    expect(activeLine(snapshot({ stages: between }), false))
      .toBe("Running Coding Agent preflight…");
    const none = [report({ status: "pending" })];
    expect(activeLine(snapshot({ stages: none }), false)).toBe("Preparing test context…");
    expect(activeLine(snapshot({ phase: "finished", outcome: "passed" }), false)).toBeNull();
  });
});

function target(path: string, over: Partial<PreflightTestTarget> = {}): PreflightTestTarget {
  return {
    replicaPath: path, requestedProfile: "A", effectiveProfile: "A", fallbackApplied: false,
    running: false, ...over,
  };
}

describe("buildPreflightDraft (#3832)", () => {
  it("keeps only preflight commands and timeouts, with null for undefined", () => {
    const settings = baseSettings({
      agents: [{
        id: "codex", label: "Codex", command: "codex", color: "#000", envs: [],
        isolatedHome: false, preflight: "agent-check", preflightTimeoutSeconds: 5,
      }],
    });
    settings.codingAgentProfiles.profilesByAgent.codex = {
      B: { enabled: false, command: "b", env: { K: "v" }, notes: "", preflight: "b-check" },
      A: { enabled: true, command: "a", env: {}, notes: "", preflightTimeoutSeconds: 9 },
    };
    const draft = buildPreflightDraft(settings, "codex");
    expect(draft).toEqual({
      codingAgent: { command: "agent-check", timeoutSeconds: 5 },
      profiles: {
        A: { command: null, timeoutSeconds: 9 },
        B: { command: "b-check", timeoutSeconds: null },
      },
    });
    expect(Object.keys(draft.profiles)).toEqual(["A", "B"]);
    expect(draftKey(draft)).toBe(draftKey(buildPreflightDraft(settings, "codex")));
  });

  it("uses null stages for an unknown agent", () => {
    expect(buildPreflightDraft(baseSettings(), "nope")).toEqual({
      codingAgent: { command: null, timeoutSeconds: null }, profiles: {},
    });
  });
});

describe("picker helpers (#3832)", () => {
  it.each([
    ["C:\\p\\.ac\\room\\__agent_dev", "__agent_dev"],
    ["/home/p/.ac/room/__agent_qa/", "__agent_qa"],
    ["C:/p\\mixed/__agent_ops", "__agent_ops"],
    ["solo", "solo"],
  ])("basename of %s", (path, name) => {
    expect(targetBasename(path)).toBe(name);
  });

  it("labels the launch Profile, or the fallback in its place", () => {
    expect(targetProfileText(target("x", { effectiveProfile: "B" })))
      .toBe("Launches with Profile B");
    expect(targetProfileText(target("x", {
      requestedProfile: "C", effectiveProfile: "A", fallbackApplied: true,
    }))).toBe("Asks for Profile C, launches with Profile A.");
  });

  it("filters by launch Profile after fallback; card mode keeps every row", () => {
    const rows = [
      target("a"), target("b", { effectiveProfile: "B" }),
      target("c", { requestedProfile: "B", effectiveProfile: "A", fallbackApplied: true }),
    ];
    expect(rowsForLetter(rows, "A").map((r) => r.replicaPath)).toEqual(["a", "c"]);
    expect(rowsForLetter(rows, "B").map((r) => r.replicaPath)).toEqual(["b"]);
    expect(rowsForLetter(rows, null)).toHaveLength(3);
  });

  it("searches the complete path case-insensitively", () => {
    const rows = [target("C:\\P\\Room\\__agent_Dev"), target("c:/p/room/__agent_qa")];
    expect(searchTargets(rows, "  ROOM\\__AGENT_d ")).toEqual([rows[0]]);
    expect(searchTargets(rows, "p/room")).toEqual([rows[1]]);
    expect(searchTargets(rows, "")).toEqual(rows);
    expect(searchTargets(rows, "zzz")).toEqual([]);
  });

  it("steps over running rows and clamps at both ends", () => {
    const rows = [target("a", { running: true }), target("b"), target("c", { running: true }),
      target("d")];
    expect(nextEnabledIndex(rows, -1, 1)).toBe(1);
    expect(nextEnabledIndex(rows, -1, -1)).toBe(1);
    expect(nextEnabledIndex(rows, 1, 1)).toBe(3);
    expect(nextEnabledIndex(rows, 3, 1)).toBe(3);
    expect(nextEnabledIndex(rows, 3, -1)).toBe(1);
    expect(nextEnabledIndex(rows, 1, -1)).toBe(1);
    expect(nextEnabledIndex([target("x", { running: true })], -1, 1)).toBe(-1);
    expect(nextEnabledIndex([], -1, 1)).toBe(-1);
  });

  it("explains empty and all-running lists", () => {
    expect(targetListText([], null)).toBe(NO_TARGETS_TEXT);
    expect(targetListText([], "B")).toBe(
      "No agent launches with Profile B. Assign Profile B to an agent to test it here.");
    expect(targetListText([target("a", { running: true })], "A")).toBe(ALL_RUNNING_TEXT);
    expect(targetListText([target("a", { running: true }), target("b")], "A")).toBeNull();
  });
});

describe("formatDuration (#3516)", () => {
  it.each([[0, "0 ms"], [999, "999 ms"], [1000, "1.0 s"], [1250, "1.3 s"], [61000, "61.0 s"]])(
    "formats %d", (ms, text) => expect(formatDuration(ms)).toBe(text));
});
