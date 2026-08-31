<p align="center">
  <img src="src-tauri/icons/icon.png" width="160" alt="Agents Commander logo" />
</p>

<h1 align="center">Agents Commander</h1>

<p align="center">
  <b>Compound your coding agents.</b> Bring any CLI coding agent at full power and activate Teams as parallel Rooms. Each Room is a separate filesystem workspace with replicas of its Team's agents and clones of its assigned repositories. <b>Parallelize the work. Keep the thread. Share the setup.</b>
</p>

<p align="center">
  <a href="https://github.com/mblua/AgentsCommander/releases/latest"><img src="https://img.shields.io/github/v/release/mblua/AgentsCommander?style=flat-square&color=00d4ff&label=release" alt="GitHub release" /></a>
  <a href="https://github.com/mblua/AgentsCommander/actions/workflows/release.yml"><img src="https://img.shields.io/github/actions/workflow/status/mblua/AgentsCommander/release.yml?style=flat-square&label=build" alt="Build" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue?style=flat-square" alt="License: MIT" /></a>
  <a href="https://github.com/mblua/AgentsCommander/stargazers"><img src="https://img.shields.io/github/stars/mblua/AgentsCommander?style=flat-square&color=00d4ff&label=stars" alt="GitHub stars" /></a>
  <a href="CODE_SIGNING_POLICY.md"><img src="https://img.shields.io/badge/code--signing-pending-00d4ff?style=flat-square&logo=windows" alt="Code signing pending" /></a>
  <a href="https://tauri.app"><img src="https://img.shields.io/badge/built%20with-Rust%20%2B%20Tauri%202-dea584?style=flat-square&logo=rust" alt="Built with Rust + Tauri 2" /></a>
</p>

<p align="center">
  <a href="#install-with-a-coding-agent"><b>Install with a Coding Agent</b></a>
  &nbsp;·&nbsp;
  <a href="docs/install-with-agent.md#support-gates"><b>Platform support</b></a>
  &nbsp;·&nbsp;
  <a href="docs/install-with-agent.md#manual-alternatives"><b>Manual install</b></a>
  &nbsp;·&nbsp;
  <a href="#60-second-quickstart"><b>▶ Quickstart</b></a>
</p>

---

## Installation

<a id="install-with-a-coding-agent"></a>

### Install with a trusted Coding Agent

Paste this prompt into a Coding Agent you already trust. It inspects first, shows you an exact plan, and waits before changing your machine.

```text
Install AgentsCommander safely on this machine.

Use only the official mblua/AgentsCommander repository, its latest stable
GitHub Release, and the canonical guide at:
https://github.com/mblua/AgentsCommander/blob/main/docs/install-with-agent.md

First resolve and report the full Git commit SHA for the current main snapshot,
then read the guide at that pinned commit. Stop if the guide, commit, stable
release, asset list, or SHASUMS256.txt is missing, ambiguous, or conflicting.

Before changing anything, detect and report the OS name and version, native CPU
architecture, process architecture if different, any existing AgentsCommander
installation, its exact binary version, and the support tier from the pinned
guide. Independently resolve and report the exact selected stable release tag.
Do not resolve or preserve configuration until both versions are known and you
have inspected each exact release tag's resolver; main is not evidence for a
published binary. Report both the existing selection and the replacement's
expected selection. Stop if a version is unknown, state cannot be attributed,
multiple plausible directories exist, a candidate is mounted/read-only and
ephemeral, or any evidence is ambiguous.
Windows 10 1809+ or Windows 11 on x86_64/AMD64 is fully supported. Linux
x86_64/AMD64 is partial/in progress: explain the limitations and wait for my
explicit confirmation before continuing. macOS is not supported yet: stop the
normal install and offer only the explicit tester/contributor path. Stop on any
other OS or architecture without emulation, substitution, or fallback.

Select only an asset explicitly mapped by the pinned guide. Report the stable
release tag and URL, exact asset name and URL, exact checksum record,
destination, every command, files created or overwritten, privilege level,
PATH or system-wide effects, configuration-preservation plan, validation, and
rollback. Prefer least privilege. Wait for my approval of that complete plan.

Treat elevation, system-wide or PATH changes, overwrites, and running unsigned
software as separate consent decisions. Current Windows artifacts may be
unsigned. Explain that a matching release checksum detects corruption or an
asset mismatch but does not protect against compromise of the publisher or
repository account.

After approval, download the exact asset and SHASUMS256.txt from the same stable
release, require one exact filename record, verify the complete SHA-256 digest,
back up the persistent configuration selected by the verified existing version,
run only the approved commands,
validate from the exact installed path, and report the result. Stop safely on
any mismatch. For a v0.30.3 AppImage update or uninstall, stop before mutation
when its selected candidate is in the temporary read-only mount, any existing
state is present, or selection is ambiguous; do not invent an external or home
configuration path.

Never bypass a security control silently, elevate automatically, use
`curl | shell`, use a mirror, build from source as a fallback, use an emulated or
substitute asset, fall back to npm, or install or authenticate a Coding Agent
CLI for me.
```

Read the complete [installation contract, platform matrix, trust boundaries, and rollback rules](docs/install-with-agent.md). Prefer a manual release download or npm? Those remain [secondary routes](docs/install-with-agent.md#manual-alternatives), with the same platform policy.

## The 30-second pitch

> **See which Team is working on each task—even while several Teams work in parallel.** Define agents and Teams in source-controlled files, then activate a Team as one or more Rooms. Each Room is a separate filesystem workspace with its own agent replicas, repository clones, messaging area, and build or test state. The UI keeps every Room's task, sessions, branches, and repository state visible.

- **Pick the coding agent per role (Claude Code, Codex, Antigravity, Pi, or OpenCode) at full power.** Each runs in its own real terminal with a full PTY, not a command runner. AgentsCommander only adds capability; it never sandboxes or nerfs your agent.
- **Direct multiple Rooms from the Root Agent.** The Agent Commander / Root Agent gives you one place to steer work across Teams. Ask it to talk to Room orchestrators, send work to different Teams, and keep initiatives aligned across parallel Rooms.
- **Keep each Room isolated on disk.** AC creates a separate workspace with repository clones, agent replicas, a messaging area, filesystem write boundaries, and a Room-specific executable. Run multiple Teams—or multiple Rooms for the same Team—without mixing source changes or build and test state.
- **Return without rebuilding the map.** A Room keeps its task, agent replicas, repository clones, and messages on disk. Come back later, reopen a Room's closed session, and AC asks Claude Code, Codex, Antigravity, or Pi to continue the prior conversation in that same workspace.
- **Share configuration as source code.** A Project's `.ac/` tree holds canonical agent roles, skills, and plans; Team rosters and repo access; and Loop prompts and schedules in reviewable Markdown, JSON, and TOML. Commit the shared files so teammates can pull the same reviewed setup instead of recreating it.
- **Coordinate Teams through auditable files.** Agents exchange markdown messages in a `messaging/` folder you can `cat`, `git diff`, and audit. The whole organization fits in `ls`.
- **Phone-ready updates with images.** The Telegram bridge can stream session output and send photos or screenshots captured by agents, so remote status can include the actual screen or report.
- **Local state, no telemetry.** Machine-local state stays in the configuration directory selected by the exact binary version; shared team state stays in each project's `.ac/` tree. The published `v0.30.3` resolver and the newer, unpublished `main` resolver differ, so use the [versioned configuration rule](docs/features/portable-instances.md#config-directory-rule) before moving or deleting state.

You bring the coding agents. AgentsCommander coordinates them.

## See it work

<p align="center">
  <img src="docs/screenshots/hero.png" alt="Agents Commander home screen with quick start guide and active agent sidebar" />
</p>

<a id="60-second-quickstart"></a>

## Quickstart: first team

1. **Install and validate** AgentsCommander with the [reviewable Coding Agent plan](#install-with-a-coding-agent). If you choose a secondary route, follow the [canonical platform and trust rules](docs/install-with-agent.md). Start the validated executable from its approved location.
2. **Open a project**: click `New Project` in the sidebar and point it at an empty folder. AC creates a Project AC Root (`.ac/`) there.
3. **Create a Team**: add an orchestrator and one worker agent, each with a role prompt. [Teams and rooms](docs/agents/teams-and-workgroups.md) walks through this.
4. **Launch the orchestrator**: pick Claude Code, Codex, Antigravity, Pi, or OpenCode from the dropdown. Ask it to send the worker a hello message. The worker terminal receives a file notification and responds in real time.

Full walkthrough: [`docs/quickstart.md`](docs/quickstart.md).

## Why this exists

Most agent tools focus on in-process orchestration or one interactive session. AgentsCommander starts with the **coding agents you already use** (Claude Code, Codex, Antigravity, Pi, and OpenCode), runs them as real OS processes, and lets them coordinate through plain markdown files that any human, any tool, and any `git diff` can inspect. You see every step in a real terminal, and the coordination state stays visible on disk.

## What you can build

| Use case | Setup |
|---|---|
| **Parallel feature development** | Activate one Room per parallel task, even when reusing the same Team. Every Room gets its own repository clones and build or test state; direct the Room orchestrators from the Root Agent. |
| **Code-review swarm** | One agent ships a PR; two others review independently. You read both reviews in their own terminals before merging. |
| **Autonomous refactor Team** | A long-running orchestrator splits a multi-file refactor across worker agents and rebases their branches as they finish. |
| **Long-running agent with phone alerts** | Pair a session with a [Telegram bot](docs/features/telegram-bridge.md), kick off a build from your phone, and receive text updates plus screenshots or image artifacts. |

Full recipes: [`docs/use-cases.md`](docs/use-cases.md).

## How it compares

| | AgentsCommander | LangGraph | AutoGen / AG2 | CrewAI | Aider | Claude Code alone |
|---|---|---|---|---|---|---|
| **Operates real CLI coding agents** | ✅ Claude Code, Codex, Antigravity, Pi, OpenCode | ❌ Python LLM calls | ❌ Python conversation | ❌ Python library | Partial (one agent) | ✅ (one agent) |
| **Real PTY per agent** | ✅ ConPTY / Unix PTY | ❌ | ❌ | ❌ | ✅ | ✅ |
| **Filesystem-first messaging** | ✅ Markdown in `messaging/` | ❌ DB / Python state | ❌ Python objects | ❌ Python tasks | n/a | n/a |
| **Standalone runtime** | ✅ Rust / Tauri | ❌ Python library | ❌ Python library | ❌ Python library | ❌ Python app | ✅ standalone CLI |
| **Multi-agent on the same repo** | ✅ | Partial | Partial | Partial | ❌ | ❌ |
| **Desktop UI** | ✅ Tauri app | ❌ | ❌ | ❌ | TUI only | TUI only |

Full comparison with trade-offs and honest losses: [`docs/comparison.md`](docs/comparison.md).

## Design principles

These are not accidents.

- **Start with files and CLIs.** AgentsCommander keeps the core workflow in plain files and real terminal sessions. External protocols, including MCP, belong where they improve a concrete integration without hiding the workflow.
- **Configuration is source code.** Canonical agent, Team, and Loop configuration lives in plain Markdown, JSON, and TOML under each Project's `.ac/` tree. Commit those shared files and review them like code; machine-local instance state stays outside that tree, and Rooms are gitignored runtime replicas.
- **Files before databases.** Shared configuration and inter-agent messages use plain files that are easy to inspect and debug. Databases can be introduced for performance-critical runtime paths without hiding how a Team is defined or how its agents coordinate.
- **One agent = one directory.** An agent is defined by a `CLAUDE.md` file (or equivalent role-prompt file) inside its own directory. Multiple role prompts within the same directory or its subdirectories are forbidden. Coding agents assume the entire contents of their working directory are relevant context; if two role prompts coexisted, an agent could read another agent's role and leak context.

## Documentation

- [Install with a Coding Agent](docs/install-with-agent.md): platform gates, approved assets, checksum verification, and rollback
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
| **Linux** | x86_64 / AMD64 | Partial and in progress; some features are untested or unsupported. Confirm the limitation before installing. |
| **macOS** | Any | Not supported yet because maintainer and test capacity is insufficient. Tester and contributor reports are welcome. |
| **Other combinations** | Any | Unsupported; do not substitute assets or use emulation as an install fallback. |

An artifact on a release is not a support promise. The [canonical installation guide](docs/install-with-agent.md) defines the operational gates and lists verified Windows-only features.

## Trust

AgentsCommander does not collect telemetry, analytics, or usage data. Optional features (Telegram Bridge, Voice-to-Text) transmit data to external services only when you enable them; see [`PRIVACY.md`](PRIVACY.md). Windows code signing is planned through [SignPath Foundation](https://signpath.org) with free signing courtesy of [SignPath.io](https://signpath.io), but current Windows release artifacts may be unsigned until [epic #717](https://github.com/mblua/AgentsCommander/issues/717) is complete. Verify the exact downloaded asset against the same release's `SHASUMS256.txt`; on Windows you can inspect signature status separately with:

```powershell
Import-Module (Join-Path $PSHOME "Modules\Microsoft.PowerShell.Security\Microsoft.PowerShell.Security.psd1") -ErrorAction Stop
Get-AuthenticodeSignature -LiteralPath ".\Agents.Commander_<version>_x64-setup.exe"
```

A release checksum detects corruption or a file that differs from the release record. It does not protect you if an attacker can replace both the asset and checksum through a compromised publisher or repository account. Full verification steps are in the [installation guide](docs/install-with-agent.md#verify-a-downloaded-asset-manually); signing policy is in [`CODE_SIGNING_POLICY.md`](CODE_SIGNING_POLICY.md).

## Community

- **Questions, ideas, show-and-tell**: [GitHub Discussions](https://github.com/mblua/AgentsCommander/discussions).
- **Bug reports, feature requests**: [GitHub Issues](https://github.com/mblua/AgentsCommander/issues).
- **What's next**: [`ROADMAP.md`](ROADMAP.md).

### Help extend Linux and macOS support

Linux support is partial and macOS is not supported yet. If you test either platform, file a reproducible [GitHub issue](https://github.com/mblua/AgentsCommander/issues) with your OS version, native architecture, AgentsCommander version or exact release asset, exact steps, expected result, actual result, and relevant sanitized logs. Contributors can follow [`CONTRIBUTING.md`](CONTRIBUTING.md); the [installation guide](docs/install-with-agent.md#help-extend-linux-and-macos-support) has a copyable report template.

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md): branch naming, local build, log-filter setup, and the docs style guide.

## Acknowledgments

AgentsCommander stands on the shoulders of:

- **[@msitarzewski/agency-agents](https://github.com/msitarzewski/agency-agents)**: community library of agent role templates. AC can download an explicit, offline cache with `agency-templates update`; normal startup and role browsing never hit the network. Big thanks to the maintainers for keeping it open.

Also: Tauri, SolidJS, xterm.js, portable-pty, axum, tokio; the toolchain layer this app would be impossible without.

## Author

Mariano is passionate about software development, AI, and blockchain. He approaches the world with deep curiosity, always amazed by life and the universe. Above all, he is a father, which remains the most wonderful part of his life.

**Mariano Blua**: [GitHub](https://github.com/mblua) · [LinkedIn](https://www.linkedin.com/in/mariano-blua/) · [🇦🇷 MarianoBlua](https://x.com/MarianoBlua) · [🇺🇸 MarianoBluaEN (English)](https://x.com/MarianoBluaEN)

## License

[MIT](LICENSE)
