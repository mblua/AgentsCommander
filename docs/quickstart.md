# Quickstart

For developers who want to get a team of coding agents running on a real repo in under 10 minutes.

By the end of this guide you will have AgentsCommander installed, an AC project pointing at one of your repos, a Team with two agents, and a coordinator that delegates a task to a worker through a markdown message.

## Prerequisites

- At least one supported coding-agent CLI (Claude Code, Codex, Gemini, or Pi) installed and authenticated as its CLI requires. See [Installing the coding-agent CLIs](integrations/coding-agents.md#installing-the-clis) for the exact upstream links and Pi install commands. You can install more than one; AC lets you pick per agent.
- Node.js 18+ and npm if you install AgentsCommander from npm.
- Git installed.
- A repo you want the agents to work on (it can be empty).

You do **not** need Rust to run AgentsCommander. Rust is only needed if you want to build from source — see [`CONTRIBUTING.md`](../CONTRIBUTING.md).

## 1. Install AgentsCommander

The fastest install path is npm:

```bash
npm install -g @mblua/agentscommander
```

Start AgentsCommander:

```bash
agentscommander
```

The npm package is `@mblua/agentscommander`. The installed command is still `agentscommander`.

If you prefer a desktop installer or manual download, get the latest asset for your platform from [GitHub Releases](https://github.com/mblua/AgentsCommander/releases/latest):

| Platform | Asset |
|---|---|
| Windows 10 1809+ | `Agents Commander_X.Y.Z_x64-setup.exe` or the portable `agentscommander.exe` |
| Linux | `agentscommander_*_amd64.AppImage` |
| macOS | `Agents Commander_*.dmg` (Apple Silicon + Intel) |

Windows code signing is planned through SignPath and pending setup. Current Windows artifacts may be unsigned until [epic #717](https://github.com/mblua/AgentsCommander/issues/717) is complete. Verify downloads with the release `SHASUMS256.txt`; on Windows you can inspect signature status with:

```powershell
Get-AuthenticodeSignature "Agents Commander_X.Y.Z_x64-setup.exe"
```

Run the installer, or drop the portable `.exe` into any folder and double-click. On first launch AC creates its config directory next to the binary (e.g. `.agentscommander/` on Windows). See [Portable instances](features/portable-instances.md) for the rules.

## 2. Open or create an AC project

An **AC project** is a folder with a Project AC Root (`.ac/`) inside. AC stores agents, teams, and messaging here so the whole project is portable and version-controllable.

In the sidebar, click **New Project** and point at any folder, empty or an existing repo. AC creates `.ac/` with a sensible `.gitignore` and registers the project in your sidebar.

> CLI equivalent: `agentscommander new-project /path/to/folder`

## 3. Create your first Team

Open the Teams pane. A **Team** is one coordinator plus one or more worker agents working toward a shared goal. The agents are created first, then the team definition links those existing agents together.

1. Click **+ Team** and give it a name (for example `feature-x`).
2. Add the **coordinator** — give the agent a directory name (for example `tech-lead`), pick a role template from the [Agents Agency picker](integrations/coding-agents.md#role-template-picker), and finish.
3. Add one **worker** the same way (for example `dev-rust` with the *Rust developer* template).
4. Mark the first agent as **coordinator**.

Behind the scenes AC creates agent matrices under `.ac/_agent_tech-lead/` and `.ac/_agent_dev-rust/`, then saves the team definition under `.ac/_team_<team-name>/`.

## 4. Write a brief and launch the coordinator

Activate the team for a task and give the workgroup a title. AC creates `.ac/wg-1-<team-name>/`, including `TASK.md`, `messaging/`, and each `__agent_<name>/` replica. Open the new `TASK.md` and write what you want the team to do. One paragraph is enough; the coordinator will expand it.

Click the coordinator's session in the sidebar. AC opens a real terminal and prompts you to pick a coding agent (Claude Code, Codex, Gemini, or Pi). Pick one. AC launches the agent in the coordinator's directory with the role and brief already loaded.

## 5. Watch the agents exchange a message

Ask the coordinator something like:

> "Send a hello message to `<project>:wg-1-feature-x/dev-rust` and ask them to confirm the role they have."

The coordinator will write a markdown file to `.ac/wg-1-feature-x/messaging/` and run `agentscommander send --to <peer> --send <filename> --mode wake`. In a second or two you will see the worker's session activate, read the file, and reply by writing its own message back.

That's the loop. Every message is a file you can `cat`, `git diff`, and audit.

## Next steps

- [Concepts](concepts.md) — the vocabulary (agent, team, workgroup, coordinator, brief).
- [Teams and workgroups](agents/teams-and-workgroups.md) — coordinator authority, brief writing, recovery.
- [Inter-agent messaging](agents/inter-agent-messaging.md) — the file protocol and the `send` CLI.
- [Feature index](features/README.md): every feature page, grouped by what it does.
- [Use cases](use-cases.md) — recipes other people are running.
- [Troubleshooting](troubleshooting.md) — when something does not work.


## Uninstalling on Windows

If you installed AgentsCommander using the setup installer (.exe), you can remove it using the standard Windows settings:
1. Open Windows **Settings** > **Apps** > **Installed apps**.
2. Search for **Agents Commander**.
3. Click the menu (three dots) next to it and select **Uninstall**.
