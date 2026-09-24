# File naming convention

For contributors who add, rename, or review an AgentsCommander config file. Check this page before you name a file, so that the name tells every reader who owns it, which layer wins, and whether it may be committed.

> **Status: target convention, mostly NOT implemented.**
> The product owner decided this convention on 2026-09-23 ([#2448](https://github.com/mblua/AgentsCommander/issues/2448)). Most renames below are still pending and will land in a dedicated epic. Until then, the shipped product still reads and writes today's names. When another doc names a file differently, this page wins for new work; the other doc describes today's code until someone updates it.
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

## Today's names and their targets

### `settings.*`

| Today | Target |
|---|---|
| `settings.json` | `settings.30.instance.no-git.json` |
| `settings.local.json` | `settings.50.personal.no-git.json` |
| `settings.json.lock` | `settings.30.instance.no-git.json.lock` |
| `settings.backup.*.json` | `settings.30.instance.no-git.backup.*.json` |
| `settings.pre-*.json` | `settings.30.instance.no-git.pre-*.json` |

Lock and backup files follow the live file's name.

### `blocking-menus.*`

| Today | Target |
|---|---|
| `settings-blocking-menus.json` | `blocking-menus.10.default.no-git.json` |
| `settings-blocking-menus.remote.json` | `blocking-menus.20.remote.no-git.json` |
| `settings-blocking-menus.local.json` | `blocking-menus.50.personal.no-git.json` |
| `blocking-menus-remote-check.json` | `blocking-menus.state.no-git.json` |

### `agents.*`

| Today | Target |
|---|---|
| `.ac/coding-agents/agents.json` | `agents.10.default.json` (tracked, so no `.no-git`) |
| `agents.local.json` | `agents.50.personal.no-git.json` |
| (none) | `agents.40.project.json` (new, tracked) |
| (none) | `agents.30.instance.no-git.json` (new, in the instance dir) |
| `.agents.json.lock` | `.agents.10.default.json.lock` |

Never renamed: `agents.migration-v1.backup.json` and `.agents.migration-v1.json`. An interrupted catalog migration resumes by those exact names.

## Planned splits

| From | To |
|---|---|
| Coding agents and profiles in `settings.json` | `agents.30.instance.no-git.json` |
| Window geometry and zoom in `settings.json` | `window.state.no-git.json` |
| Agent `config.json` | Decisions stay in `config.json` (tracked); state moves to `config.state.no-git.json` |

## The model: blocking-menu precedence

Blocking menus already resolve layers the way this convention generalizes (`src-tauri/src/config/settings.rs:1698-1714`). The first match wins:

1. Per-agent local entries
2. Per-command local entries
3. Remote entries
4. Shipped entries

## Migration policy

- AC migrates once, at startup, and writes a log of what it renamed. After that, the code knows only the new names.
- The migration is **forward only**. You cannot downgrade past the version that migrates your files.
- Renaming the tracked `agents.json` shows up in your repo as a delete plus an add. The migration log says so.

## Rules for new files

- Pick the family and layer from the [Layers](#layers) table.
- Never invent a new suffix meaning.
- Generate `<NN>` and `<owner>` from one constant per layer in `src-tauri/src/config/instance_artifacts.rs`; never type them by hand.

The constants are `LAYER_DEFAULT`, `LAYER_REMOTE`, `LAYER_INSTANCE`, `LAYER_PROJECT` and `LAYER_PERSONAL` (each one `NN.owner` token), `NO_GIT_MARKER` and `STATE_MARKER`; the `layered_name!` macro composes a name from them. Every target name in the tables above already exists there as a constant, and none is in use until the rename ships.

## Cross-references

- [Directory layout](directory-layout.md): where the instance dir and `.ac/` live
- [Settings reference](settings.md): today's `settings.json` schema
