import { describe, expect, it } from "vitest";
import agentsDefault from "../../src-tauri/resources/coding-agents/agents.default.json";
import {
  AGENT_HELP_SCHEMA_VERSION,
  EMBEDDED_AGENT_HELP,
  EMPTY_AGENT_HELP_OVERLAY,
  GENERIC_PARAMS_EXAMPLE,
  docsUrlFor,
  paramsExampleFor,
  resolveAgentHelpEntry,
  type AgentHelpEntry,
  type AgentHelpFile,
  type AgentHelpOverlay,
  type AgentHelpTip,
} from "./agent-help";

// Transcribed in source order from src/guide/components/HintsTab.tsx, which #2412
// deleted from main. Check it against the blob:
//   git cat-file -p e3e3f1a6daac60fe13918d367d1e1966ceefc82e
const EXPECTED_MOVED_HINTS: AgentHelpTip[] = [
  {
    title: "--enable-auto-mode",
    body: "Claude Code has an \"Auto\" mode that replaces permission prompts with an intelligent safety classifier. It auto-approves safe actions (reads, local edits) and blocks risky ones (force push, mass deletion, data exfiltration). It's the ideal middle ground between asking permission for everything and the dangerous --dangerously-skip-permissions.",
    link: {
      label: "Learn more in the official docs",
      url: "https://docs.anthropic.com/en/docs/claude-code/security#auto-accept-mode"
    }
  },
  {
    title: "claude-hud",
    body: "A statusline HUD for Claude Code that displays real-time context in your terminal - model, token usage, active tools, and session state at a glance. Essential for monitoring long-running agent sessions.",
    link: {
      label: "GitHub repo",
      url: "https://github.com/jarrodwatts/claude-hud"
    }
  },
  {
    title: "feature-dev plugin",
    body: "Official Claude plugin for guided feature development. It analyzes your codebase, designs architectures, and writes implementation plans before writing code - resulting in higher quality features that follow your project's conventions.",
    link: {
      label: "Install plugin",
      url: "https://claude.com/plugins/feature-dev"
    }
  },
  {
    title: "Exclude global CLAUDE.md (Windows)",
    body: "By default, Claude Code loads ~/.claude/CLAUDE.md into every conversation. To prevent this, add the following to your settings.json or settings.local.json:\n\n{\n  \"claudeMdExcludes\": [\n    \"C:/Users/<your user name>/.claude/CLAUDE.md\"\n  ]\n}"
  },
  {
    title: "Load CLAUDE.md as project instructions",
    body: "Codex uses AGENTS.md by default, but you can make it fall back to CLAUDE.md when AGENTS.md is absent. Add this to your ~/.codex/config.toml:\n\nproject_doc_fallback_filenames = [\"CLAUDE.md\"]\n\nCodex will check AGENTS.override.md → AGENTS.md → fallback filenames, in that order.",
    link: {
      label: "Official config docs",
      url: "https://github.com/openai/codex/blob/main/docs/config.md"
    }
  },
  {
    title: "Isolation and sandboxes",
    body: "For tasks that may touch local state, isolate the work before you delegate it. Disposable git worktrees, containers, and VMs are useful options. AgentsCommander room replicas are stronger but heavier: each room gets its own repo copy, agent session directories, messaging area, write zones, and executable, which keeps team context and test builds separated."
  },
  {
    title: "Specification first",
    body: "Write the desired behavior, constraints, acceptance checks, and non-goals before implementation. A clear spec gives agents a stable target and makes review less subjective."
  },
  {
    title: "Break work into small tasks",
    body: "Split large goals into reviewable steps with one clear outcome each. Smaller tasks reduce context drift and make it easier to recover when an agent chooses the wrong path."
  },
  {
    title: "Use lighter agents when no filesystem is needed",
    body: "If a task only needs reasoning, drafting, classification, or summarization, use a model session without coding-agent filesystem instructions. It keeps prompts smaller and saves tokens."
  },
  {
    title: "Polish context before starting",
    body: "Give agents concise, current context: the exact repo, branch, goal, constraints, files to inspect, and definition of done. Remove stale notes and irrelevant history."
  },
  {
    title: "Evaluate the workflow",
    body: "Keep notes on what worked, what failed, where agents got stuck, and which prompts produced reliable results. Treat agent workflows as systems that need measurement."
  },
  {
    title: "Prefer red/green development",
    body: "When behavior matters, ask for a failing test first, then the smallest change that makes it pass. This gives the agent a concrete feedback loop and gives reviewers evidence."
  },
  {
    title: "Keep CI/CD authoritative",
    body: "Use CI to run formatting, linting, tests, security checks, and packaging. Agents should not be the final source of truth for whether a change is ready."
  },
  {
    title: "Automate deterministic work",
    body: "If a step can be handled by a script, generator, formatter, linter, or validation command, make it deterministic. Save agent attention for judgment, debugging, and tradeoffs."
  }
];

const PLANNED_DOCS_URLS: Record<string, string> = {
  claude: "https://docs.claude.com/en/docs/claude-code/cli-reference",
  codex: "https://developers.openai.com/codex/cli/",
  hermes: "https://hermes.nousresearch.com/docs",
  agent: "https://cursor.com/docs/cli/overview",
  pi: "https://github.com/earendil-works/pi",
  opencode: "https://opencode.ai/docs/cli/",
  agy: "https://antigravity.google/docs/cli",
  grok: "https://docs.x.ai/build/overview",
};

const PLANNED_PARAMS_EXAMPLES: Record<string, string> = {
  claude: "--model opus",
  codex: "--sandbox workspace-write --model gpt-5-codex",
  hermes: "--help",
  agent: "--help",
  pi: "--help",
  opencode: "--help",
  agy: "--help",
  grok: "--help",
};

const byCommand = EMBEDDED_AGENT_HELP.byCommand ?? {};

function file(parts: Partial<AgentHelpFile>): AgentHelpFile {
  return { schemaVersion: 1, ...parts };
}

function overlay(parts: Partial<AgentHelpOverlay>): AgentHelpOverlay {
  return { ...EMPTY_AGENT_HELP_OVERLAY, ...parts };
}

const LOCAL_AGENT: AgentHelpEntry = { label: "local agent", paramsExample: "--local-agent" };
const LOCAL_COMMAND: AgentHelpEntry = { label: "local command", paramsExample: "--local-command" };
const REMOTE_COMMAND: AgentHelpEntry = { label: "remote command" };

describe("embedded agent help", () => {
  it("the embedded file declares schema version 1", () => {
    expect(EMBEDDED_AGENT_HELP.schemaVersion).toBe(AGENT_HELP_SCHEMA_VERSION);
  });

  it("the embedded file covers every enabled built-in command", () => {
    const expected = agentsDefault.agents
      .filter((agent) => agent.key !== "muse")
      .map((agent) => agent.command)
      .sort();
    expect(expected).toHaveLength(8);
    expect(Object.keys(byCommand).sort()).toEqual(expected);
  });

  it("the embedded file carries no byAgent rows and no guideOrder", () => {
    expect(EMBEDDED_AGENT_HELP).not.toHaveProperty("byAgent");
    expect(EMBEDDED_AGENT_HELP).not.toHaveProperty("guideOrder");
  });

  it("every embedded URL is https", () => {
    const entries = [EMBEDDED_AGENT_HELP.general ?? {}, ...Object.values(byCommand)];
    const urls = entries.flatMap((entry) => [
      ...(entry.docsUrl ? [entry.docsUrl] : []),
      ...(entry.tips ?? []).flatMap((tip) => (tip.link ? [tip.link.url] : [])),
    ]);
    expect(urls).toHaveLength(12);
    for (const url of urls) expect(new URL(url).protocol).toBe("https:");
  });

  it("every built-in carries the planned docsUrl", () => {
    for (const [key, url] of Object.entries(PLANNED_DOCS_URLS)) {
      expect(byCommand[key]?.docsUrl).toBe(url);
    }
  });

  it("every built-in carries the planned paramsExample", () => {
    for (const [key, example] of Object.entries(PLANNED_PARAMS_EXAMPLES)) {
      expect(byCommand[key]).toHaveProperty("paramsExample", example);
    }
  });

  it("the moved hints are preserved verbatim", () => {
    const claude = byCommand.claude?.tips ?? [];
    const codex = byCommand.codex?.tips ?? [];
    const opencode = byCommand.opencode?.tips ?? [];
    const general = EMBEDDED_AGENT_HELP.general?.tips ?? [];
    expect([claude.length, codex.length, opencode.length, general.length]).toEqual([4, 1, 0, 9]);
    const moved = [...claude, ...codex, ...opencode, ...general];
    expect(moved).toHaveLength(14);
    expect(moved).toEqual(EXPECTED_MOVED_HINTS);
  });
});

describe("resolveAgentHelpEntry", () => {
  const layered = overlay({
    local: file({ byAgent: { mine: LOCAL_AGENT }, byCommand: { claude: LOCAL_COMMAND } }),
    remote: file({ byCommand: { claude: REMOTE_COMMAND, codex: REMOTE_COMMAND } }),
  });

  it("local byAgent wins over every other layer", () => {
    expect(resolveAgentHelpEntry(layered, "mine", "claude")).toEqual(LOCAL_AGENT);
  });

  it("local byCommand wins over remote", () => {
    expect(resolveAgentHelpEntry(layered, "other", "claude")).toEqual(LOCAL_COMMAND);
  });

  it("remote wins over embedded", () => {
    expect(resolveAgentHelpEntry(layered, "other", "codex")).toEqual(REMOTE_COMMAND);
  });

  it("embedded applies when the overlay is empty", () => {
    expect(resolveAgentHelpEntry(EMPTY_AGENT_HELP_OVERLAY, "other", "codex")).toEqual(byCommand.codex);
  });

  it("an absent key falls through to the next layer", () => {
    const local = file({ byAgent: { someone: LOCAL_AGENT }, byCommand: { grok: LOCAL_COMMAND } });
    expect(resolveAgentHelpEntry(overlay({ local, remote: layered.remote }), "x", "codex")).toEqual(
      REMOTE_COMMAND
    );
    expect(resolveAgentHelpEntry(overlay({ local }), "x", "hermes")).toEqual(byCommand.hermes);
  });

  it("an empty command resolves to no entry", () => {
    const emptyStem = '""';
    for (const command of ["", "   ", emptyStem]) {
      expect(resolveAgentHelpEntry(layered, "other", command)).toBeNull();
    }
    expect(resolveAgentHelpEntry(layered, "mine", "")).toEqual(LOCAL_AGENT);
  });

  it("a stem is matched exactly and case-insensitively", () => {
    expect(resolveAgentHelpEntry(EMPTY_AGENT_HELP_OVERLAY, "x", "C:\\tools\\CLAUDE.EXE")).toEqual(
      byCommand.claude
    );
    expect(resolveAgentHelpEntry(EMPTY_AGENT_HELP_OVERLAY, "x", "claude-code")).toBeNull();
  });
});

describe("paramsExampleFor and docsUrlFor", () => {
  it("paramsExampleFor falls back to the generic example", () => {
    expect(paramsExampleFor(EMPTY_AGENT_HELP_OVERLAY, "x", "unknown-agent")).toBe(GENERIC_PARAMS_EXAMPLE);
    expect(paramsExampleFor(EMPTY_AGENT_HELP_OVERLAY, "x", "")).toBe("--help");
    expect(paramsExampleFor(EMPTY_AGENT_HELP_OVERLAY, "x", "codex")).toBe(
      "--sandbox workspace-write --model gpt-5-codex"
    );
  });

  it("docsUrlFor rejects a non-https url", () => {
    for (const docsUrl of ["http://example.com/docs", "javascript:alert(1)", "not a url"]) {
      const local = file({ byCommand: { tool: { docsUrl } } });
      expect(docsUrlFor(overlay({ local }), "x", "tool")).toBeNull();
    }
    expect(docsUrlFor(EMPTY_AGENT_HELP_OVERLAY, "x", "")).toBeNull();
    expect(docsUrlFor(EMPTY_AGENT_HELP_OVERLAY, "x", "grok")).toBe(PLANNED_DOCS_URLS.grok);
  });
});
