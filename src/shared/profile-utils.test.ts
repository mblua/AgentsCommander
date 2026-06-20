import { describe, expect, it } from "vitest";
import type { AgentConfig, CodingAgentProfilesConfig } from "./types";
import {
  commandExecutableBasename,
  defaultInstructionsFilename,
  effectiveEnvProjection,
  executableBasename,
  expandAcRootPreview,
  hasAcRootPlaceholder,
  isAcAgentPath,
  isCodexAgent,
  isWgReplicaPath,
  parseArgvText,
  profileBadgeKind,
  profileCellCommandText,
  profileConfiguredElsewhere,
  profileDisplayLabel,
  profileEnvOrigin,
  resolveProfilePreview,
  sessionProfileBadge,
  shouldOfferRestartAfterAssign,
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

  it("derives the default instructions filename with parity to the Rust resolver (#529, G2)", () => {
    // Claude family → CLAUDE.md, including wrapped/suffixed/absolute shapes and
    // — critically — commands carrying trailing flags (the all-token scan, not
    // first/last token, is what gives parity with the Rust detector here).
    expect(defaultInstructionsFilename("claude")).toBe("CLAUDE.md");
    expect(defaultInstructionsFilename("claude --model sonnet")).toBe("CLAUDE.md");
    expect(defaultInstructionsFilename("cmd.exe /c claude")).toBe("CLAUDE.md");
    expect(defaultInstructionsFilename("cmd.exe /c claude --continue")).toBe("CLAUDE.md");
    expect(defaultInstructionsFilename("claude-mb")).toBe("CLAUDE.md");
    expect(defaultInstructionsFilename("C:\\tools\\claude.exe")).toBe("CLAUDE.md");
    // Gemini → GEMINI.md, with and without flags.
    expect(defaultInstructionsFilename("gemini")).toBe("GEMINI.md");
    expect(defaultInstructionsFilename("gemini --yolo")).toBe("GEMINI.md");
    // Codex, OpenCode, custom, and empty all fall to AGENTS.md.
    expect(defaultInstructionsFilename("codex")).toBe("AGENTS.md");
    expect(defaultInstructionsFilename("codex --sandbox workspace-write")).toBe("AGENTS.md");
    expect(defaultInstructionsFilename("opencode")).toBe("AGENTS.md");
    expect(defaultInstructionsFilename("my-agent-cli --flag")).toBe("AGENTS.md");
    expect(defaultInstructionsFilename("")).toBe("AGENTS.md");
    // Codex precedence over a later gemini token (mirrors Rust claude>codex>gemini).
    expect(defaultInstructionsFilename("codex --base gemini")).toBe("AGENTS.md");
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

  it("offers a post-assign restart only for a replica-scope assign onto a live WG session (#537)", () => {
    const wgPath = "C:/repo/.ac/wg-7-dev-team/__agent_dev-webpage-ui";
    const replica = (restartSessions = false) => ({ scope: "replica" as const, restartSessions });

    // Live PTY (string status) on a WG replica, replica scope, no prior restart -> prompt.
    expect(shouldOfferRestartAfterAssign(replica(), { status: "running", workingDirectory: wgPath })).toBe(true);
    expect(shouldOfferRestartAfterAssign(replica(), { status: "idle", workingDirectory: wgPath })).toBe(true);
    expect(shouldOfferRestartAfterAssign(replica(), { status: "active", workingDirectory: wgPath })).toBe(true);

    // Exited session ({ exited }) has nothing to restart.
    expect(shouldOfferRestartAfterAssign(replica(), { status: { exited: 0 }, workingDirectory: wgPath })).toBe(false);
    // No session at all.
    expect(shouldOfferRestartAfterAssign(replica(), undefined)).toBe(false);
    // The picker already restarted via the toggle -> no second prompt.
    expect(shouldOfferRestartAfterAssign(replica(true), { status: "running", workingDirectory: wgPath })).toBe(false);
    // Non-WG path (origin agent / repo) restarts directly, never via this prompt.
    expect(shouldOfferRestartAfterAssign(replica(), { status: "running", workingDirectory: "C:/repo/.ac/_agent_architect" })).toBe(false);
    // Broad scopes never prompt here (kept on the in-modal toggle).
    expect(shouldOfferRestartAfterAssign({ scope: "kind", restartSessions: false }, { status: "running", workingDirectory: wgPath })).toBe(false);
    expect(shouldOfferRestartAfterAssign({ scope: "workgroup", restartSessions: false }, { status: "running", workingDirectory: wgPath })).toBe(false);
  });
});

describe("profileEnvOrigin (#526/#527 env origin badges)", () => {
  it("classifies an AgentsCommander-managed home path as system", () => {
    expect(profileEnvOrigin("CODEX_HOME", "%AC_ROOT%\\.codex\\agents\\codex")).toBe("system");
    expect(profileEnvOrigin("CLAUDE_CONFIG_DIR", "%AC_ROOT%/.claude")).toBe("system");
  });

  it("classifies a literal absolute path on a managed home key as accepted", () => {
    expect(profileEnvOrigin("CODEX_HOME", "D:\\manual\\codex")).toBe("accepted");
    expect(profileEnvOrigin("CODEX_HOME", "/opt/codex")).toBe("accepted");
  });

  it("classifies plain profile values (and non-home keys) as profile", () => {
    expect(profileEnvOrigin("OPENAI_ORG", "ac-prod")).toBe("profile");
    expect(profileEnvOrigin("AC_TRACE", "profile-c")).toBe("profile");
    // A non-home key keeps the profile origin even with an absolute-looking value.
    expect(profileEnvOrigin("SOME_PATH", "C:\\x")).toBe("profile");
  });
});

describe("effectiveEnvProjection (#527 Env / EFFECTIVE)", () => {
  it("merges agent env (system/accepted) with profile env (profile), profile wins", () => {
    const merged = effectiveEnvProjection(
      [
        { key: "CODEX_HOME", value: "%AC_ROOT%\\.codex", source: "system", enabled: true },
        { key: "OPENAI_API_KEY", value: "redacted", source: "user", enabled: true },
        { key: "DISABLED", value: "x", source: "user", enabled: false },
      ],
      { OPENAI_ORG: "ac-prod", OPENAI_API_KEY: "from-profile" },
      "C:\\root",
    );
    // Disabled agent rows are dropped; profile overrides the agent value on key collision.
    expect(merged).toEqual([
      { key: "CODEX_HOME", value: "C:\\root\\.codex", origin: "system" },
      { key: "OPENAI_API_KEY", value: "from-profile", origin: "profile" },
      { key: "OPENAI_ORG", value: "ac-prod", origin: "profile" },
    ]);
  });

  it("returns an empty list when nothing is configured", () => {
    expect(effectiveEnvProjection([], {}, null)).toEqual([]);
    expect(effectiveEnvProjection(undefined, undefined, undefined)).toEqual([]);
  });
});

describe("profileBadgeKind (#526/#527 shared Config/Selection taxonomy)", () => {
  // profiles(): codex configures A (empty cmd) + C; B is unconfigured everywhere.
  it("returns MATCH for the A baseline", () => {
    expect(profileBadgeKind(profiles(), "codex", "A")).toBe("match");
  });

  it("returns CONFIGURED for a non-A slot with its own enabled cell", () => {
    // codex C has its own enabled cell → direct match on a non-baseline slot.
    expect(profileBadgeKind(profiles(), "codex", "C")).toBe("configured");
  });

  it("returns FALLBACK for a non-A slot that resolves through a lower letter", () => {
    // codex B has no cell and is not configured on any other agent → falls back to A.
    expect(profileBadgeKind(profiles(), "codex", "B")).toBe("fallback");
  });

  it("returns MISSING for a slot configured on another agent but absent here", () => {
    const config: CodingAgentProfilesConfig = {
      schemaVersion: 2,
      profileSlots: { A: { label: "" }, B: { label: "fast" } },
      defaultProfileByAgent: {},
      profilesByAgent: {
        codex: { A: { enabled: true, command: "codex", env: {}, notes: "" } },
        claude: {
          A: { enabled: true, command: "claude", env: {}, notes: "" },
          B: { enabled: true, command: "claude --model opus", env: {}, notes: "" },
        },
      },
    };
    // B exists on claude but not on codex → codex B is MISSING (red), not fallback.
    expect(profileBadgeKind(config, "codex", "B")).toBe("missing");
    // And it IS configured elsewhere from codex's perspective.
    expect(profileConfiguredElsewhere(config, "codex", "B")).toBe(true);
    // From claude's perspective, B is its own cell → CONFIGURED.
    expect(profileBadgeKind(config, "claude", "B")).toBe("configured");
    expect(profileConfiguredElsewhere(config, "claude", "B")).toBe(false);
  });

  it("treats a disabled non-A cell as not configured (falls back, not configured)", () => {
    const config: CodingAgentProfilesConfig = {
      schemaVersion: 2,
      profileSlots: { A: { label: "" }, B: { label: "fast" } },
      defaultProfileByAgent: {},
      profilesByAgent: {
        codex: {
          A: { enabled: true, command: "codex", env: {}, notes: "" },
          B: { enabled: false, command: "codex --profile fast", env: {}, notes: "" },
        },
      },
    };
    // A disabled cell is not a live config → B resolves back to A.
    expect(profileBadgeKind(config, "codex", "B")).toBe("fallback");
  });
});
