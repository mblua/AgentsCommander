# Concepts

For developers reading the docs for the first time. Thirteen terms. Once these click, the rest of the docs make sense.

## Agent

A directory with a role-prompt file at its root: `CLAUDE.md` or `AGENTS.md` depending on the coding agent. The directory IS the agent's identity. Everything inside is the agent's working context.

> **One agent = one directory.** Multiple role prompts inside the same directory tree are forbidden — coding agents read freely from their working directory, so a second role file would leak into the first agent's context.

An agent does not run by itself. It comes alive when you launch a session with a coding agent (Claude Code, Codex, Antigravity, or Pi) pointed at that directory.

## Coding agent

The CLI process that does the actual LLM work: Claude Code, Codex, Antigravity, or Pi. AgentsCommander is **not** a coding agent. It spawns coding-agent processes and lets you watch and coordinate them.

You pick the coding agent **per session**. The same agent directory can be launched with Claude one day and Codex the next.

## Profile

A lettered launch variant (`A`, `B`, `C`, ...) of a coding agent: extra command parameters plus environment variables, layered on top of the agent's base command. You set a default profile per agent and override it per session, so one `claude` entry can launch as "max effort" in one session and "cheap" in another.

See [Coding Agent Profiles](features/coding-agent-profiles.md).

## Session

One running process bound to one agent directory, running inside a real PTY (ConPTY on Windows, Unix PTY on Linux/macOS). Each session shows in the sidebar with a status dot:

- cyan — active (live PTY, currently working)
- blue — running (PTY output is streaming)
- green — waiting for human input (the agent finished its turn and is ready for your reply)
- amber — pending (the agent finished its turn but the row has not been focused yet)
- red — exited (clean or crash; detail in the row tooltip)
- gray — idle (no recent activity)
- translucent — offline (no live session row, for example an inactive member)

You can detach a session into its own window, attach a Telegram bot to it, or talk to it by voice. Idle teams can close their own sessions after a timeout; see [Session auto-close](features/session-auto-close.md).

## Team

An orchestrator agent plus one or more worker agents working toward a shared goal. Teams are defined in a JSON config under `.ac/_team_<name>/` and discovered automatically.

The **orchestrator** is the only member that can:
- send messages to any team member (members can only send to the orchestrator and to peers they share a team with),
- edit the workgroup `TASK.md` brief through the CLI,
- close other members' sessions.

## Workgroup

A workgroup is a Team **in action** on a specific task. When the orchestrator decides "we are working on task X," AC creates `.ac/wg-<N>-<team>/` and replicates the team's agent directories into it as **replicas** (`__agent_<name>/`). The replicas are isolated working copies — every replica has its own scratch space, inbox, and outbox.

You can run multiple workgroups for the same team in parallel.

## Non-stop mode

A per-project group of workgroups AC watches for you. While the group has at least one member and at least one alert measure turned on, AC compares how many of its workgroups are working against how many there are; a shortfall that lasts longer than the group's tolerance raises one alert. The default name of that group is `Alert me!`. See [Non-stop mode](features/non-stop-mode.md).

## Project Loop

A scheduled prompt. A Loop belongs to one project, targets one workgroup, and carries a cron expression and the text to send. When it comes due, AC delivers that text to the workgroup's orchestrator, waking or respawning the session if it is not running. See [Project Loops](features/project-loops.md).

## Watcher

A pattern AC matches against the terminal output of your agent sessions. Watchers live at the root of `settings.json`, keyed by watcher id, so one pattern can reach every configured agent, which the per-agent context pattern cannot. Matches land in the Watcher Activity window. See [Watchers](features/watchers.md).

## Spec Board

A separate window holding one Mermaid file: the source on one side, the rendered diagram on the other. It saves to a real file in your repository, snapshots your edits as you make them, and can hand the file to a running agent. See [Spec Board](features/spec-board.md).

## Brief

The plain-language description of the workgroup's goal. Lives at `.ac/wg-<N>-<team>/TASK.md` with YAML frontmatter for the title and a freeform body for context, links, and constraints.

The orchestrator is the only agent that should be writing to the brief directly. Workers reference it and update their own outboxes.

## Messaging

Inter-agent communication is **file-based**. Every message is a markdown file at `.ac/wg-<N>-<team>/messaging/` with a UTC-timestamped filename:

```
YYYYMMDD-HHMMSS-wg<N>-<from>-to-wg<N>-<to>-<slug>.md
```

The sender writes the file, then calls `agentscommander send --to <peer> --send <filename> --mode wake`. The CLI injects a short notification into the recipient's PTY; the recipient reads the file via filesystem. Payload size is unbounded — PTY truncation does not apply.

Messages are never auto-purged. They form the audit trail of how the team worked.

See [Inter-agent messaging](agents/inter-agent-messaging.md).

## Agents Agency

A community library of agent role templates at [@msitarzewski/agency-agents](https://github.com/msitarzewski/agency-agents). AC uses an explicit downloaded cache so the role picker works offline after `agency-templates update`. When you create a new agent, the picker lists cached Agency roles plus your local templates.

The Agency does not run anything — it is a catalog of well-written role prompts. See [Coding agents and the Agents Agency picker](integrations/coding-agents.md).

---

Next: [Teams and workgroups](agents/teams-and-workgroups.md) to see how these pieces compose into real work.
