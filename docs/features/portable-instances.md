# Portable instances

For developers who want isolated AgentsCommander instances on the same machine,
for example a `prod` config and a `team-a` config side by side.

Raw AgentsCommander binaries remain portable. The canonical Linux DEB is the
exception: `/usr/bin/agentscommander` uses the user's XDG config directory and
has no executable-relative instance base.

## Config directory rule

The config directory depends on how AgentsCommander is launched:

- The canonical Linux DEB executable `/usr/bin/agentscommander` uses
  `$XDG_CONFIG_HOME/agentscommander`, or
  `$HOME/.config/agentscommander` when `XDG_CONFIG_HOME` is unavailable.
- Raw portable binaries, including renamed Linux binaries, retain the
  executable-relative config directory named after that binary.

Portable Windows examples:

```
C:\tools\agentscommander.exe          ->  C:\tools\.agentscommander\
C:\tools\agentscommander_stage.exe    ->  C:\tools\.agentscommander_stage\
C:\work\agentscommander_team-a.exe    ->  C:\work\.agentscommander_team-a\
```

Each config directory contains `settings.json`, `sessions.json`, the web token,
conversation logs, and every other piece of per-instance state. Config roots
are the isolation boundary. Two portable binaries with different config roots
have separate settings, sessions, ports, and instance locks. On Linux, two
launches that resolve to the same config root cannot run together; the second
exits successfully without modifying the live instance.

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

## Portable project paths

Every project registration contains its canonical absolute path. When the
location resolver supplies a portable instance base, it also stores a companion
path relative to the folder holding the running binary. Move that raw binary,
its config directory, and the project tree together, and each companion that
still resolves is picked up at its new absolute path. AC reconciles
`settings.json` on the next load.

The canonical Linux DEB has no portable instance base. Its project
registrations are absolute-only.

The relative form is anchored to the executable's own directory, never the process working directory, so it does not matter which shell or folder you launch AC from.

Relocation carries a project across the move when:

- the project keeps the same position relative to the binary's folder (they move together under a new parent), and
- the binary and the project still share one filesystem root (same Windows drive letter or UNC share).

It does not help when only one side moves, or when the project lives on a different drive or share than the binary. A project on a different drive/share has no relative form at all: its companion value is stored as `null`, it keeps working in place through the absolute path, and it is simply not portable if you later move the install folder.

**Packaging layout.** When a portable base exists, its anchor is the directory
of the real native executable, not a wrapper or app root. On Windows that is
the folder containing `agentscommander*.exe`. On macOS it is
`Foo.app/Contents/MacOS` inside the bundle. On Linux it is the directory of the
raw binary or running AppImage. `/usr/bin/agentscommander` is classified as the
canonical DEB executable and deliberately supplies no anchor.

**Conflict handling.** Both stored forms are resolved and validated on every load. If they point at the same directory (symlinks and Windows aliases included), the project loads once. If they resolve to two different real directories, that registration is a conflict: AC loads neither side, writes nothing to disk for it, and the sidebar shows one sticky red error toast listing both resolved paths. Other, non-conflicting projects still load normally. Dismiss the toast, then fix the registration (remove it, then re-open the folder you want) to clear the conflict.

**Sessions stay absolute.** This portability applies to project registrations only. Saved sessions in `sessions.json` keep absolute working directories and absolute nested repo paths, and they follow the existing retention and purge rules. Relocating an instance does not rewrite session paths.

## What is NOT isolated

A portable instance still shares:

- The parent process environment. On Linux, a local Codex child additionally
  gets existing `$HOME/.local/bin`, `$HOME/bin`, and `$HOME/.cargo/bin`
  candidates prepended to its own PATH. This child-only adjustment does not
  alter AgentsCommander's PATH.
- The user account's filesystem permissions.
- API keys exported in the environment.

If you need hard isolation, run AC inside a VM or container.

## Cleaning up an instance

For a raw portable instance, delete the binary and its
`.agentscommander_<suffix>/` directory. The canonical DEB's package files and
XDG config are separate: uninstall the package with the system package manager,
then remove its config directory only if you also want to discard user state.

## See also

- [Settings reference](../reference/settings.md) — what lives in `settings.json`
- [`PRIVACY.md`](../../PRIVACY.md) — what data each instance writes to disk
