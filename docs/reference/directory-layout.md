# Directory layout

For anyone who needs to know where AgentsCommander keeps its on-disk data, which files are shared with the team, and which are per-instance state you must never share.

AgentsCommander keeps its on-disk data in two distinct trees:

| Tree | Location | Scope |
|---|---|---|
| Project `.ac/` | `<project>/.ac/` | Shared team and tool configuration, tracked in the project's git |
| Instance config dir | `<binary folder>/.<binary stem>/` | Per-instance state, never shared |

In this deployment the binary is `agentscommander_ac2.exe` sitting next to the project root, so the two trees are `D:\0_repos\AgentsCommander_iac\.ac\` and `D:\0_repos\AgentsCommander_iac\.agentscommander_ac2\`.

## The config-dir rule

The per-instance config directory lives next to the binary and is named after the binary's file stem with a leading dot (`src-tauri/src/config/mod.rs`, `resolve_instance_location`):

```
C:\tools\agentscommander.exe        ->  C:\tools\.agentscommander\
C:\tools\agentscommander_ac2.exe    ->  C:\tools\.agentscommander_ac2\
```

- The stem comes from the running executable only. Renaming the binary gives you a fresh, isolated instance; see [Portable instances](../features/portable-instances.md).
- If `current_exe()` is unavailable, AC falls back to `$HOME/<config-dir-name>`.
- Debug builds honor the `AGENTSCOMMANDER_TEST_CONFIG_DIR` override.
- Replica agent directories inside workgroups follow the same naming, `.<stem>` (example: `__agent_dev-rust/.agentscommander_ac2/`).

The rule has two consequences:

- `.ac/` is shared: commit it to the project's git so the team gets the same agents, teams, workgroups, and tool configuration.
- The instance dir is per-instance: never commit or share it. It holds tokens, sessions, logs, and machine-local state. AC writes a `.gitignore` inside it so those files stay out of git when the binary runs inside a repository.

## `.ac/` (shared, tracked in the project git)

The project-scoped tree. AC creates and maintains it, and the project commits it. In this deployment the project git tracks 692 files under `.ac/` and none inside the instance dir. Unless a row says otherwise, everything here is shared and tracked.

### Top-level files

| Entry | What it is | Notes |
|---|---|---|
| `Context.AgentsCommander.md` | Project context template (seed scope `context:agentscommander`) | Seeded by AC |
| `Context.coordinator.md` | Coordinator context template (seed scope `context:coordinator`) | Seeded by AC |
| `Context.coordinator.md.bak` | Previous version of the coordinator template, kept when AC refreshes it | Written by AC on refresh |
| `.agentscommander-context-templates.json` | Seeded-template state: per-template version and content hashes | Written by AC |
| `seed-manifest.toml` | Seed manifest: inventory of every file AC last seeded into `.ac` | Written by AC; see [Seed manifest](../features/seed-manifest.md) |
| `.seed-manifest.lock` | Write lock for the seed manifest | Written by AC; gitignored |
| `.gitignore` | AC-maintained ignore rules for this tree (`wg-*/`, lock files; un-ignores `seed-manifest.toml`) | Written by AC at project discovery |
| `project-settings.json` | Project settings: agent catalog overrides, groups, and project-level configuration | Written by AC |
| `default.claude/`, `default.codex/` | Default config-folder masters (`default` + the tool's dotfolder) that config seed copies into replicas | Written by AC; see [Config seed](../features/config-seed.md) |
| `default.claude.archived-20260710-000519/` | Timestamped archive of a previous `default.claude` master | No writer in the current source; treat as legacy or hand-placed |
| `.vscode/`, `prueba.txt`, `wg-2-dev-rust-to-wg2-tech-lead-revised-scc87-draft-blocker.md` | Hand-created files observed in this deployment | AC neither creates, tracks, nor overwrites them |

### Directories

| Entry | What it is |
|---|---|
| `_agent_<name>/` | Agent matrix: one directory per agent, holding `Role.md`, `config.json`, `memory/`, `memory_YYYYMMDD_hhmmss/` (rotated memory archives), `plans/`, and `skills/`. See [Agent Matrix conventions](../agent-matrix-conventions.md) |
| `_team_<name>/` | Team definitions: `config.json` (members, coordinator, repos) and `conventions.md` |
| `wg-<N>-<name>/` | Workgroups: `__agent_<name>/` replica directories, `messaging/` (inter-agent message files), `repo-*/` workgroup clones, `TASK*.md` briefs. Project-scoped and shared, but gitignored (`wg-*/`) because the `repo-*` folders are their own git repositories |
| `competitions/` | Competition packages, one folder per competition with a `MANIFEST.md`. No writer in the current source; treat as hand-managed |

## `.agentscommander_ac2/` (per-instance, never shared)

Per-user instantiation state next to the binary. Everything in this tree is per-instance: never commit it, never share it. The inventory below reflects this deployment and is cross-checked against the `src-tauri/src/` writers.

### Files

| Entry | What it is | Source |
|---|---|---|
| `settings.json` | Per-instance settings | `config/settings.rs` |
| `settings.json.lock` | Write lock for settings writers | `config/local_config_io.rs` |
| `settings.pre-384-v1.json` | Pre-v384 settings backup taken during the settings migration | `config/settings.rs` |
| `sessions.json` | Session registry | `config/sessions_persistence.rs` |
| `activity.jsonl` | Activity log | `config/activity_log.rs` |
| `coordinator_clocks.json` | Coordinator clock state | `config/coordinator_clocks.rs` |
| `daemon.pid` | PID of the running daemon; the CLI uses it to detect stale sessions | `config/daemon_pid.rs` |
| `master-token.txt` | CLI master token | `lib.rs` boot |
| `web-token.txt` | Web access token, separate from the master token | `lib.rs` boot |
| `app-outbox-path.txt` | Path to the current run's app outbox | `lib.rs` boot |
| `update-check.json` | Update-check cache | `update_check.rs` |
| `injected-messages.toml` | User-editable injected messages | `cli/injected_messages.rs` |
| `injected-messages.default.toml` | Reference copy of the injected-messages defaults | `cli/injected_messages.rs` |
| `.agentscommander-injected-messages.json` | Seeded state of the injected messages | `cli/injected_messages.rs` |
| `Context.root-agent.md` | Root-agent context template | `config/root_agent.rs` |
| `.agentscommander-context-templates.json` | Seeded-template state for the root-agent template | `config/seeded_context_templates.rs` |
| `.gitignore` | Ignore rules for this instance dir, for when the binary runs inside a git repo | `config/instance_gitignore.rs` |
| `api-message-bus.sqlite3` (+ `-shm`, `-wal`) | SQLite database backing the API message bus | `api/message_store.rs` |
| `app.log` | Main application log | `logging.rs` |
| `api-audit.log` | API audit log | `api/audit.rs` |
| `diag-raw.log`, `diag-sent.log` | Telegram Claude-watcher diagnostic captures | `telegram/claude_watcher/output.rs` |
| `telegram-bridge.log` | Telegram bridge log | telegram logging |

### Directories

| Entry | What it is |
|---|---|
| `instances/<uuid>/outbox/` | App outbox for the current run; AC removes stale instance dirs at boot |
| `coding-agents/` | Coding-agent catalog: `agents.json` (manifest) and `_seed/` (per-tool default config-folder masters) |
| `context-cache/` | Rendered session contexts (`ac-context-*.md`) |
| `pty-input-locks/` | PTY input serialization locks |
| `git-guard/` | Windows git guard shim (`git.cmd`, `git-guard.ps1`) that wraps git for guarded subprocesses |
| `agent-templates/` | User-defined role templates; `README.md` explains the format |
| `agency-agents_templates/` | Built-in agency role-template catalog, one folder per discipline |
| `ac-root-agent/` | Root-agent replica: `Role.md`, `AGENTS.md` (session context), `config.json`, `memory/`, `plans/`, `skills/`, `inbox/`, `outbox/`, `messaging/`, and a nested `.<stem>/` config dir with its own outbox and response folders |

## Where the seed manifest tracks seeded files

The seed manifest at `.ac/seed-manifest.toml` records every file AC seeded into `.ac`, one row per project-relative logical destination: the project context templates (`.ac/Context.AgentsCommander.md`, `.ac/Context.coordinator.md`) and the replica config folders (rows under `config:<dest>` scopes such as `__agent_<name>/.claude/`). `.seed-manifest.lock` serializes the writes. See [Seed manifest](../features/seed-manifest.md) for the schema and [Config seed](../features/config-seed.md) for what gets copied.

The manifest never tracks the per-instance dir: that state is outside `.ac` by construction.

## Shared vs per-instance in one rule

- `<project>/.ac/`: shared, commit it.
- `<binary folder>/.<binary stem>/`: per-instance, never commit or share. Tokens, sessions, logs, and the coding-agent catalog live there per machine and per binary.

## Cross-references

- [Seed manifest](../features/seed-manifest.md): what AC seeds into `.ac` and how the manifest records it
- [Config seed](../features/config-seed.md): how replica config folders are seeded from the masters
- [Portable instances](../features/portable-instances.md): the config-dir rule and instance isolation
- [Settings reference](settings.md): the per-instance `settings.json`
- [Agent Matrix conventions](../agent-matrix-conventions.md): the `_agent_*` and `wg-*` layout
- [Architecture map](architecture.md): where the code that manages these trees lives
