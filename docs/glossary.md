# Glossary

Quick alphabetical reference. For richer explanations of how the pieces fit together, see [Concepts](concepts.md).

## Activity log

An append-only JSONL file recording when each session started working and when it went idle, plus app start, heartbeat and stop records. Off by default, and it holds no terminal content. See [Activity log](features/activity-log.md).

## Agent

A directory with a role-prompt file at its root (`CLAUDE.md`, `AGENTS.md`, or `GEMINI.md`). The directory IS the agent's identity.

## Agents Agency

A community library of agent role templates at [@msitarzewski/agency-agents](https://github.com/msitarzewski/agency-agents). AC can download a validated cache of the catalog for offline use in the role-template picker.

## Archived project

A project AC still has registered but hides from the sidebar. Archiving moves the path between two lists in `settings.json` and touches no files. See [Project archiving](features/project-archiving.md).

## Brief

The plain-language description of a workgroup's goal. Lives at `<workgroup>/TASK.md` with YAML frontmatter title + freeform body.

## Coding agent

The CLI process that runs the LLM work: Claude Code, Codex, Gemini, or Pi. AC is **not** a coding agent.

## Config seed

A per-coding-agent option that copies a template config folder (for example `.claude`) into each replica at spawn, with AC path tokens substituted. See [Config seed](features/config-seed.md).

## ConPTY

Windows' native PTY API. AC uses ConPTY via [portable-pty](https://github.com/wez/wezterm/tree/main/pty) on Windows; Unix PTYs on Linux/macOS.

## Context alert

A notice AC injects into a workgroup's coordinator when a member's context usage crosses a threshold the team configured. It fires once per crossing and takes no action on the session. See [Context tracking](features/context-tracking.md).

## Context badge

The `CTX <n>%` badge on a session row, showing how much of that agent's context window is used. It appears only when the session's coding agent has a context pattern configured. See [Context tracking](features/context-tracking.md).

## Control-plane API

A local, opt-in HTTP server inside the daemon that lets a machine client speak the inter-agent control plane over a scoped token instead of the filesystem outbox. See [Control-plane API](features/control-plane-api.md).

## Coordinator

The single agent in a team that can send messages to any team member, edit the team's brief, and close other members' sessions. Members can only message peers they share a team with and their coordinator.

## Detached window

A session popped out of the main window into its own dedicated terminal window. Useful for long-running sessions you want to keep visible while doing other work.

## Idle badge

The `Nm` badge on a coordinator row showing whole minutes since the team was last active. Turns yellow, then red, as idle time grows. See [Session auto-close](features/session-auto-close.md).

## Idle detector

The PTY component that flags a session as idle after a configurable silence threshold (2.5 seconds by default). Drives the green-dot indicator in the sidebar.

## Inbox / outbox

Per-replica directories where an agent receives (`inbox/`) and stages outbound (`outbox/`) messages before the CLI delivers them.

## Loop (Project Loop)

A scheduled prompt delivered to a workgroup's coordinator on a cron expression. See [Concepts](concepts.md#project-loop).

## Messaging directory

`<workgroup>/messaging/`. Every inter-agent message in a workgroup lives here as a UTC-timestamped markdown file. Never auto-purged.

## Non-stop mode

A per-project group of workgroups AC watches, alerting you when one stops working. See [Concepts](concepts.md#non-stop-mode).

## Portable instance

A renamed copy of `agentscommander.exe` (with an `_<suffix>` like `agentscommander_team-a.exe`) that runs fully isolated: its own config directory, its own mutex, its own web port.

## Profile

A lettered launch variant (`A`, `B`, `C`, ...) of a coding agent: extra command parameters and environment variables layered on its base command. Resolved per agent and per session. See [Coding Agent Profiles](features/coding-agent-profiles.md). Distinct from a tuned `CodingAgentKind` integration and from path placeholders, which also use the word "profile".

## Profile matrix

The grid of profiles: one row per coding agent, one column per letter, each cell holding that variant's params, env, and notes. Stored in `settings.json` under `codingAgentProfiles`.

## Project (AC project)

A folder containing a Project AC Root (`.ac/`). AC manages all agents, teams, and workgroups under that root.

## Project AC Root

The `.ac/` container folder at the project root holding origin agent state, configuration, and team structures. *(Note: Previously called "Workspace" or "AC Workspace". CLI flags and internal files may still contain "workspace" during the transition).*

## PTY

Pseudo-terminal. Each AC session runs in a real PTY (ConPTY on Windows, Unix PTY elsewhere) — not a command runner. Full vt100, interactive prompts, color, the lot.

## Raise hand

A coordinator's request for your attention, raised by the agent itself through the `raise-hand` CLI verb and shown on its rail entry and replica row. It survives restarts and is cleared only by real user input to that session. See [Sidebar guide](features/sidebar-guide.md#raise-hand).

## Replica

A working copy of an agent inside a workgroup at `wg-<N>-<team>/__agent_<name>/`. Replicas share the canonical agent matrix's `memory/`, `plans/`, `skills/`, and `Role.md`, but have their own scratch space, inbox, outbox, and session artifacts.

## Resource watchdog

The part of the resource monitor that compares each agent group against the configured memory thresholds. It either surfaces the state or terminates the offending session's process group, depending on `resourceWatchdogAction`. See [Resource monitor](features/resource-monitor.md).

## Role template

A reusable agent definition (prompt + optional skills) that AC clones when you create a new agent. Templates come from the downloaded Agency cache or from your local `agent-templates/` directory.

## Root Agent

A Project AC Root-level coordinator that can route messages between coordinators of different teams. Identity-verified WG coordinators see it as a synthetic `agentscommander://root-agent` peer.

## Session

A running process bound to one agent directory and one coding-agent CLI. Sessions live in the sidebar with status dots: cyan active, blue running, green waiting (ready for your input), amber pending, red exited, gray idle, translucent offline.

## Session auto-close

Automatic shutdown of an idle team (coordinator plus agent-owned sessions) after a timeout (60 minutes by default). Ad-hoc shells are never auto-closed. See [Session auto-close](features/session-auto-close.md).

## Settings tab

The five tabs in the Settings modal: **General**, **Coding Agents**, **Resources**, **Watchers**, **Integrations**.

## Spec Board

A window holding one Mermaid file, with a live diagram beside its source. See [Concepts](concepts.md#spec-board).

## Team

A coordinator + worker agents working toward shared goals. Defined in `.ac/_team_<name>/config.json`.

## Telegram bridge

Per-session attachment to a Telegram bot. PTY output streams to Telegram (filtered, rate-limited, vt100-cleaned); Telegram messages stream into the PTY. See [Telegram bridge](features/telegram-bridge.md).

## Token (session token)

A UUID issued per session. The CLI shape-validates it; the daemon mailbox identity-validates it. Live token refresh without respawn is not supported.

## Voice-to-text

Push-to-talk transcription via the Google Gemini API. Dictate a prompt; AC writes the transcription into the session's PTY. See [Voice-to-text](features/voice-to-text.md).

## Watcher

A root-level pattern AC matches against agent terminal output, reaching every configured agent unless a selector narrows it. See [Concepts](concepts.md#watcher).

## Workspace (Deprecated)

*Deprecated alias.* See **Project AC Root**.

## Workspace (kept exceptions)

Three "workspace" spellings survive the rename to **Project AC Root** on purpose. Each belongs to someone else's vocabulary (the Rust toolchain, the Codex CLI, the container ecosystem) rather than to AC's product vocabulary, so renaming one would break a build, a flag, or an image instead of clarifying anything. **Do not rename them.** If an audit turns them up, the migration is not half-finished: these are the documented exceptions (epic #1366).

- **`[workspace]` and `--workspace`, Cargo's vocabulary** (#1372). The root `Cargo.toml` declares the Rust workspace with this key, and `--workspace` is how a Cargo command (`cargo clippy --workspace`, for example) targets every member crate. Cargo has no alternative spelling; changing the key breaks the build.
- **`workspace-write`, the Codex CLI's vocabulary** (#1373). A value of Codex's `--sandbox` flag, as in `codex --sandbox workspace-write`. AC only passes it through: it arrives in a coding agent's profile command, and AC's own UI shows it just as an example placeholder in Settings. Renaming the value breaks the flag.
- **`/workspace`, the container ecosystem's convention** (#1371). The path inside AC's Docker container where the replica root is bind-mounted, defined as `DEFAULT_CONTAINER_WORKDIR` in `src-tauri/src/pty/container_runtime.rs`. Docker does not impose the path, but devcontainers, Cloud Build and GitPod all mount the checkout at `/workspace`, and tools running inside a container expect to find it there. Kept by user decision of 2026-08-19.

## Workgroup

A team's activation for a specific task. Lives at `.ac/wg-<N>-<team>/` with replicas of every team member.

## `wg-<N>-<team>`

The on-disk name of a workgroup directory. The integer `<N>` is sequential per project; `<team>` is the team's display name.
