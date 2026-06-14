import { describe, expect, it } from "vitest";
import type { CodingAgentProfilesConfig } from "./types";
import {
  parseArgvText,
  profileDisplayLabel,
  resolveProfilePreview,
  isAcAgentPath,
  sessionProfileBadge,
  stringifyArgv,
  validateEnvRows,
} from "./profile-utils";

function profiles(): CodingAgentProfilesConfig {
  return {
    schemaVersion: 1,
    letters: {
      A: { name: "" },
      B: { name: "full power" },
      C: { name: "fast" },
    },
    agentDefaults: {},
    matrix: {
      codex: {
        A: { enabled: true, argv: [], env: {}, notes: "" },
        C: { enabled: true, argv: ["--model", "gpt-5"], env: {}, notes: "" },
      },
    },
  };
}

describe("profile utils", () => {
  it("formats custom profile names as letter-name labels", () => {
    expect(profileDisplayLabel(profiles(), "B")).toBe("B-FULL POWER");
    expect(profileDisplayLabel(profiles(), "A")).toBe("A");
  });

  it("previews fallback to the nearest lower available cell", () => {
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

  it("parses and stringifies argv text with quoted values", () => {
    const parsed = parseArgvText('--model "gpt 5" --config effort=high');
    expect(parsed).toEqual({
      argv: ["--model", "gpt 5", "--config", "effort=high"],
      error: null,
    });
    expect(stringifyArgv(parsed.argv)).toBe('--model "gpt 5" --config effort=high');
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
