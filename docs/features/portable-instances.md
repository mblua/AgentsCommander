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

The rename is what makes the copy independent, not the folder you put it in. Two copies that still share a file name share one instance identity, so starting the second one shows a message explaining the collision and pointing at the rename, then exits without disturbing the first. Before #1592 it exited silently, which looked like nothing happening at all.

## Why you might want this

- **Stage / prod parity.** Run `agentscommander_stage.exe` against your test repos while the canonical `agentscommander.exe` stays focused on shipping work.
- **Per-team instances.** One instance per team you operate, each with its own coding-agent credentials and project list.
- **Reset experiments cheaply.** Want to try a different settings layout without losing your current one? Copy the `.exe`, rename it, experiment, delete when done.

## Portable project paths

Every project you register is stored two ways in `settings.json`: the usual absolute path, and a portable path **relative to the folder that holds the running binary** (the same folder the config directory sits next to). Move the whole tree together (the binary, its `.agentscommander_<suffix>/` directory, and the project folders) to a new location, and each project whose relative form still resolves is picked up at its new absolute path automatically. AC reconciles `settings.json` to the new absolute path on the next load.

The relative form is anchored to the executable's own directory, never the process working directory, so it does not matter which shell or folder you launch AC from.

Relocation carries a project across the move when:

- the project keeps the same position relative to the binary's folder (they move together under a new parent), and
- the binary and the project still share one filesystem root (same Windows drive letter or UNC share).

It does not help when only one side moves, or when the project lives on a different drive or share than the binary. A project on a different drive/share has no relative form at all: its companion value is stored as `null`, it keeps working in place through the absolute path, and it is simply not portable if you later move the install folder.

**Packaging layout.** The anchor is the directory of the real native executable, not a wrapper or an app root. On Windows that is the folder containing `agentscommander*.exe`. On macOS it is `Foo.app/Contents/MacOS` inside the bundle, not the `.app` root. On Linux it is the directory of the raw binary or the running AppImage. A project stored relative to `Contents/MacOS` relocates correctly only when it keeps that relationship to the bundle.

**Conflict handling.** Both stored forms are resolved and validated on every load. If they point at the same directory (symlinks and Windows aliases included), the project loads once. If they resolve to two different real directories, that registration is a conflict: AC loads neither side, writes nothing to disk for it, and the sidebar shows one sticky red error toast listing both resolved paths. Other, non-conflicting projects still load normally. Dismiss the toast, then fix the registration (remove it, then re-open the folder you want) to clear the conflict.

**Sessions stay absolute.** This portability applies to project registrations only. Saved sessions in `sessions.json` keep absolute working directories and absolute nested repo paths, and they follow the existing retention and purge rules. Relocating an instance does not rewrite session paths.

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
