# Glossary

Quick alphabetical reference. For richer explanations of how the pieces fit together, see [Concepts](concepts.md).

## Agent

A directory with a role-prompt file at its root (`CLAUDE.md`, `AGENTS.md`, or `GEMINI.md`). The directory IS the agent's identity.

## Agents Agency

A community library of agent role templates at [@msitarzewski/agency-agents](https://github.com/msitarzewski/agency-agents). AC can download a validated cache of the catalog for offline use in the role-template picker.

## Brief

The plain-language description of a workgroup's goal. Lives at `<workgroup>/TASK.md` with YAML frontmatter title + freeform body.

## Coding agent

The CLI process that runs the LLM work: Claude Code, Codex, or Gemini. AC is **not** a coding agent.

## ConPTY

Windows' native PTY API. AC uses ConPTY via [portable-pty](https://github.com/wez/wezterm/tree/main/pty) on Windows; Unix PTYs on Linux/macOS.

## Coordinator

The single agent in a team that can send messages to any team member, edit the team's brief, and close other members' sessions. Members can only message peers they share a team with and their coordinator.

## Detached window

A session popped out of the main window into its own dedicated terminal window. Useful for long-running sessions you want to keep visible while doing other work.

## Idle detector

The PTY component that flags a session as idle after a configurable silence threshold (2.5 seconds by default). Drives the green-dot indicator in the sidebar.

## Inbox / outbox

Per-replica directories where an agent receives (`inbox/`) and stages outbound (`outbox/`) messages before the CLI delivers them.

## Messaging directory

`<workgroup>/messaging/`. Every inter-agent message in a workgroup lives here as a UTC-timestamped markdown file. Never auto-purged.

## Portable instance

A renamed copy of `agentscommander.exe` (with an `_<suffix>` like `agentscommander_team-a.exe`) that runs fully isolated: its own config directory, its own mutex, its own web port.

## Project (AC project)

A folder containing a Project AC Root (`.ac/`). AC manages all agents, teams, and workgroups under that root.

## Project AC Root

The `.ac/` container folder at the project root holding origin agent state, configuration, and team structures. *(Note: Previously called "Workspace" or "AC Workspace". CLI flags and internal files may still contain "workspace" during the transition).*

## PTY

Pseudo-terminal. Each AC session runs in a real PTY (ConPTY on Windows, Unix PTY elsewhere) — not a command runner. Full vt100, interactive prompts, color, the lot.

## Replica

A working copy of an agent inside a workgroup at `wg-<N>-<team>/__agent_<name>/`. Replicas share the canonical agent matrix's `memory/`, `plans/`, `skills/`, and `Role.md`, but have their own scratch space, inbox, outbox, and session artifacts.

## Role template

A reusable agent definition (prompt + optional skills) that AC clones when you create a new agent. Templates come from the downloaded Agency cache or from your local `agent-templates/` directory.

## Root Agent

A Project AC Root-level coordinator that can route messages between coordinators of different teams. Identity-verified WG coordinators see it as a synthetic `agentscommander://root-agent` peer.

## RTK (Rust Token Killer)

A CLI proxy that compresses verbose command outputs to cut LLM token consumption by 60–90%. AC auto-detects `rtk` on PATH and wires a Claude `PreToolUse` hook into managed agent directories. See [RTK integration](features/rtk-integration.md).

## Session

A running process bound to one agent directory and one coding-agent CLI. Sessions live in the sidebar with status dots: green waiting (ready for your input), blue running, amber pending, red exited, gray idle.

## Settings tab

The three tabs in the Settings modal: **General**, **Coding Agents**, **Integrations**.

## Team

A coordinator + worker agents working toward shared goals. Defined in `.ac/_team_<name>/config.json`.

## Telegram bridge

Per-session attachment to a Telegram bot. PTY output streams to Telegram (filtered, rate-limited, vt100-cleaned); Telegram messages stream into the PTY. See [Telegram bridge](features/telegram-bridge.md).

## Token (session token)

A UUID issued per session. The CLI shape-validates it; the daemon mailbox identity-validates it. Live token refresh without respawn is not supported.

## Voice-to-text

Push-to-talk transcription via the Google Gemini API. Dictate a prompt; AC writes the transcription into the session's PTY. See [Voice-to-text](features/voice-to-text.md).

## Workspace (Deprecated)

*Deprecated alias.* See **Project AC Root**.

## Workgroup

A team's active workspace for a specific task. Lives at `.ac/wg-<N>-<team>/` with replicas of every team member.

## `wg-<N>-<team>`

The on-disk name of a workgroup directory. The integer `<N>` is sequential per project; `<team>` is the team's display name.
