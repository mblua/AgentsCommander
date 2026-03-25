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
        body: "A statusline HUD for Claude Code that displays real-time context in your terminal — model, token usage, active tools, and session state at a glance. Essential for monitoring long-running agent sessions.",
        link: {
          label: "GitHub repo",
          url: "https://github.com/jarrodwatts/claude-hud",
        },
      },
      {
        title: "feature-dev plugin",
        body: "Official Claude plugin for guided feature development. It analyzes your codebase, designs architectures, and writes implementation plans before writing code — resulting in higher quality features that follow your project's conventions.",
        link: {
          label: "Install plugin",
          url: "https://claude.com/plugins/feature-dev",
        },
      },
      {
        title: "RTK — Token Optimizer",
        body: "CLI proxy that compresses command outputs to reduce token consumption. Prefix any command with rtk and it transparently compresses verbose outputs (git, cargo, npm, etc.) while passing through unchanged when no filter applies. Always safe to use.",
        link: {
          label: "rtk-ai.app",
          url: "https://www.rtk-ai.app/",
        },
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
