import { describe, expect, it } from "vitest";
import type { AgentConfig, CodingAgentProfilesConfig } from "./types";
import {
  CLAUDE_CONTEXT_REGEX,
  CODEX_CONTEXT_REGEX,
  PI_CONTEXT_REGEX,
  commandExecutableBasename,
  composeEffectiveCommand,
  defaultInstructionsFilename,
  effectiveEnvProjection,
  executableBasename,
  expandAcPlaceholdersPreview,
  hasAcPlaceholder,
  isAcAgentPath,
  isCodexAgent,
  isWgReplicaPath,
  parseArgvText,
  profileBadgeKind,
  profileCellCommandText,
  profileConfiguredElsewhere,
  profileDisplayLabel,
  profileEnvOrigin,
  resolveProfileLabel,
  resolveProfilePreview,
  sessionProfileBadge,
  shouldMaskEnvValue,
  shouldOfferRestartAfterAssign,
  stringifyArgv,
  suggestedContextRegex,
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
    profileLabelsByAgent: {},
  };
}

function agent(overrides: Partial<AgentConfig>): AgentConfig {
  return {
    id: "custom",
    label: "Custom",
    command: "custom",
    color: "#000",
    envs: [],
    isolatedHome: false,
    ...overrides,
  };
}

describe("profile utils", () => {
  it("formats custom profile names as letter-name labels", () => {
    // No per-agent overrides → resolves via the legacy slot label (back-compat).
    const agents = [agent({ id: "codex" })];
    expect(profileDisplayLabel(profiles(), agents, "codex", "B")).toBe("B-FULL POWER");
    expect(profileDisplayLabel(profiles(), agents, "codex", "A")).toBe("A");
  });

  it("#548: per-agent labels — own override, primigenio inheritance, independence", () => {
    // codex = agents[0] = primigenio; claude is the second coding agent.
    const agents = [agent({ id: "codex" }), agent({ id: "claude" })];
    const p: CodingAgentProfilesConfig = {
      ...profiles(),
      profileLabelsByAgent: { codex: { B: "turbo" } },
    };
    // codex shows its OWN label.
    expect(profileDisplayLabel(p, agents, "codex", "B")).toBe("B-TURBO");
    // claude has no own B label → inherits the primigenio (codex).
    expect(profileDisplayLabel(p, agents, "claude", "B")).toBe("B-TURBO");
    // After claude sets its own B, editing claude does NOT change codex.
    const p2: CodingAgentProfilesConfig = {
      ...profiles(),
      profileLabelsByAgent: { codex: { B: "turbo" }, claude: { B: "zen" } },
    };
    expect(profileDisplayLabel(p2, agents, "claude", "B")).toBe("B-ZEN");
    expect(profileDisplayLabel(p2, agents, "codex", "B")).toBe("B-TURBO");
  });

  it("#548: a fresh config (no labels, empty slot) shows the bare letter", () => {
    const fresh: CodingAgentProfilesConfig = {
      schemaVersion: 2,
      profileSlots: { A: { label: "" } },
      defaultProfileByAgent: {},
      profilesByAgent: {},
      profileLabelsByAgent: {},
    };
    expect(profileDisplayLabel(fresh, [agent({ id: "codex" })], "codex", "A")).toBe("A");
  });

  it("#548: resolveProfileLabel follows own > primigenio > legacy slot > empty", () => {
    const agents = [agent({ id: "codex" }), agent({ id: "claude" })];
    const base = profiles(); // legacy slot labels: B = "full power", C = "fast"
    // 1. own label wins over everything.
    expect(
      resolveProfileLabel(
        { ...base, profileLabelsByAgent: { claude: { B: "own" }, codex: { B: "prim" } } },
        agents,
        "claude",
        "B",
      ),
    ).toBe("own");
    // 2. else the primigenio (codex) label, when the queried agent has none.
    expect(
      resolveProfileLabel(
        { ...base, profileLabelsByAgent: { codex: { B: "prim" } } },
        agents,
        "claude",
        "B",
      ),
    ).toBe("prim");
    // 3. else the legacy shared slot label (pre-#548 back-compat bridge).
    expect(
      resolveProfileLabel({ ...base, profileLabelsByAgent: {} }, agents, "claude", "B"),
    ).toBe("full power");
    // 4. else "" — the "no name" case the rail placeholder relies on.
    expect(
      resolveProfileLabel({ ...base, profileLabelsByAgent: {} }, agents, "claude", "A"),
    ).toBe("");
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

  it("composes the effective command from agent base + cell params (#597)", () => {
    // Mirrors the Rust compose_effective_command edge cases.
    expect(composeEffectiveCommand("claude", "--x")).toBe("claude --x");
    expect(composeEffectiveCommand("claude", "")).toBe("claude");
    expect(composeEffectiveCommand("", "--x")).toBe("--x");
    expect(composeEffectiveCommand("", "")).toBe("");
    // Each side is trimmed; no double space at the seam.
    expect(composeEffectiveCommand("  claude  ", "  --x  ")).toBe("claude --x");
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

  it("previews AC placeholder expansion for display only (3 tokens, #576)", () => {
    expect(hasAcPlaceholder("%AC_REPLICA_ROOT%\\.codex")).toBe(true);
    expect(hasAcPlaceholder("%AC_WORKSPACE_ROOT%\\.claude")).toBe(true);
    expect(hasAcPlaceholder("%AC_MATRIX_ROOT%\\.codex")).toBe(true);
    expect(hasAcPlaceholder("D:\\manual\\codex")).toBe(false);
    // Old %AC_ROOT% token is no longer recognized (breaking rename, no alias).
    expect(hasAcPlaceholder("%AC_ROOT%\\.codex")).toBe(false);

    // %AC_REPLICA_ROOT% → replica root; %AC_WORKSPACE_ROOT% and %AC_MATRIX_ROOT%
    // derive from a full replica path (…\.ac\wg-*\__agent_<name>).
    const replica = "C:\\proj\\.ac\\wg-6-dev-team\\__agent_codex";
    expect(expandAcPlaceholdersPreview("%AC_REPLICA_ROOT%\\.codex\\agents\\codex", replica)).toBe(
      "C:\\proj\\.ac\\wg-6-dev-team\\__agent_codex\\.codex\\agents\\codex",
    );
    expect(expandAcPlaceholdersPreview("%AC_WORKSPACE_ROOT%\\.claude", replica)).toBe(
      "C:\\proj\\.ac\\.claude",
    );
    expect(expandAcPlaceholdersPreview("%AC_MATRIX_ROOT%\\.codex", replica)).toBe(
      "C:\\proj\\.ac\\_agent_codex\\.codex",
    );

    // No root context → returned unchanged (backend expands authoritatively at launch).
    expect(expandAcPlaceholdersPreview("%AC_REPLICA_ROOT%\\.codex", null)).toBe(
      "%AC_REPLICA_ROOT%\\.codex",
    );
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
    expect(profileEnvOrigin("CODEX_HOME", "%AC_REPLICA_ROOT%\\.codex\\agents\\codex")).toBe("system");
    expect(profileEnvOrigin("CLAUDE_CONFIG_DIR", "%AC_REPLICA_ROOT%/.claude")).toBe("system");
    // Any of the three AC placeholders on a managed home key is system-managed.
    expect(profileEnvOrigin("CLAUDE_CONFIG_DIR", "%AC_WORKSPACE_ROOT%/.claude")).toBe("system");
    expect(profileEnvOrigin("CODEX_HOME", "%AC_MATRIX_ROOT%\\.codex")).toBe("system");
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
        { key: "CODEX_HOME", value: "%AC_REPLICA_ROOT%\\.codex", source: "system", enabled: true },
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
      profileLabelsByAgent: {},
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
      profileLabelsByAgent: {},
    };
    // A disabled cell is not a live config → B resolves back to A.
    expect(profileBadgeKind(config, "codex", "B")).toBe("fallback");
  });

  // #1033 - the suggested context pattern (plan SS9.1).
  //
  // The glyphs below are literals, not code-point escapes. What proves they are the
  // right code points is not this file: both constants are byte-identical to the
  // plan's SS5.4 and were injected from it rather than retyped, so the bytes are
  // verified at their source. These assertions pin the weaker, still-useful thing -
  // that the shipped pattern carries a bar glyph and a middle dot at all, and that
  // U+2593 never appears: it does not occur in the real output, and the issue's own
  // probe fixtures used it and tested fiction.
  it("suggests the captured Claude pattern verbatim (the_claude_suggestion_is_the_captured_pattern)", () => {
    expect(suggestedContextRegex("claude")).toBe(CLAUDE_CONTEXT_REGEX);
    expect(CLAUDE_CONTEXT_REGEX).toContain("░"); // LIGHT SHADE
    expect(CLAUDE_CONTEXT_REGEX).toContain("█"); // FULL BLOCK
    expect(CLAUDE_CONTEXT_REGEX).not.toContain("▓"); // never real output
  });

  it("suggests the captured Codex pattern verbatim (the_codex_suggestion_is_the_captured_pattern)", () => {
    expect(suggestedContextRegex("codex")).toBe(CODEX_CONTEXT_REGEX);
    expect(CODEX_CONTEXT_REGEX).toContain("·"); // MIDDLE DOT, adjacent to the capture
    expect(CODEX_CONTEXT_REGEX).not.toContain("▓");
  });

  it("suggests the exact Pi slash-footer pattern for supported command shapes", () => {
    expect(PI_CONTEXT_REGEX).toBe(String.raw`^(?:.*? )?(\d{1,3})\.\d%/`);
    expect(suggestedContextRegex("pi")).toBe(PI_CONTEXT_REGEX);
    expect(suggestedContextRegex("C:\\tools\\pi.exe")).toBe(PI_CONTEXT_REGEX);
    expect(suggestedContextRegex("cmd.exe /c pi")).toBe(PI_CONTEXT_REGEX);
  });

  it("keeps Pi precedence over Claude and Codex model tokens", () => {
    expect(suggestedContextRegex("pi --model claude-sonnet")).toBe(PI_CONTEXT_REGEX);
    expect(suggestedContextRegex("pi --model codex-model")).toBe(PI_CONTEXT_REGEX);
  });

  it("does not let standalone Pi argument values override the actual command", () => {
    expect(suggestedContextRegex("claude --model pi")).toBe(CLAUDE_CONTEXT_REGEX);
    expect(suggestedContextRegex("codex --profile pi")).toBe(CODEX_CONTEXT_REGEX);
    expect(suggestedContextRegex("echo pi")).toBeNull();
    expect(suggestedContextRegex("cmd.exe /c echo pi")).toBeNull();
  });

  it("does not treat Pi executable look-alikes as Pi", () => {
    for (const command of ["pip", "pipx", "ping", "pixel"]) {
      expect(suggestedContextRegex(command)).toBeNull();
    }
  });

  it("resolves a wrapped or suffixed claude the same way the filename default does (a_docker_wrapped_claude_still_resolves)", () => {
    // The same shapes defaultInstructionsFilename pins, since both reuse
    // executableTokenBasename over every token.
    expect(suggestedContextRegex("claude --model sonnet")).toBe(CLAUDE_CONTEXT_REGEX);
    expect(suggestedContextRegex("cmd.exe /c claude")).toBe(CLAUDE_CONTEXT_REGEX);
    expect(suggestedContextRegex("cmd.exe /c claude --continue")).toBe(CLAUDE_CONTEXT_REGEX);
    expect(suggestedContextRegex("claude-mb")).toBe(CLAUDE_CONTEXT_REGEX);
    expect(suggestedContextRegex("C:\\tools\\claude.exe")).toBe(CLAUDE_CONTEXT_REGEX);
    expect(suggestedContextRegex("codex --sandbox workspace-write")).toBe(CODEX_CONTEXT_REGEX);
    // claude > codex precedence, mirroring the Rust detector.
    expect(suggestedContextRegex("codex --base gemini")).toBe(CODEX_CONTEXT_REGEX);
  });

  it("suggests nothing for an agent no capture covered (an_unknown_agent_gets_no_suggestion)", () => {
    // Deliberately NOT defaultInstructionsFilename's "AGENTS.md"-style fallback: a
    // wrong filename costs a renamed file, a wrong pattern costs a silently wrong
    // percentage. No evidence, no guess.
    expect(suggestedContextRegex("gemini")).toBeNull();
    expect(suggestedContextRegex("gemini --yolo")).toBeNull();
    expect(suggestedContextRegex("nonsense")).toBeNull();
    expect(suggestedContextRegex("opencode")).toBeNull();
    expect(suggestedContextRegex("my-agent-cli --flag")).toBeNull();
    expect(suggestedContextRegex("")).toBeNull();
  });
});

describe("shouldMaskEnvValue (#1052 env value masking rule)", () => {
  it("masks keys that start with PASSWORD (case-insensitive, trimmed)", () => {
    expect(shouldMaskEnvValue("PASSWORD")).toBe(true);
    expect(shouldMaskEnvValue("PASSWORD_TOKEN_API")).toBe(true);
    expect(shouldMaskEnvValue("PASSWORDS")).toBe(true);
    expect(shouldMaskEnvValue("password_token")).toBe(true);
    expect(shouldMaskEnvValue("Password")).toBe(true);
    expect(shouldMaskEnvValue("PaSsWoRd")).toBe(true);
    expect(shouldMaskEnvValue("  password_x  ")).toBe(true);
  });
  it("leaves other keys in plaintext", () => {
    expect(shouldMaskEnvValue("")).toBe(false);
    expect(shouldMaskEnvValue("   ")).toBe(false);
    expect(shouldMaskEnvValue("CODEX_HOME")).toBe(false);
    expect(shouldMaskEnvValue("OPENAI_API_KEY")).toBe(false);
    expect(shouldMaskEnvValue("MY_PASSWORD")).toBe(false);
    expect(shouldMaskEnvValue("DB_PASSWORD")).toBe(false);
  });
});
