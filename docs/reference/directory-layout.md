# Directory layout

For anyone who needs to know where AgentsCommander keeps its on-disk data, which files are shared with the team, and which are per-instance state you must never share.

A new entity directory is always `room-<N>-<team>`. Every `wg-*` directory that already exists keeps its name and stays fully supported; nothing on disk is renamed or converted. The CLI names `room`, `purge-room` and `--room` are canonical, and `workgroup`, `purge-wg`, `--wg` and `--workgroup` remain accepted as deprecated aliases that a later release will remove.

AgentsCommander keeps its on-disk data in two distinct trees:

| Tree | Location | Scope |
|---|---|---|
| Project `.ac/` | `<project>/.ac/` | Shared team and tool configuration. Version it in its own [Agents config repo](../glossary.md#agents-config-repo) (recommended), track it inside a work repo, or leave it untracked |
| Application config dir | Selected by the exact binary version; see below | Machine-local application state, never shared |

In the deployment documented below, the adjacent candidate is selected: the binary is `agentscommander_ac2.exe` beside the project root, so the two trees are `D:\0_repos\AgentsCommander_iac\.ac\` and `D:\0_repos\AgentsCommander_iac\.agentscommander_ac2\`. This example describes that deployment, not every package layout.

## The config-dir selection rule

Production resolves the application config directory once per process (`src-tauri/src/config/mod.rs`, `resolve_instance_location`), but the resolver changed after the latest published release:

- **Published `v0.30.3`:** when `current_exe()` has a parent and stem, AC immediately selects `<native-executable-folder>/.<native-executable-stem>`. It uses `$HOME/.agentscommander-new` only when those path parts cannot be derived. The release build has no public `AGENTSCOMMANDER_CONFIG_DIR`, `portable.txt`, writability probe, or read-only-candidate fallback.
- **Unpublished `main`:** a nonblank public override wins. Otherwise AC checks the adjacent `portable.txt` and probes writability. A marked failure stops startup; an unmarked, conclusively unwritable candidate uses the identity-specific home fallback; indeterminate results stop startup.
- **Any other release:** inspect its exact tag and apply only the behavior found there. Do not infer a cutoff between the two snapshots above.

Debug `v0.30.3` builds may use the internal `AGENTSCOMMANDER_TEST_CONFIG_DIR`; that is not a public release override. See [Portable instances](../features/portable-instances.md#config-directory-rule) for the full operational rule, including AppImage behavior.

These examples apply only after the adjacent candidate is selected:

```text
C:\tools\agentscommander.exe        ->  C:\tools\.agentscommander\
C:\tools\agentscommander_ac2.exe    ->  C:\tools\.agentscommander_ac2\
```

- The adjacent candidate's stem comes from the running executable only. Renaming changes that candidate. Only a resolver verified to include the public environment variable can override it, and only a resolver verified to include write probes tests candidate writability before selection.
- Replica agent directories inside rooms always follow the executable-stem naming, `.<stem>` (example: `__agent_dev-rust/.agentscommander_ac2/`); that local name is independent of the application's selected config location.

The rule has two consequences:

- `.ac/` is shared: version it so the team gets the same agents, teams, and tool configuration; see the layouts in the table above.
- The selected application config dir is machine-local: never commit or share it. It holds tokens, sessions, logs, and other local state. AC writes a `.gitignore` inside it so those files stay out of git when the selected path is inside a repository.

## `.ac/` (shared across the team)

The project-scoped tree. AC creates and maintains it, and the recommended layout versions it in git; see the layouts in the table above. In this deployment the project git tracks 692 files under `.ac/` and none inside the instance dir. Unless a row says otherwise, everything here is shared, and tracked wherever `.ac/` is tracked.

### Top-level files

| Entry | What it is | Notes |
|---|---|---|
| `Context.AgentsCommander.md` | Project context template (seed scope `context:agentscommander`) | Seeded by AC |
| `Context.coordinator.md` | Orchestrator context template (seed scope `context:coordinator`) | Seeded by AC |
| `Context.coordinator.md.bak` | Previous version of the orchestrator template, kept when AC refreshes it | Written by AC on refresh |
| `.agentscommander-context-templates.json` | Seeded-template state: per-template version and content hashes | Written by AC |
| `seed-manifest.toml` | Seed manifest: inventory of every file AC last seeded into `.ac` | Written by AC; see [Seed manifest](../features/seed-manifest.md) |
| `.seed-manifest.lock` | Write lock for the seed manifest | Written by AC; gitignored |
| `.gitignore` | AC-maintained ignore rules for this tree (`room-*/`, lock files; un-ignores `seed-manifest.toml`) | Written by AC at project discovery |
| `project-settings.json` | Project settings: agent catalog overrides, groups, and project-level configuration | Written by AC |
| `default.claude/`, `default.codex/` | Default config-folder masters (`default` + the tool's dotfolder) that config seed copies into replicas | Written by AC; see [Config seed](../features/config-seed.md) |
| `default.claude.archived-20260710-000519/` | Timestamped archive of a previous `default.claude` master | No writer in the current source; treat as legacy or hand-placed |
| `.vscode/`, `prueba.txt`, `wg-2-dev-rust-to-wg2-tech-lead-revised-scc87-draft-blocker.md` | Hand-created files observed in this deployment | AC neither creates, tracks, nor overwrites them |

### Directories

| Entry | What it is |
|---|---|
| `_agent_<name>/` | Agent matrix: one directory per agent, holding `Role.md`, `config.json`, `memory/`, `memory_YYYYMMDD_hhmmss/` (rotated memory archives), `plans/`, and `skills/`. See [Agent Matrix conventions](../agent-matrix-conventions.md), and see [Agent Matrix conventions §11](../agent-matrix-conventions.md#11-agent-memory-rotation-at-spawn) for how the archives are made |
| `_team_<name>/` | Team definitions: `config.json` (members, orchestrator, repos) and `conventions.md` |
| `room-<N>-<name>/` | Rooms: `__agent_<name>/` replica directories, `messaging/` (inter-agent message files), `repo-*/` [work repo](../glossary.md#work-repo) clones, `TASK*.md` briefs. Project-scoped and shared, but gitignored (`room-*/`) because the `repo-*` folders are their own git repositories |
| `coding-agents/` | Coding-agent catalog: `agents.json` (manifest) and `_seed/` (per-tool default config-folder masters). Seeded per registered project; this is the copy AC reads and writes |
| `competitions/` | Competition packages, one folder per competition with a `MANIFEST.md`. No writer in the current source; treat as hand-managed |

## `.agentscommander_ac2/` (per-instance, never shared)

This deployment's selected machine-local application state. Never commit or share it. The inventory below reflects this adjacent-selection deployment and is cross-checked against the `src-tauri/src/` writers.

### Files

| Entry | What it is | Source |
|---|---|---|
| `settings.json` | Per-instance settings | `config/settings.rs` |
| `settings.json.lock` | Write lock for settings writers | `config/local_config_io.rs` |
| `settings.pre-384-v1.json` | Pre-v384 settings backup taken during the settings migration | `config/settings.rs` |
| `sessions.json` | Session registry | `config/sessions_persistence.rs` |
| `activity.jsonl` | Activity log, see [Activity log](../features/activity-log.md) | `config/activity_log.rs` |
| `coordinator_clocks.json` | Orchestrator clock state | `config/coordinator_clocks.rs` |
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
| `coding-agents/` | Legacy catalog location, kept as a read and seed source only. Since #1318 the catalog AC reads and writes is the project's `.ac/coding-agents/`; nothing is written here |
| `context-cache/` | Rendered session contexts (`ac-context-*.md`) |
| `pty-input-locks/` | PTY input serialization locks |
| `git-guard/` | Windows git guard shim (`git.cmd`, `git-guard.ps1`) that wraps git for guarded subprocesses |
| `agent-templates/` | User-defined role templates; `README.md` explains the format |
| `agency-agents_templates/` | Built-in agency role-template catalog, one folder per discipline |
| `ac-root-agent/` | Root-agent replica: `Role.md`, `AGENTS.md` (session context), `config.json`, `memory/`, `plans/`, `skills/`, `inbox/`, `outbox/`, `messaging/`, and a nested `.<stem>/` config dir with its own outbox and response folders |

## Where the seed manifest tracks seeded files

The seed manifest at `.ac/seed-manifest.toml` records every file AC seeded into `.ac`, one row per project-relative logical destination: the project context templates (`.ac/Context.AgentsCommander.md`, `.ac/Context.coordinator.md`) and the replica config folders (rows under `config:<dest>` scopes such as `__agent_<name>/.claude/`). `.seed-manifest.lock` serializes the writes. See [Seed manifest](../features/seed-manifest.md) for the schema and [Config seed](../features/config-seed.md) for what gets copied.

The manifest never tracks the selected application config dir. Under the unpublished `main` resolver, a public override can place that directory anywhere, including under a project tree, but it remains machine-local state outside seed-manifest ownership. Published `v0.30.3` has no public override.

## Shared vs per-instance in one rule

- `<project>/.ac/`: shared. Version it in its own repo (recommended), track it inside a work repo, or leave it untracked; see the layouts in the table at the top of this page.
- The active selected application config directory: machine-local, never commit or share. Tokens, sessions, logs, and the coding-agent catalog live there; its path follows the selection rule above.

## Cross-references

- [Seed manifest](../features/seed-manifest.md): what AC seeds into `.ac` and how the manifest records it
- [Config seed](../features/config-seed.md): how replica config folders are seeded from the masters
- [Portable instances](../features/portable-instances.md): the config-dir rule and instance isolation
- [Settings reference](settings.md): the per-instance `settings.json`
- [Agent Matrix conventions](../agent-matrix-conventions.md): the `_agent_*` and `room-*` layout
- [Architecture map](architecture.md): where the code that manages these trees lives
