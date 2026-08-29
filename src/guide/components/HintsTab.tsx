import { Component, For } from "solid-js";

interface Hint {
  title: string;
  body: string;
  link?: { label: string; url: string };
}

interface HintSection {
  agent: string;
  hints: Hint[];
}

const sections: HintSection[] = [
  {
    agent: "Claude Code",
    hints: [
      {
        title: "--enable-auto-mode",
        body: "Claude Code has an \"Auto\" mode that replaces permission prompts with an intelligent safety classifier. It auto-approves safe actions (reads, local edits) and blocks risky ones (force push, mass deletion, data exfiltration). It's the ideal middle ground between asking permission for everything and the dangerous --dangerously-skip-permissions.",
        link: {
          label: "Learn more in the official docs",
          url: "https://docs.anthropic.com/en/docs/claude-code/security#auto-accept-mode",
        },
      },
      {
        title: "claude-hud",
        body: "A statusline HUD for Claude Code that displays real-time context in your terminal - model, token usage, active tools, and session state at a glance. Essential for monitoring long-running agent sessions.",     
        link: {
          label: "GitHub repo",
          url: "https://github.com/jarrodwatts/claude-hud",
        },
      },
      {
        title: "feature-dev plugin",
        body: "Official Claude plugin for guided feature development. It analyzes your codebase, designs architectures, and writes implementation plans before writing code - resulting in higher quality features that follow your project's conventions.",
        link: {
          label: "Install plugin",
          url: "https://claude.com/plugins/feature-dev",
        },
      },
      {
        title: "Exclude global CLAUDE.md (Windows)",
        body: "By default, Claude Code loads ~/.claude/CLAUDE.md into every conversation. To prevent this, add the following to your settings.json or settings.local.json:\n\n{\n  \"claudeMdExcludes\": [\n    \"C:/Users/<your user name>/.claude/CLAUDE.md\"\n  ]\n}",
      },
    ],
  },
  {
    agent: "Codex",
    hints: [
      {
        title: "Load CLAUDE.md as project instructions",
        body: "Codex uses AGENTS.md by default, but you can make it fall back to CLAUDE.md when AGENTS.md is absent. Add this to your ~/.codex/config.toml:\n\nproject_doc_fallback_filenames = [\"CLAUDE.md\"]\n\nCodex will check AGENTS.override.md → AGENTS.md → fallback filenames, in that order.",
        link: {
          label: "Official config docs",
          url: "https://github.com/openai/codex/blob/main/docs/config.md",
        },
      },
    ],
  },
  {
    agent: "OpenCode",
    hints: [],
  },
  {
    agent: "Crucial Practices",
    hints: [
      {
        title: "Isolation and sandboxes",
        body: "For tasks that may touch local state, isolate the work before you delegate it. Disposable git worktrees, containers, and VMs are useful options. AgentsCommander room replicas are stronger but heavier: each room gets its own repo copy, agent session directories, messaging area, write zones, and executable, which keeps team context and test builds separated.",
      },
      {
        title: "Specification first",
        body: "Write the desired behavior, constraints, acceptance checks, and non-goals before implementation. A clear spec gives agents a stable target and makes review less subjective.",
      },
      {
        title: "Break work into small tasks",
        body: "Split large goals into reviewable steps with one clear outcome each. Smaller tasks reduce context drift and make it easier to recover when an agent chooses the wrong path.",
      },
      {
        title: "Use lighter agents when no filesystem is needed",
        body: "If a task only needs reasoning, drafting, classification, or summarization, use a model session without coding-agent filesystem instructions. It keeps prompts smaller and saves tokens.",
      },
      {
        title: "Polish context before starting",
        body: "Give agents concise, current context: the exact repo, branch, goal, constraints, files to inspect, and definition of done. Remove stale notes and irrelevant history.",
      },
      {
        title: "Evaluate the workflow",
        body: "Keep notes on what worked, what failed, where agents got stuck, and which prompts produced reliable results. Treat agent workflows as systems that need measurement.",
      },
      {
        title: "Prefer red/green development",
        body: "When behavior matters, ask for a failing test first, then the smallest change that makes it pass. This gives the agent a concrete feedback loop and gives reviewers evidence.",
      },
      {
        title: "Keep CI/CD authoritative",
        body: "Use CI to run formatting, linting, tests, security checks, and packaging. Agents should not be the final source of truth for whether a change is ready.",
      },
      {
        title: "Automate deterministic work",
        body: "If a step can be handled by a script, generator, formatter, linter, or validation command, make it deterministic. Save agent attention for judgment, debugging, and tradeoffs.",
      },
    ],
  },
];

const HintsTab: Component = () => {
  return (
    <div class="guide-tab-content">
      <For each={sections}>
        {(section) => (
          <div class="guide-section">
            <div class="guide-section-title">{section.agent}</div>
            <For each={section.hints}>
              {(hint) => (
                <div class="guide-card">
                  <div class="guide-card-title">{hint.title}</div>
                  <div class="guide-card-body">{hint.body}</div>
                  {hint.link && (
                    <a
                      class="guide-card-link"
                      href={hint.link.url}
                      target="_blank"
                      rel="noopener noreferrer"
                    >
                      {hint.link.label} &rarr;
                    </a>
                  )}
                </div>
              )}
            </For>
            {section.hints.length === 0 && (
              <div class="guide-card guide-card-draft">
                <div class="guide-card-body">No hints yet</div>
              </div>
            )}
          </div>
        )}
      </For>
    </div>
  );
};

export default HintsTab;