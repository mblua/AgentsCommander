# Quickstart

For developers who want to get a team of coding agents running on a real repo in under 10 minutes.

By the end of this guide you will have AgentsCommander installed, an AC project pointing at one of your repos, a Team with two agents, and an orchestrator that delegates a task to a worker through a markdown message.

## Prerequisites

- A host covered by the [platform support policy](install-with-agent.md#support-gates). Windows 10 1809+ or Windows 11 on x86_64/AMD64 is fully supported; Linux x86_64/AMD64 is partial and requires explicit confirmation before installation. macOS is not a normal install target yet.
- At least one supported coding-agent CLI (Claude Code, Codex, Antigravity, or Pi) installed and authenticated as its CLI requires. See [Installing the coding-agent CLIs](integrations/coding-agents.md#installing-the-clis) for the exact upstream links and Pi install commands. You can install more than one; AC lets you pick per agent.
- Node.js 18+ and npm only if you deliberately choose the secondary npm route.
- Git installed.
- A repo you want the agents to work on (it can be empty).

You do **not** need Rust to run AgentsCommander. Rust is only needed if you want to build from source — see [`CONTRIBUTING.md`](../CONTRIBUTING.md).

## 1. Install AgentsCommander

Use the copyable [install-with-a-Coding-Agent prompt](../README.md#install-with-a-coding-agent). The Coding Agent reads the pinned [installation contract](install-with-agent.md), reports your host's support tier, selected stable release asset, checksum, destination, commands, privilege and `PATH` effects, validation, and rollback, then waits for your approval.

After it verifies the exact asset against `SHASUMS256.txt` and validates the executable, start AgentsCommander from the approved location. If you choose a secondary release or npm route, use the same platform and trust gates in the canonical guide.

## 2. Open or create an AC project

An **AC project** is a folder with a Project AC Root (`.ac/`) inside. AC stores agents, teams, and messaging here so the whole project is portable and version-controllable.

In the sidebar, click **New Project** and point at any folder, empty or an existing repo. AC creates `.ac/` with a sensible `.gitignore` and registers the project in your sidebar.

> CLI equivalent: `agentscommander new-project /path/to/folder`

## 3. Create your first Team

Open the Teams pane. A **Team** is one orchestrator plus one or more worker agents working toward a shared goal. The agents are created first, then the team definition links those existing agents together.

1. Click **+ Team** and give it a name (for example `feature-x`).
2. Add the **orchestrator** — give the agent a directory name (for example `tech-lead`), pick a role template from the [Agents Agency picker](integrations/coding-agents.md#role-template-picker), and finish.
3. Add one **worker** the same way (for example `dev-rust` with the *Rust developer* template).
4. Mark the first agent as **orchestrator**.

Behind the scenes AC creates agent matrices under `.ac/_agent_tech-lead/` and `.ac/_agent_dev-rust/`, then saves the team definition under `.ac/_team_<team-name>/`.

## 4. Write a brief and launch the orchestrator

Activate the team for a task and give the room a title. AC creates `.ac/room-1-<team-name>/`, including `TASK.md`, `messaging/`, and each `__agent_<name>/` replica. Open the new `TASK.md` and write what you want the team to do. One paragraph is enough; the orchestrator will expand it.

Click the orchestrator's session in the sidebar. AC opens a real terminal and prompts you to pick a coding agent (Claude Code, Codex, Antigravity, or Pi). Pick one. AC launches the agent in the orchestrator's directory with the role and brief already loaded.

## 5. Watch the agents exchange a message

Ask the orchestrator something like:

> "Send a hello message to `<project>:room-1-feature-x/dev-rust` and ask them to confirm the role they have."

The orchestrator will write a markdown file to `.ac/room-1-feature-x/messaging/` and run `agentscommander send --to <peer> --send <filename> --mode wake`. In a second or two you will see the worker's session activate, read the file, and reply by writing its own message back.

That's the loop. Every message is a file you can `cat`, `git diff`, and audit.

## Next steps

- [Concepts](concepts.md) — the vocabulary (agent, team, room, orchestrator, brief).
- [Teams and rooms](agents/teams-and-workgroups.md) — orchestrator authority, brief writing, recovery.
- [Inter-agent messaging](agents/inter-agent-messaging.md) — the file protocol and the `send` CLI.
- [Feature index](features/README.md): every feature page, grouped by what it does.
- [Use cases](use-cases.md) — recipes other people are running.
- [Troubleshooting](troubleshooting.md) — when something does not work.
- [Install and rollback](install-with-agent.md) — platform gates, checksum verification, secondary routes, and uninstall guidance.
