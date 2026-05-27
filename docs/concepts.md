# Concepts

For developers reading the docs for the first time. Eight terms — once these click, the rest of the docs make sense.

## Agent

A directory with a role-prompt file at its root: `CLAUDE.md`, `AGENTS.md`, or `GEMINI.md` depending on the coding agent. The directory IS the agent's identity. Everything inside is the agent's working context.

> **One agent = one directory.** Multiple role prompts inside the same directory tree are forbidden — coding agents read freely from their working directory, so a second role file would leak into the first agent's context.

An agent does not run by itself. It comes alive when you launch a session with a coding agent (Claude Code, Codex, or Gemini) pointed at that directory.

## Coding agent

The CLI process that does the actual LLM work: Claude Code, Codex, or Gemini. AgentsCommander is **not** a coding agent. It spawns coding agent processes and lets you watch and coordinate them.

You pick the coding agent **per session**. The same agent directory can be launched with Claude one day and Codex the next.

## Session

One running process bound to one agent directory, running inside a real PTY (ConPTY on Windows, Unix PTY on Linux/macOS). Each session shows in the sidebar with a status dot:

- green — idle, waiting for input
- yellow — running
- gray — exited

You can detach a session into its own window, attach a Telegram bot to it, or talk to it by voice.

## Team

A coordinator agent plus one or more worker agents working toward a shared goal. Teams are defined in a JSON config under `.ac-new/_team_<name>/` and discovered automatically.

The **coordinator** is the only member that can:
- send messages to any team member (members can only send to the coordinator and to peers they share a team with),
- edit the workgroup `TASK.md` brief through the CLI,
- close other members' sessions.

## Workgroup

A workgroup is a Team **in action** on a specific task. When the coordinator decides "we are working on task X," AC creates `.ac-new/wg-<N>-<team>/` and replicates the team's agent directories into it as **replicas** (`__agent_<name>/`). The replicas are isolated working copies — every replica has its own scratch space, inbox, and outbox.

You can run multiple workgroups for the same team in parallel.

## Brief

The plain-language description of the workgroup's goal. Lives at `.ac-new/wg-<N>-<team>/TASK.md` with YAML frontmatter for the title and a freeform body for context, links, and constraints.

The coordinator is the only agent that should be writing to the brief directly. Workers reference it and update their own outboxes.

## Messaging

Inter-agent communication is **file-based**. Every message is a markdown file at `.ac-new/wg-<N>-<team>/messaging/` with a UTC-timestamped filename:

```
YYYYMMDD-HHMMSS-wg<N>-<from>-to-wg<N>-<to>-<slug>.md
```

The sender writes the file, then calls `agentscommander send --to <peer> --send <filename> --mode wake`. The CLI injects a short notification into the recipient's PTY; the recipient reads the file via filesystem. Payload size is unbounded — PTY truncation does not apply.

Messages are never auto-purged. They form the audit trail of how the team worked.

See [Inter-agent messaging](agents/inter-agent-messaging.md).

## Agents Agency

A community library of agent role templates at [@msitarzewski/agency-agents](https://github.com/msitarzewski/agency-agents). AC ships a **vendored snapshot** so the role picker works offline. When you create a new agent, the picker lists every role from the snapshot plus your local templates.

The Agency does not run anything — it is a catalog of well-written role prompts. See [Coding agents and the Agents Agency picker](integrations/coding-agents.md).

---

Next: [Teams and workgroups](agents/teams-and-workgroups.md) to see how these pieces compose into real work.
