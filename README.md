<p align="center">
  <img src="src-tauri/icons/icon.png" width="160" alt="Agents Commander logo" />
</p>

<h1 align="center">Agents Commander</h1>

<p align="center">
  <b>Stop letting your experience disappear between sessions. Build it into your own agent ecosystem.</b> For developers working solo or in Teams: keep roles, skills, and recorded experience in your repository, refine them from real work, and bring the setup to your next task. Your coding agents run in real terminals; you decide how they work together.
</p>

<p align="center">
  <a href="https://github.com/mblua/AgentsCommander/releases/latest"><img src="https://img.shields.io/github/v/release/mblua/AgentsCommander?style=flat-square&color=00d4ff&label=release" alt="GitHub release" /></a>
  <a href="https://github.com/mblua/AgentsCommander/actions/workflows/release.yml"><img src="https://img.shields.io/github/actions/workflow/status/mblua/AgentsCommander/release.yml?style=flat-square&label=build" alt="Build" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue?style=flat-square" alt="License: MIT" /></a>
  <a href="https://github.com/mblua/AgentsCommander/stargazers"><img src="https://img.shields.io/github/stars/mblua/AgentsCommander?style=flat-square&color=00d4ff&label=stars" alt="GitHub stars" /></a>
  <a href="https://tauri.app"><img src="https://img.shields.io/badge/built%20with-Rust%20%2B%20Tauri%202-dea584?style=flat-square&logo=rust" alt="Built with Rust + Tauri 2" /></a>
</p>


<p align="center">
  <a href="https://agentscommander.org/"><b>Explore the ecosystem</b></a>
  &nbsp;·&nbsp;
  <a href="#get-started"><b>Get started</b></a>
  &nbsp;·&nbsp;
  <a href="docs/quickstart.md"><b>Your first team</b></a>
</p>

<p align="center">
  <img src="docs/screenshots/hero.png" alt="AgentsCommander desktop with agent terminals and the active agent sidebar" />
</p>

## Get started

<a id="install-with-a-coding-agent"></a>

Bring your own coding-agent CLIs, accounts, and model access. Choose how you want to install AgentsCommander.

### 1. Install with npm

Requires Node.js 18 or newer and npm. Run:

```bash
npm install -g @mblua/agentscommander@latest
```

Then launch:

```bash
agentscommander
```

See the [npm guide](npm/README.md) for supported platforms, verification, and preserving your configuration when upgrading.

### 2. Install with winget on Windows

```powershell
winget install --id mblua.AgentsCommander --exact --source winget
```

### 3. Ask your coding agent

Paste this into the coding agent you already use:

```text
Install the stable version of AgentsCommander with npm install -g @mblua/agentscommander@latest.
Verify that it works, tell me how to launch it, and explain the basic concepts used by AgentsCommander: Agents-Matrix, Teams, and Rooms.
```

Prefer a release download? See [GitHub Releases](https://github.com/mblua/AgentsCommander/releases/latest) and the [installation guide](docs/install-with-agent.md) for the right asset and verification steps. macOS uses the npm route.

**Platform support:** Windows is the primary supported platform. Linux has medium test coverage; macOS uses npm and has very low test coverage. Check the [platform details](#platform-support) before installing. Before updating an existing installation, follow the [configuration backup guidance](npm/README.md#resolve-versions-and-preserve-configuration).

## Your experience belongs in your next task

A useful session leaves more than code: a clearer role, a tested skill, a recorded mistake, or a better way to delegate. Keep those lessons in files you can review and improve, then bring the setup to your next task.

**AgentsCommander is an open source playground and meta-harness for coding agents.** Your agents keep their own tools and run in real terminals. AgentsCommander adds the surrounding workspace for roles, skills, reusable Teams, Rooms, and coordination you can inspect.

Start with one agent configured your way, or build a Team with an orchestrator and the specialists you choose. You bring your CLIs, accounts, and model access.

## Create → Use → Refine → Reuse

1. **Create your setup.** Define agents, their roles and skills. Use one agent day to day, or group agents into a Team.
2. **Use it on real work.** Activate a Team as a Room, give its orchestrator a task, and follow the terminals and messages. Open another Room for another task.
3. **Refine it from evidence.** Review messages, recorded errors, plans, notes, tests, and diffs. You can do this yourself or prepare a team of agents for that review.
4. **Reuse what works.** Improve the roles, skills, and instructions you maintain, version the reusable configuration, and carry it into the next task.

You decide what to record and improve. This workflow preserves explicit knowledge in files; it does not automatically train models or guarantee better results.

## Start with one agent. Grow your own ecosystem.

- **Solo:** give an agent a role and the skills it needs. Keep refining its setup as you learn what works.
- **Teams:** define an orchestrator and specialist agents once. Activate the Team as a Room for each task, with its own agent replicas and repository clones.
- **Multiple Rooms:** run independent tasks in separate workspaces and steer their orchestrators from the Root Agent.

Roles, skills, messages, recorded errors, plans, and notes are plain files you can inspect with ordinary developer tools. Share reviewed configuration through Git; keep credentials and machine-local runtime state separate. Not every runtime file belongs in version control.

## The harness around your coding agents

- **Real terminals, real CLIs.** Claude Code, Codex, Antigravity, and Pi have dedicated integrations. Other shell-based CLIs, including OpenCode, can use custom agents; not every integration feature is available to custom commands.
- **Delegation you can inspect.** Orchestrators send briefs and collect replies as timestamped Markdown messages on disk.
- **Separate workspaces for separate tasks.** Rooms have their own repository clones and working state. Agent write restrictions are instructions, not an enforced security sandbox.
- **Resume from explicit state.** Rooms retain files and messages. Compatible CLIs can also use a written handoff and best-effort context clearing; unsupported wrappers may not support that flow.
- **Experiment with your setup.** Rerun a brief with different roles, skills, or CLI profiles, then inspect tests, diffs, messages, and terminal output yourself.
- **Keep work visible.** Follow agent terminals and session states from one desktop window. Optionally connect Telegram for updates, replies, and images.
- **Local project state, no telemetry.** See the [storage and backup contract](https://github.com/mblua/AgentsCommander/issues/1118) before moving or deleting application state, and [Privacy](PRIVACY.md) for optional external services.

<a id="60-second-quickstart"></a>

## Quickstart: first team

1. **Install and launch** using [npm or your coding agent](#get-started), then open AgentsCommander.
2. **Open a project**: click `New Project` in the sidebar and point it at an empty folder. AC creates a Project AC Root (`.ac/`) there.
3. **Create a Team**: add an orchestrator and one worker agent, each with a role prompt. [Teams and rooms](docs/agents/teams-and-workgroups.md) walks through this.
4. **Launch the orchestrator**: pick Claude Code, Codex, Antigravity, Pi, or OpenCode from the dropdown. Ask it to send the worker a hello message. The worker terminal receives a file notification and responds in real time.

Full walkthrough: [`docs/quickstart.md`](docs/quickstart.md).

## What will you build?

Use your ecosystem for feature development, independent reviews, coordinated refactoring, research, or long-running tasks with phone check-ins. Build different teams for different jobs and refine them using evidence from their work.

See the [workflow recipes](docs/use-cases.md), or share your own setup with the community.

## Design principles

These are not accidents.

- **Start with files and CLIs.** AgentsCommander keeps the core workflow in plain files and real terminal sessions.
- **Configuration is source code.** Canonical agent, Team, and Loop configuration lives in plain Markdown, JSON, and TOML under each Project's `.ac/` tree. Commit those shared files and review them like code. Gitignored runtime and machine-local state (Rooms, Loop state, and project-local settings) can also live under `.ac/`; keep it distinct from the versioned configuration.
- **Files before databases.** Shared configuration and inter-agent messages use plain files that are easy to inspect and debug. Databases can be introduced for performance-critical runtime paths without hiding how a Team is defined or how its agents coordinate.
- **One agent = one directory.** An agent is defined by a `CLAUDE.md` file (or equivalent role-prompt file) inside its own directory. Multiple role prompts within the same directory or its subdirectories are forbidden. Coding agents assume the entire contents of their working directory are relevant context; if two role prompts coexisted, an agent could read another agent's role and leak context.

## Documentation

- [Installation guide](docs/install-with-agent.md): release downloads, platform support, verification, and rollback
- [npm guide](npm/README.md): package installation, launch, and upgrades
- [Quickstart](docs/quickstart.md): installed app to first running agent
- [Concepts](docs/concepts.md): agent, team, room, orchestrator, brief
- [Teams and rooms](docs/agents/teams-and-workgroups.md): orchestrators, members, briefs, messaging
- [Features](docs/features/): [Coding Agent Profiles](docs/features/coding-agent-profiles.md), [Session auto-close](docs/features/session-auto-close.md), [Config seed](docs/features/config-seed.md), [Seed manifest](docs/features/seed-manifest.md), [Container coding agents](docs/features/container-coding-agents.md), Telegram bridge with image and screenshot sends, voice-to-text, portable instances
- [Reference](docs/reference/): full CLI, `settings.json` schema, architecture, log filtering
- [Roadmap](ROADMAP.md) · [Changelog](CHANGELOG.md) · [Docs style guide](docs/style-guide.md)

## Platform support

| Platform | Native architecture | Status |
|---|---|---|
| **Windows 10 1809+ / Windows 11** | x86_64 / AMD64 | Fully supported; primary development and release-validation platform. |
| **Linux** | x86_64 / AMD64 | Supported with medium test coverage; some features are untested or unsupported, so expect some turbulence. Confirm the limitation before installing. |
| **macOS** | Any | Supported via npm only, with very low test coverage; errors are expected. |
| **Other combinations** | Any | Unsupported; do not substitute assets or use emulation as an install fallback. |

An artifact on a release is not a support promise. The [canonical installation guide](docs/install-with-agent.md) defines the operational gates and lists verified Windows-only features.

## Trust

AgentsCommander does not collect telemetry, analytics, or usage data. Optional features (Telegram Bridge, Voice-to-Text, and Jev classification for [Co-managed rooms](docs/features/co-managed-rooms.md), which sends captured agent text only when the global `coManagedEnabled` switch, off by default while in development, is on, a room's flag is on and a Jev key is set) transmit data to external services only when you enable them; see [`PRIVACY.md`](PRIVACY.md).
## Community

- **Discuss setups and share what you learn**: join the [official Discord](https://discord.gg/EcwW5KvDCy).
- **Watch videos**: [Mariano Blua on YouTube](https://www.youtube.com/@marianoblua).
- **Questions, ideas, show-and-tell**: [GitHub Discussions](https://github.com/mblua/AgentsCommander/discussions).
- **Bug reports, feature requests**: [GitHub Issues](https://github.com/mblua/AgentsCommander/issues).
- **What's next**: [`ROADMAP.md`](ROADMAP.md).

### Help extend Linux and macOS support

Linux is supported with medium test coverage and macOS via npm with very low test coverage. Both tiers need the reports that turn gaps into fixes. If either platform misbehaves, file a reproducible [GitHub issue](https://github.com/mblua/AgentsCommander/issues) with your OS version, native architecture, AgentsCommander version or exact release asset, exact steps, expected result, actual result, and relevant sanitized logs. Contributors can follow [`CONTRIBUTING.md`](CONTRIBUTING.md); the [installation guide](docs/install-with-agent.md#help-extend-linux-and-macos-support) has a copyable report template.

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md): branch naming, local build, log-filter setup, and the docs style guide.

## Acknowledgments

AgentsCommander stands on the shoulders of:

- **[@msitarzewski/agency-agents](https://github.com/msitarzewski/agency-agents)**: community library of agent role templates. AC can download an explicit, offline cache with `agency-templates update`; normal startup and role browsing never hit the network. Big thanks to the maintainers for keeping it open.

Also: Tauri, SolidJS, xterm.js, portable-pty, axum, tokio; the toolchain layer this app would be impossible without.

## Author

Mariano is passionate about software development, AI, and blockchain. He approaches the world with deep curiosity, always amazed by life and the universe. Above all, he is a father, which remains the most wonderful part of his life.

**Mariano Blua**: [GitHub](https://github.com/mblua) · [LinkedIn](https://www.linkedin.com/in/mariano-blua/) · [X](https://x.com/MarianoBlua)

## License

[MIT](LICENSE)
