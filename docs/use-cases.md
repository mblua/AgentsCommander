# Use cases

For developers deciding whether AgentsCommander fits their workflow. Four worked examples — pick the one closest to your problem and adapt.

Every example assumes you have the [Quickstart](quickstart.md) finished: a project with a Project AC Root (`.ac/`) and at least one coding agent (Claude Code, Codex, Antigravity, or Pi) installed.

## 1. Parallel feature development

**Setup**: Two worker agents own different modules; an orchestrator routes work between them.

```
room-1-feature-x/
  __agent_tech-lead/         # orchestrator — Claude Code
  __agent_dev-backend/       # backend module
  __agent_dev-frontend/      # frontend module
```

Workflow:

1. Orchestrator reads `TASK.md`, splits the feature into a backend change and a frontend change.
2. Orchestrator sends each worker a markdown brief naming the files they own.
3. Workers branch and commit. They send progress messages back to the orchestrator.
4. Orchestrator merges, runs the e2e tests, and reports back to you.

You watch all three terminals at once. When the green dot lights up on the orchestrator, the feature is done.

## 2. Code-review swarm

**Setup**: One agent ships a PR; two others review it independently using different models.

```
room-2-review/
  __agent_shipper/           # opens the PR — Codex
  __agent_reviewer-claude/   # Claude Code
  __agent_reviewer-antigravity/   # Antigravity
```

Workflow:

1. Shipper commits and opens the PR with `gh pr create`.
2. Orchestrator (or you) sends the two reviewers a message with the PR URL.
3. Each reviewer pulls the diff (`gh pr diff`), reads it in its own terminal, and writes a markdown review to messaging.
4. You read both reviews side by side. Different model, different lens — disagreements are the interesting cases.

## 3. Autonomous refactor crew

**Setup**: A long-running orchestrator splits a multi-file refactor across worker agents and rebases their branches as they finish.

```
room-3-refactor-auth/
  __agent_arch/              # orchestrator — Claude Code, runs for hours
  __agent_dev-1/             # worker
  __agent_dev-2/             # worker
  __agent_dev-3/             # worker
```

Workflow:

1. Orchestrator parses the refactor plan, partitions files into three buckets.
2. Sends each worker a brief naming files in their bucket and the target API.
3. Workers branch from a shared base, commit independently.
4. Orchestrator runs `git rebase` to fold each worker's branch back, resolves trivial conflicts, and pings you only when human judgment is required.

This works because every coordination step is a markdown file you can audit. If the refactor goes sideways at 2 AM, you wake up, read `messaging/`, and know exactly where to step in.

## 4. Long-running agent with phone alerts

**Setup**: A single coding agent runs an overnight task. A [Telegram bot](features/telegram-bridge.md) bridges the PTY to your phone so you can kick off prompts from bed.

Workflow:

1. Configure a Telegram bot under **Settings → Integrations → Telegram**.
2. Attach the bot to the agent's session (right-click the session → **Attach Telegram bot**).
3. Send the agent a long-running prompt from your phone: *"Run the full integration suite, fix any failures, commit, push."*
4. The agent's PTY output streams to Telegram (filtered, rate-limited, vt100-cleaned). Reply from your phone if it asks a question.

Variant — voice-to-text: with [Gemini voice transcription](features/voice-to-text.md) enabled, hold the mic button on a session and dictate your prompt. Useful for long prompts while making coffee.

---

## Anti-patterns

A few configurations that look reasonable but cause pain:

- **One agent doing everything.** AC's value is multi-agent. If you only have one agent, you are paying for a Tauri shell around the coding agent CLI you already have — use the CLI directly.
- **Two role prompts in one directory.** Forbidden. The second agent will read the first one's role file and lose its identity. One agent = one directory.
- **Sharing `TASK.md` across teams.** Each room has its own brief. Sharing one brief across teams will produce orchestrator conflicts on the YAML frontmatter title.
- **Assuming orchestrators cannot talk to each other.** Orchestrators of any teams can message each other directly (cross-team orchestrator chat is allowed by the routing rules). The Root Agent (Project AC Root-level) is the top-level orchestrator that sees every room orchestrator as a peer and can route work across the whole project.

---

Have a use case worth adding? Open a [GitHub Discussion](https://github.com/mblua/AgentsCommander/discussions) under *Show & Tell*.
