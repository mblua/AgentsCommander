# Portable instances

For developers who want isolated AgentsCommander instances on the same machine — for example a `prod` config and a `team-a` config side by side.

AgentsCommander is fully portable. The binary carries everything it needs; no installation is required. Two copies of the binary in different folders, or with different filenames, run as completely independent applications.

## Config directory rule

The config directory lives **next to the binary**, named after it:

```
C:\tools\agentscommander.exe          ->  C:\tools\.agentscommander\
C:\tools\agentscommander_stage.exe    ->  C:\tools\.agentscommander_stage\
C:\work\agentscommander_team-a.exe    ->  C:\work\.agentscommander_team-a\
```

Each config directory contains `settings.json`, `sessions.json`, the web token, conversation logs, and every other piece of per-instance state. Two binaries in different folders (or with different names) are **fully isolated**: separate settings, separate sessions, separate ports, separate mutex.

## Instance labels via underscore suffix

Rename the binary with an underscore suffix to create a labeled instance:

```
agentscommander_<suffix>.exe
```

The suffix (uppercased) appears as a badge in the titlebar and drives port and mutex allocation:

| Binary name | Titlebar | Mutex | Web port |
|---|---|---|---|
| `agentscommander.exe` | Agents Commander | Shared (prod) | 9877 |
| `agentscommander_stage.exe` | Agents Commander **[STAGE]** | Unique | 9878 |
| `agentscommander_dev.exe` | Agents Commander **[DEV]** | Unique | 9876 |
| `agentscommander_team-a.exe` | Agents Commander **[TEAM-A]** | Unique | Auto (9880–9899) |

Unknown suffixes get a deterministic port in the 9880–9899 range based on a hash of the suffix name. Reserved suffixes (`stage`, `dev`, `prod`) keep their fixed ports.

## Creating an isolated instance

1. Copy `agentscommander.exe` to any folder.
2. Rename it with an underscore suffix: `agentscommander_myteam.exe`.
3. Run it.

That's it. The instance creates its own config directory on first launch, gets a unique mutex (so it does not conflict with other instances), and shows **[MYTEAM]** in the titlebar.

## Why you might want this

- **Stage / prod parity.** Run `agentscommander_stage.exe` against your test repos while the canonical `agentscommander.exe` stays focused on shipping work.
- **Per-team workspaces.** One instance per team you operate, each with its own coding-agent credentials and project list.
- **Reset experiments cheaply.** Want to try a different settings layout without losing your current one? Copy the `.exe`, rename it, experiment, delete when done.

## What is NOT isolated

A portable instance still shares:

- The user's `PATH` (and therefore which coding-agent binaries it can find).
- The user account's filesystem permissions.
- API keys exported in the environment.

If you need hard isolation, run AC inside a VM or container.

## Cleaning up an instance

Delete the binary and its `.agentscommander_<suffix>/` directory. AC keeps no global state outside that directory.

## See also

- [Settings reference](../reference/settings.md) — what lives in `settings.json`
- [`PRIVACY.md`](../../PRIVACY.md) — what data each instance writes to disk
