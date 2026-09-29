# File naming convention

For contributors who add, rename, or review an AgentsCommander config file. Check this page before you name a file, so that the name tells every reader who owns it, which layer wins, and whether it may be committed.

> **Status: implemented, except `agents.40.project.json`.**
> The product owner decided this convention on 2026-09-23 ([#2448](https://github.com/mblua/AgentsCommander/issues/2448)). The file-naming epic ([#2470](https://github.com/mblua/AgentsCommander/issues/2470)) shipped the renames below. AC now reads and writes only the new names. `agents.40.project.json` is reserved: its name exists in code, but no build reads or writes it yet.
> The migration is **forward only**: once a version renames your files, you cannot downgrade past it.

## The rule

```text
<name>.<NN>.<owner>[.no-git].<ext>
```

| Part | Meaning |
|---|---|
| `<name>` | The file family. It comes first so every layer of one family sorts together in a folder listing. |
| `<NN>` | Precedence. A higher number overrides a lower one. Numbers go in steps of 10 and mean the same layer in every folder. |
| `<owner>` | The word for that layer. It is always written next to its number, never alone. |
| `.no-git` | Only on files that must never be committed. A file without it may be tracked. |
| `<ext>` | The format, for example `json`. |

## Layers

| NN | Owner | Who writes it | Notes |
|---|---|---|---|
| 10 | `default` | AC | Shipped defaults. AC rewrites them; never edit them. |
| 20 | `remote` | AC | Downloaded by AC. |
| 30 | `instance` | You | Lives in the instance dir. |
| 40 | `project` | You | Shared with your team through git. |
| 50 | `personal` | You | This machine only, even when AC writes it for you from the UI. |

## State files

A state file is AC's working memory. No other layer competes with it, so it has no number:

```text
<name>.state.no-git.<ext>
```

## What gets renamed

Only the three layered families are renamed: `settings.*`, `blocking-menus.*` and `agents.*`.

Every other AC file keeps its name: logs, locks, pids, databases, queues, directories, context templates, RTK files, tokens, and `CLAUDE.md` / `AGENTS.md` (coding-agent CLIs read those by fixed name).

The 7 git-tracked instance entries also keep their names (`src-tauri/src/config/instance_artifacts.rs:651-693`):

- `Context.AgentsCommander.md`
- its backup glob `Context.AgentsCommander.md.retired-*.bak`
- `Context.root-agent.md`
- `ac-root-agent/`
- `agency-agents_templates/`
- `agent-templates/`
- `coding-agents/`

## The folder gives the scope

The name does not say the scope; the folder does. The instance dir (`.agentscommander*/`) holds instance files, and the project dir (`.ac/`) holds project files. Being inside `.ac/` does not mean a file is tracked: rooms are never committed. See [Directory layout](directory-layout.md).

## Old names and their new names

### `settings.*`

| Old name | Target |
|---|---|
| `settings.json` | `settings.30.instance.no-git.json` |
| `settings.local.json` | `settings.50.personal.no-git.json` |
| `settings.json.lock` | `settings.30.instance.no-git.json.lock` |
| `settings.backup.*.json` | `settings.30.instance.no-git.backup.*.json` |
| `settings.pre-*.json` | `settings.30.instance.no-git.pre-*.json` |

Lock and backup files follow the live file's name.

### `blocking-menus.*`

| Old name | Target |
|---|---|
| `settings-blocking-menus.json` | `blocking-menus.10.default.no-git.json` |
| `settings-blocking-menus.remote.json` | `blocking-menus.20.remote.no-git.json` |
| `settings-blocking-menus.local.json` | `blocking-menus.50.personal.no-git.json` |
| `blocking-menus-remote-check.json` | `blocking-menus.state.no-git.json` |

### `agents.*`

| Old name | Target |
|---|---|
| `.ac/coding-agents/agents.json` | `agents.10.default.json` (tracked, so no `.no-git`) |
| `agents.local.json` | `agents.50.personal.no-git.json` |
| (none) | `agents.40.project.json` (new, tracked) |
| (none) | `agents.30.instance.no-git.json` (new, in the instance dir) |
| `.agents.json.lock` | `.agents.10.default.json.lock` |

Never renamed: `agents.migration-v1.backup.json` and `.agents.migration-v1.json`. An interrupted catalog migration resumes by those exact names.

## Names phase B added

These names are not in the three tables above. Two replace an older name, and two are new files.

| File | Where | What it holds |
|---|---|---|
| `.ac/settings.50.personal.no-git.json` | Each project's `.ac/` | Project settings, formerly `.ac/project-settings.json`. Personal: never committed. |
| `_loop_*/loop.state.no-git.json` | Each Loop directory under `.ac/` | The Loop's scheduler state, formerly `state.json` beside its `config.toml`. |
| `agents.30.instance.no-git.json` | The instance dir | Coding agents (`agents`) and their profiles (`codingAgentProfiles`), moved out of the instance settings file. |
| `naming-migration.state.no-git.json` | The instance dir | The migration journal: what the migration renamed and set aside. It also marks the migration as done. |

## Planned splits

| From | To |
|---|---|
| Window geometry and zoom in `settings.30.instance.no-git.json` | `window.state.no-git.json` |
| Agent `config.json` | Decisions stay in `config.json` (tracked); state moves to `config.state.no-git.json` |

## The model: blocking-menu precedence

Blocking menus already resolve layers the way this convention generalizes (`src-tauri/src/config/settings.rs:1698-1714`). The first match wins:

1. Per-agent local entries
2. Per-command local entries
3. Remote entries
4. Shipped entries

## Migration policy

- **When it runs.** AC migrates the instance dir at startup, before it reads its settings; each registered project is migrated when AC loads it. The migration is one-shot and idempotent: once a directory is done, later starts skip it. A project that did not finish is retried on the next startup.
- **What it records.** It writes the journal `naming-migration.state.no-git.json` in the instance dir, and logs one summary line of what it renamed. After that, the code knows only the new names.
- **It never deletes or overwrites.** Each file is renamed in place. When both the old and the new name exist, the **new** file wins and AC keeps running. The old file is renamed beside it to `<old name>.deprecated-<n>.no-git`, which the ignore rules keep out of Git. No dialog appears. If you wanted the older file's contents, open that `.deprecated-<n>.no-git` file.
- **Lock sidecars.** After the rename, AC writes a new lock sidecar beside each renamed file, for example `.agents.10.default.json.lock`. The old sidecar, such as `.agents.json.lock`, stays on disk as an inert leftover that AC ignores.
- **When it stops AC.** Three outcomes stop AC at startup with a message, and none of them is repaired by guesswork:
  - Another process holds the migration lock for longer than five seconds and has not finished. Retry once that process has exited.
  - An I/O error while renaming. Fix the permission or disk problem the message names.
  - A migration journal that cannot be read, or that carries an unknown version. The message names the file; send it to support.

  In every case AC starts normally on the next run once the named condition is gone. Both names being present is **not** one of these outcomes.
- **Forward only.** You cannot downgrade past the version that migrates your files, and there is no compatibility shim. An older build finds none of the old names, starts from defaults, and leaves your real settings untouched under names it does not know.
- Renaming the tracked `agents.json` shows up in your repo as a delete plus an add. The migration log says so.

## Rules for new files

- Pick the family and layer from the [Layers](#layers) table.
- Never invent a new suffix meaning.
- Generate `<NN>` and `<owner>` from one constant per layer in `src-tauri/src/config/instance_artifacts.rs`; never type them by hand.

The constants are `LAYER_DEFAULT`, `LAYER_REMOTE`, `LAYER_INSTANCE`, `LAYER_PROJECT` and `LAYER_PERSONAL` (each one `NN.owner` token), `NO_GIT_MARKER` and `STATE_MARKER`; the `layered_name!` macro composes a name from them. Every target name in the tables above exists there as a constant.

## Cross-references

- [Directory layout](directory-layout.md): where the instance dir and `.ac/` live
- [Settings reference](settings.md): the `settings.30.instance.no-git.json` schema
