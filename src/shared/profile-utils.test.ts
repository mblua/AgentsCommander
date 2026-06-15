import { describe, expect, it } from "vitest";
import type { AgentConfig, CodingAgentProfilesConfig } from "./types";
import {
  commandExecutableBasename,
  executableBasename,
  expandAcRootPreview,
  hasAcRootPlaceholder,
  isAcAgentPath,
  isCodexAgent,
  isWgReplicaPath,
  parseArgvText,
  profileCellCommandText,
  profileDisplayLabel,
  resolveProfilePreview,
  sessionProfileBadge,
  stringifyArgv,
  validateEnvRows,
} from "./profile-utils";

function profiles(): CodingAgentProfilesConfig {
  return {
    schemaVersion: 2,
    profileSlots: {
      A: { label: "" },
      B: { label: "full power" },
      C: { label: "fast" },
    },
    defaultProfileByAgent: {},
    profilesByAgent: {
      codex: {
        A: { enabled: true, command: "", env: {}, notes: "" },
        C: { enabled: true, command: "codex --model gpt-5", env: {}, notes: "" },
      },
    },
  };
}

function agent(overrides: Partial<AgentConfig>): AgentConfig {
  return {
    id: "custom",
    label: "Custom",
    command: "custom",
    color: "#000",
    gitPullBefore: false,
    excludeGlobalClaudeMd: false,
    envs: [],
    isolatedHome: false,
    ...overrides,
  };
}

describe("profile utils", () => {
  it("formats custom profile names as letter-name labels", () => {
    expect(profileDisplayLabel(profiles(), "B")).toBe("B-FULL POWER");
    expect(profileDisplayLabel(profiles(), "A")).toBe("A");
  });

  it("previews fallback to the nearest lower available cell (v2 profileSlots/profilesByAgent)", () => {
    expect(resolveProfilePreview(profiles(), "codex", "B")).toMatchObject({
      requestedProfile: "B",
      effectiveProfile: "A",
      fallbackApplied: true,
      fallbackChain: ["B", "A"],
    });
    expect(resolveProfilePreview(profiles(), "codex", "C")).toMatchObject({
      requestedProfile: "C",
      effectiveProfile: "C",
      fallbackApplied: false,
    });
  });

  it("reads the full command string from a v2 profile cell", () => {
    expect(profileCellCommandText(profiles().profilesByAgent.codex.C)).toBe("codex --model gpt-5");
    expect(profileCellCommandText(null)).toBe("");
    expect(profileCellCommandText(undefined)).toBe("");
  });

  it("derives the binary basename from a full command string", () => {
    expect(commandExecutableBasename("codex --sandbox workspace-write --model gpt-5-codex")).toBe("codex");
    expect(commandExecutableBasename('"C:\\Program Files\\OpenAI Codex\\codex.exe" --yolo')).toBe("codex");
  });

  it("recognizes WG replica paths but not origin agents or repos", () => {
    expect(isWgReplicaPath("C:/repo/.ac/wg-7-dev-team/__agent_dev-webpage-ui")).toBe(true);
    expect(isWgReplicaPath("C:\\repo\\.ac\\wg-7-dev-team\\__agent_dev-webpage-ui")).toBe(true);
    // origin agent (single underscore, no wg- parent) is NOT a WG replica
    expect(isWgReplicaPath("C:/repo/.ac/_agent_architect")).toBe(false);
    expect(isWgReplicaPath("C:/repo/worktree")).toBe(false);
    expect(isWgReplicaPath(null)).toBe(false);
  });

  it("previews %AC_ROOT% expansion for display only", () => {
    expect(hasAcRootPlaceholder("%AC_ROOT%\\.codex")).toBe(true);
    expect(hasAcRootPlaceholder("D:\\manual\\codex")).toBe(false);
    expect(expandAcRootPreview("%AC_ROOT%\\.codex\\agents\\codex", "C:\\wg\\__agent_codex")).toBe(
      "C:\\wg\\__agent_codex\\.codex\\agents\\codex",
    );
    // No root context → returned unchanged (backend expands authoritatively at launch).
    expect(expandAcRootPreview("%AC_ROOT%\\.codex", null)).toBe("%AC_ROOT%\\.codex");
  });

  it("parses and stringifies argv text with quoted values", () => {
    const parsed = parseArgvText('--model "gpt 5" --config effort=high');
    expect(parsed).toEqual({
      argv: ["--model", "gpt 5", "--config", "effort=high"],
      error: null,
    });
    expect(stringifyArgv(parsed.argv)).toBe('--model "gpt 5" --config effort=high');
  });

  it("preserves Windows path backslashes in argv text", () => {
    expect(parseArgvText("--config C:\\Users\\maria\\codex.json")).toEqual({
      argv: ["--config", "C:\\Users\\maria\\codex.json"],
      error: null,
    });
    expect(
      parseArgvText('"C:\\Program Files\\Codex\\codex.exe" --yolo')
    ).toEqual({
      argv: ["C:\\Program Files\\Codex\\codex.exe", "--yolo"],
      error: null,
    });
  });

  it("parses escaped quotes without treating normal backslashes as escapes", () => {
    expect(parseArgvText('--name "say \\"hi\\""')).toEqual({
      argv: ["--name", 'say "hi"'],
      error: null,
    });
  });

  it("roundtrips stringified Windows argv values", () => {
    const argv = [
      "C:\\Program Files\\Codex\\codex.exe",
      "--config",
      "C:\\Users\\maria\\codex.json",
      "C:\\Program Files\\Codex\\",
    ];
    expect(parseArgvText(stringifyArgv(argv))).toEqual({ argv, error: null });
  });

  it("detects Codex from quoted Windows executable paths", () => {
    const command = '"C:\\Program Files\\Codex\\codex.exe" --yolo';
    expect(executableBasename(command)).toBe("codex");
    expect(isCodexAgent(agent({ command }))).toBe(true);
  });

  it("reports duplicate env keys case-insensitively for UI validation", () => {
    expect(
      validateEnvRows([
        { key: "CODEX_HOME", value: "x", source: "user", enabled: true },
        { key: "codex_home", value: "y", source: "user", enabled: true },
      ]),
    ).toContain("Duplicate");
  });

  it("only treats AgentsCommander agent directories as profile-persistable paths", () => {
    expect(isAcAgentPath("C:/repo/.ac/_agent_architect")).toBe(true);
    expect(isAcAgentPath("C:/repo/.ac/wg-7-dev-team/__agent_dev-webpage-ui")).toBe(true);
    expect(isAcAgentPath("C:/repo/worktree")).toBe(false);
  });

  it("formats session profile badges with fallback when applied", () => {
    expect(
      sessionProfileBadge({
        requestedProfile: "B",
        effectiveProfile: "A",
        profileFallbackApplied: true,
      }),
    ).toBe("B->A");
    expect(
      sessionProfileBadge({
        requestedProfile: "C",
        effectiveProfile: "C",
        profileFallbackApplied: false,
      }),
    ).toBe("C");
  });
});
