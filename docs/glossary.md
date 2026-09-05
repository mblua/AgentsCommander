# Glossary

Quick alphabetical reference. For richer explanations of how the pieces fit together, see [Concepts](concepts.md).

A note on one renamed term: what these docs call a **Room** was called a Workgroup. New entities are created as `room-<N>-<team>`; existing `wg-*` directories keep their name and stay fully supported; and `workgroup`, `purge-wg`, `--wg` and `--workgroup` are still accepted as deprecated CLI aliases until a later release removes them.

## Activity log

An append-only JSONL file recording when each session started working and when it went idle, plus app start, heartbeat and stop records. Off by default, and it holds no terminal content. See [Activity log](features/activity-log.md).

## Agent

A directory with a role-prompt file at its root (`CLAUDE.md` or `AGENTS.md`). The directory IS the agent's identity.

## Agent Matrix

An agent's canonical directory at `.ac/_agent_<name>/`, holding `Role.md` plus the `inbox/`, `outbox/`, `memory/`, `plans/`, and `skills/` folders AC creates on first use. It is the single source of truth for that agent's persistent knowledge: a replica reads `memory/`, `plans/`, `skills/`, and `Role.md` from here and keeps only scratch, inbox, outbox, and session artifacts of its own. See [Agent Matrix conventions](agent-matrix-conventions.md).

## Agents Agency

A community library of agent role templates at [@msitarzewski/agency-agents](https://github.com/msitarzewski/agency-agents). AC can download a validated cache of the catalog for offline use in the role-template picker.

## Agents config repo

The git repository that versions a project's **Project AC Root** (`.ac/`): the origin agent matrices (`_agent_<name>/`) with their `Role.md`, `skills/`, `memory/`, and `plans/`, the team definitions (`_team_<name>/config.json`), and `seed-manifest.toml`. Giving `.ac/` a repository of its own is the recommended layout, because agent configuration then gets its own history, review, and rollback, and a checkout of the product code never drags agent state along with it. Two other layouts work and are supported: track `.ac/` inside an existing **Work repo**, which is simplest when the project is a single repo, or do not track it at all, which leaves it local with no history and nothing to share. All three keep rooms out of git: `.ac/.gitignore` must exclude `room-*/`, because the `repo-*` clones inside a room are independent git repositories nested in the parent tree and parent-repo operations corrupt them. See [Project AC Root](#project-ac-root), [Work repo](#work-repo), and [Agent Matrix conventions](agent-matrix-conventions.md).

## Archived project

A project AC still has registered but hides from the sidebar. Archiving moves the path between two lists in `settings.json` and touches no files. See [Project archiving](features/project-archiving.md).

## Brief

The plain-language description of a room's goal. Lives at `<room>/TASK.md` with YAML frontmatter title + freeform body.

## Coding agent

The CLI process that runs the LLM work: Claude Code, Codex, Antigravity, or Pi. AC is **not** a coding agent.

## Config seed

A per-coding-agent option that copies a template config folder (for example `.claude`) into each replica at spawn, with AC path tokens substituted. See [Config seed](features/config-seed.md).

## ConPTY

Windows' native PTY API. AC uses ConPTY via [portable-pty](https://github.com/wez/wezterm/tree/main/pty) on Windows; Unix PTYs on Linux/macOS.

## Container coding agent

A coding agent AC launches under its **Container** runtime instead of as a local host process. Host login reuse is on by default, so a container Claude Code session starts signed in with no interaction. Repos are mounted inside the container at `/repos/...`; if a repo is not listed as admissible for the room, it will not be mounted. See [Container coding agents](features/container-coding-agents.md).

## Context alert

A notice AC injects into a room's orchestrator when a member's context usage crosses a threshold the team configured. It fires once per crossing and takes no action on the session. See [Context tracking](features/context-tracking.md).

## Context badge

The `CTX <n>%` badge on a session row, showing how much of that agent's context window is used. It appears only when the session's coding agent has a context pattern configured. See [Context tracking](features/context-tracking.md).

## Control-plane API

A local, opt-in HTTP server inside the daemon that lets a machine client speak the inter-agent control plane over a scoped token instead of the filesystem outbox. See [Control-plane API](features/control-plane-api.md).

## Detached window

A session popped out of the main window into its own dedicated terminal window. Useful for long-running sessions you want to keep visible while doing other work.

## Harness

`agentscommander harness`, a policy-controlled entry point for coding-agent OS command execution, with an argv form, a `--raw-command` form, and a JSON Lines audit log. Phase 1 is deliberately obedient: agents are expected to call it, but it does not prevent direct shell execution and provides no strong sandboxing. See [Harness roadmap](harness-roadmap.md) and [`harness`](reference/cli.md#harness).

## Idle badge

The `Nm` badge on an orchestrator row showing whole minutes since the team was last active. Turns yellow, then red, as idle time grows. See [Session auto-close](features/session-auto-close.md).

## Idle detector

The PTY component that flags a session as idle after a configurable silence threshold (2.5 seconds by default). Drives the green-dot indicator in the sidebar.

## Inbox / outbox

Per-replica directories where an agent receives (`inbox/`) and stages outbound (`outbox/`) messages before the CLI delivers them.

## Injected messages

The operator-editable templates AC injects into a session's PTY, each identified by an id such as `context-alert`. They live in `injected-messages.toml` in the version-selected configuration directory, with `injected-messages.default.toml` as the canonical reference set. `agentscommander injected-messages reseed` resets one id or every id back to the defaults the binary ships, writing a timestamped `.bak-` copy first and preserving comments, unknown keys, entry order, and untargeted entries. See [`injected-messages`](reference/cli.md#injected-messages).

## Loop (Project Loop)

A scheduled prompt delivered to a room's orchestrator on a cron expression. See [Concepts](concepts.md#project-loop).

## Messaging directory

`<room>/messaging/`. Every inter-agent message in a room lives here as a UTC-timestamped markdown file. Never auto-purged.

## Non-stop mode

A per-project group of rooms AC watches, alerting you when one stops working. See [Concepts](concepts.md#non-stop-mode).

## Orchestrator

The single agent in a team that can send messages to any team member, edit the team's brief, and close other members' sessions. Members can only message peers they share a team with and their orchestrator.

## Origin agent

A project-level agent, one `_agent_<name>/` directory under `.ac/`, shown in the sidebar's **AGENTS** section. Origin agents are the canonical definitions; every room agent is a replica of one. Contrast with **Replica**, the per-room working copy. See [Agent Matrix conventions](agent-matrix-conventions.md).

## Portable instance

A renamed raw executable (with an `_<suffix>` such as `agentscommander_team-a.exe`) verified to select a distinct adjacent config directory, plus its own mutex and web port. Published `v0.30.3` selects adjacency without consulting `portable.txt`; the newer unpublished `main` resolver can use the marker to fail closed. Project `.ac/` state is still shared when two instances register the same project.

## Privileged PTY input

`send --pty-input` or `send --pty-input-stdin`, which submits 1 to 65,536 bytes of validated exact UTF-8 text to one already trusted coding-agent PTY. It never passes the accepted value to a host or container shell evaluator, command line, environment variable, or path. Only three routes are authorized: a live identity-verified room orchestrator targeting one verified non-orchestrator member of its own room, a live local Root Agent targeting one verified room orchestrator, and a container orchestrator using the dedicated API helper. `Queued` means durable admission only; only `Injected` means the backend accepted the write. See [Privileged exact PTY input](reference/cli.md#privileged-exact-pty-input).

## Profile

A lettered launch variant (`A`, `B`, `C`, ...) of a coding agent: extra command parameters and environment variables layered on its base command. Resolved per agent and per session. See [Coding Agent Profiles](features/coding-agent-profiles.md). Distinct from a tuned `CodingAgentKind` integration and from path placeholders, which also use the word "profile".

## Profile matrix

The grid of profiles: one row per coding agent, one column per letter, each cell holding that variant's params, env, and notes. Stored in `settings.json` under `codingAgentProfiles`.

## Project (AC project)

A folder containing a Project AC Root (`.ac/`). AC manages all agents, teams, and rooms under that root.

## Project AC Root

The `.ac/` container folder at the project root holding origin agent state, configuration, and team structures. *(Note: Previously called "Workspace" or "AC Workspace". CLI flags and internal files may still contain "workspace" during the transition).*

## PTY

Pseudo-terminal. Each AC session runs in a real PTY (ConPTY on Windows, Unix PTY elsewhere) — not a command runner. Full vt100, interactive prompts, color, the lot.

## Purge (`purge-room`)

`agentscommander purge-room`, which destroys every session of every peer in the caller's own room. It is orchestrator-only and scoped to that one room: the caller must be the identity-verified orchestrator of the room, the master or root token does not bypass the check, and cross-room purge is not supported. The busy gate is fail-closed, so if any in-scope peer has produced printable output within `--quiet-period-ms` the command purges nobody and exits 3. The caller itself and the Root Agent are never purged. See [`purge-room`](reference/cli.md#purge-room).

## Raise hand

An orchestrator's request for your attention, raised by the agent itself through the `raise-hand` CLI verb and shown on its rail entry and replica row. It survives restarts and is cleared only by real user input to that session. See [Sidebar guide](features/sidebar-guide.md#raise-hand).

## Remote web UI

The embedded HTTP and WebSocket server inside the running app that serves the AgentsCommander interface to a browser: the same sidebar, the same terminals, the same input, over a WebSocket transport instead of the desktop IPC. It is off by default, bound to `127.0.0.1`, and configured by the `webServer*` settings. It is not the **Control-plane API**, which is a separate opt-in listener with its own port, bind address, and tokens. See [Remote web UI](features/remote-web-ui.md).

## Replica

A working copy of an agent inside a room at `room-<N>-<team>/__agent_<name>/`. Replicas share the canonical agent matrix's `memory/`, `plans/`, `skills/`, and `Role.md`, but have their own scratch space, inbox, outbox, and session artifacts.

## Resource watchdog

The part of the resource monitor that compares each agent group against the configured memory thresholds. It either surfaces the state or terminates the offending session's process group, depending on `resourceWatchdogAction`. See [Resource monitor](features/resource-monitor.md).

## Role template

A reusable agent definition (prompt + optional skills) that AC clones when you create a new agent. Templates come from the downloaded Agency cache or from your local `agent-templates/` directory.

## Room

A team's activation for a specific task. Lives at `.ac/room-<N>-<team>/` with replicas of every team member.

## `room-<N>-<team>`

The on-disk name of a room directory. The integer `<N>` is sequential per project; `<team>` is the team's display name.

## Root Agent

A Project AC Root-level orchestrator that can route messages between orchestrators of different teams. Identity-verified Room orchestrators see it as a synthetic `agentscommander://root-agent` peer.

## Seed manifest

`<project>/.ac/seed-manifest.toml`, a git-diffable text file recording every project-scoped file AC last published into `.ac/`, one row per project-relative logical destination, each carrying the UTC time of that file's most recent successful publication. It is a diagnostic inventory, not an ownership ledger: it never grants ownership and never authorizes AC to overwrite, repair, or delete anything. See [Seed manifest](features/seed-manifest.md).

## Self-handoff

A token-authorized two-phase operation on the caller's own session that carries context across a reset. You write `SELF-HANDOFF.md` first; phase 1 waits for 30 seconds of sustained idle and acts, then phase 2 waits a fresh 30 seconds of idle, archives the handoff into `self-clear/`, and injects a resume prompt naming that archive. Three verbs differ in what phase 1 does: `self-handoff-and-clear` injects the provider's logical-clear text in place, `self-handoff-and-switch` respawns the session on a requested coding agent or profile, and `self-handoff-and-restart` respawns it on the same coding agent and profile letter it is already running. `self-handoff-and-switch` and `self-handoff-and-restart` additionally require `SELF-HANDOFF.md` in your own root and reject the request if it is missing, and each states a caller scope: Room replicas only for `-and-switch`, and every session that owns a token and runs a configured coding agent for `-and-restart`. `self-handoff-and-clear` states no caller scope, and both phases are documented as best-effort for `-and-clear` and `-and-restart` only. See [`self-handoff-and-clear`](reference/cli.md#self-handoff-and-clear).

## Session

A running process bound to one agent directory and one coding-agent CLI. Sessions live in the sidebar with status dots: cyan active, blue running, green waiting (ready for your input), amber pending, red exited, gray idle, translucent offline.

## Session auto-close

Automatic shutdown of an idle team (orchestrator plus agent-owned sessions) after a timeout (60 minutes by default). Ad-hoc shells are never auto-closed. See [Session auto-close](features/session-auto-close.md).

## Settings tab

The five tabs in the Settings modal: **General**, **Coding Agents**, **Resources**, **Watchers**, **Integrations**.

## Spec Board

A window holding one Mermaid file, with a live diagram beside its source. See [Concepts](concepts.md#spec-board).

## Team

An orchestrator + worker agents working toward shared goals. Defined in `.ac/_team_<name>/config.json`.

## Telegram bridge

Per-session attachment to a Telegram bot. PTY output streams to Telegram (filtered, rate-limited, vt100-cleaned); Telegram messages stream into the PTY. See [Telegram bridge](features/telegram-bridge.md).

## Terminal snapshot

One live backend terminal viewport read as versioned JSON or a deterministic PNG by an authorized Root Agent or room orchestrator. The read never wakes, spawns, focuses, selects, resizes, writes to, or captures OS pixels from the target, so it is how you see a hidden, minimized, detached, or never-mounted session. It is not a transcript, a frontend screenshot, or a request to wake an agent, and it stays unavailable until you enable it in **Settings > General**. See [Terminal snapshots](features/terminal-snapshots.md) and [`terminal-snapshot`](reference/cli.md#terminal-snapshot).

## Token (session token)

A UUID issued per session. The CLI shape-validates it; the daemon mailbox identity-validates it. Live token refresh without respawn is not supported.

## Voice-to-text

Push-to-talk transcription via the Google Gemini API. Dictate a prompt; AC writes the transcription into the session's PTY. See [Voice-to-text](features/voice-to-text.md).

## Wake (delivery mode)

`send --mode wake`, the default and today the only delivery mode. A file message injects into the target's PTY and can spawn or respawn a persistent session; logical PTY actions are capability-gated and idle-gated and can be terminally rejected before spawn. A first wake against a cold peer only spawns the session, and the message is delivered on a second send. See [Inter-agent messaging](agents/inter-agent-messaging.md).

## Watcher

A root-level pattern AC matches against agent terminal output, reaching every configured agent unless a selector narrows it. See [Concepts](concepts.md#watcher).

## Window capture

A PNG of exactly one live native desktop window, captured by the canonical decimal `window_id` that `window-list` prints. Windows only: the CLI verbs and the HTTP route are compiled only on Windows and are deliberately absent on other targets. It writes the captured OS pixels with no redaction, which is what separates it from a **Terminal snapshot**. See [Window capture](features/window-capture.md) and [`window-screenshot`](reference/cli.md#window-screenshot).

## Work repo

A git repository holding the code the agents actually edit. A room receives its own clone of each work repo, placed beside the `__agent_*` replicas inside the room directory. The clone is named with the `repo-` prefix (`repo-<name>`), and that prefix is what the write-access rules key on: the golden rule allows write access only to `repo-*` folders. It is distinct from the [Agents config repo](#agents-config-repo), which versions agent configuration rather than product code. See [Agent Matrix conventions](agent-matrix-conventions.md).

## Workspace (Deprecated)

*Deprecated alias.* See **Project AC Root**.

## Workspace (kept exceptions)

Three "workspace" spellings survive the rename to **Project AC Root** on purpose. Each belongs to someone else's vocabulary (the Rust toolchain, the Codex CLI, the container ecosystem) rather than to AC's product vocabulary, so renaming one would break a build, a flag, or an image instead of clarifying anything. **Do not rename these three.** Any other `workspace` occurrence is not a fourth exception: it is legacy pending rename under epic #1366, not vocabulary to imitate.

- **`[workspace]` and `--workspace`, Cargo's vocabulary** (#1372). The root `Cargo.toml` declares the Rust workspace with this key, and `--workspace` is how a Cargo command targets every member crate: AC's CI gates every pull request on `cargo clippy --workspace --all-targets`. Cargo has no alternative spelling; changing the key breaks the build.
- **`workspace-write`, the Codex CLI's vocabulary** (#1373). A value of Codex's `--sandbox` flag, as in `codex --sandbox workspace-write`. AC only passes it through: it arrives in a coding agent's profile command, and AC's own UI shows it just as an example placeholder in Settings. Renaming the value breaks the flag.
- **`/workspace`, the container ecosystem's convention** (#1371). The path inside AC's Docker container where the replica root is bind-mounted, defined as `DEFAULT_CONTAINER_WORKDIR` in `src-tauri/src/pty/container_runtime.rs`. Docker does not impose the path, but Cloud Build and GitPod both mount the checkout at `/workspace`, and tools running inside a container expect to find it there. Kept by user decision of 2026-08-19.
