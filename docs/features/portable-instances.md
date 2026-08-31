# Portable instances

For developers who want distinct AgentsCommander configurations on the same machine — for example a `prod` config and a `team-a` config side by side.

A portable instance is a raw native executable whose adjacent configuration candidate is selected. Add a `portable.txt` marker beside that executable to make this fail closed: without a higher-priority public override, AgentsCommander either uses the writable adjacent directory or refuses to start.

## Config directory rule

AgentsCommander resolves the configuration directory once per process. The first decisive state wins:

1. A nonblank `AGENTSCOMMANDER_CONFIG_DIR` selects its original value verbatim. It has highest priority and skips marker and write probes. An empty or whitespace-only value is ignored; prefer an absolute value so the selected path is unambiguous.
2. Without the override, AgentsCommander derives an adjacent candidate named `.<native-executable-stem>` and looks for `portable.txt` beside the real native executable.
   - **Marker present:** a successful write probe selects the adjacent candidate. Any write-probe failure, or an indeterminate marker state, stops startup. There is no home fallback.
   - **Marker absent:** a successful write probe selects the adjacent candidate. A conclusively unwritable candidate selects the home fallback. An indeterminate write failure stops startup rather than guessing.
3. If the runtime cannot derive a usable executable parent and stem, it uses the home fallback when one is available.

For the normal production identity, the home fallback is `$HOME/.agentscommander-new`; the `dev` identity uses `$HOME/.agentscommander-new-dev`. A `portable.txt` marker cannot override `AGENTSCOMMANDER_CONFIG_DIR` because the public override is evaluated first.

These examples apply only when the adjacent candidate is selected:

```text
C:\tools\agentscommander.exe          ->  C:\tools\.agentscommander\
C:\tools\agentscommander_stage.exe    ->  C:\tools\.agentscommander_stage\
C:\work\agentscommander_team-a.exe    ->  C:\work\.agentscommander_team-a\
```

The selected directory contains `settings.json`, `sessions.json`, the web token, conversation logs, and other machine-local application state. Two copies have separate application state only when they select different configuration directories. Project-scoped team state remains in each project's shared `.ac/` tree.

## Instance labels via underscore suffix

Rename the binary with an underscore suffix to create a labeled instance:

```text
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

## Creating an isolated portable instance

1. Copy the raw native executable to a user-writable folder.
2. Rename it with an underscore suffix, such as `agentscommander_myteam.exe`.
3. Confirm that its launch environment has no nonblank `AGENTSCOMMANDER_CONFIG_DIR`; that override wins over portable mode.
4. Create an empty regular file named `portable.txt` beside the executable.
5. Run the executable. If configuration selection fails, stop and move the portable tree to a writable location; do not remove the marker merely to obtain a silent home fallback.

The marker can serve multiple binaries in one folder. Each binary derives its adjacent directory from its own file stem. Distinct selected directories isolate settings, sessions, logs, and tokens; the suffix separately determines the mutex and port.

## Why you might want this

- **Stage / prod parity.** Run `agentscommander_stage.exe` against your test repos while the canonical `agentscommander.exe` stays focused on shipping work.
- **Per-team instances.** Give each instance a distinct selected configuration, coding-agent credentials, and project list.
- **Reset experiments cheaply.** Back up the selected portable directory, experiment in a renamed copy, and remove only that confirmed copy when done.

## Portable project paths

Every project registration has a canonical absolute path and may have a companion path relative to the selected instance base:

- an absolute executable with a selected adjacent configuration uses the native executable's directory as the base;
- an absolute `AGENTSCOMMANDER_CONFIG_DIR` uses the override directory's parent as the base; and
- the home fallback, a relative override, or another degraded location has no instance base, so project registrations remain absolute-only and their relative companions are `null` or unavailable.

For a marked adjacent instance, move the raw native executable, its selected `.agentscommander_<suffix>/` directory, and project folders together. A project whose relative form still resolves is picked up at its new absolute path, and AC reconciles `settings.json` on the next load. The relative form is anchored to the selected instance base, never the process working directory.

Relocation carries a project across the move when:

- the project keeps the same position relative to the selected instance base, and
- the base and project still share one filesystem root, such as the same Windows drive letter or UNC share.

It does not help when only one side moves or when the project is on another drive or share. Such a project has no relative form, keeps working through its absolute path, and does not relocate with the portable tree.

**Packaging layout.** The executable used by the resolver is the real native executable, not a wrapper or downloaded container file. On Windows raw-binary installs, it is the folder containing `agentscommander*.exe`. For an unsupported macOS tester/contributor bundle, it is `Foo.app/Contents/MacOS`, not the `.app` root. An AppImage executes its payload from a [temporary, read-only mount](https://docs.appimage.org/reference/architecture.html); the directory containing the external `.AppImage` file is not the native executable directory. For the current unmarked release AppImage without a public override, the read-only mounted candidate falls back to the production home directory and supplies no portable project base. A `portable.txt` beside the external AppImage does not change that selection.

**Conflict handling.** Both stored forms are resolved and validated on every load. If they point at the same directory, including through symlinks or Windows aliases, the project loads once. If they resolve to different real directories, AC loads neither side, writes nothing for that registration, and shows one sticky red error toast listing both paths. Other, non-conflicting projects still load normally. Dismiss the toast, then fix the registration by removing it and reopening the intended folder.

**Sessions stay absolute.** This portability applies to project registrations only. Saved sessions in `sessions.json` keep absolute working directories and absolute nested repo paths, and they follow the existing retention and purge rules. Relocating an instance does not rewrite session paths.

## What is not isolated

A portable instance still shares:

- Project `.ac/` state when another instance registers the same project.
- The user's `PATH`, and therefore which coding-agent binaries it can find.
- The user account's filesystem permissions.
- API keys exported in the environment.

If you need hard isolation, run AC inside a VM or container.

## Cleaning up an instance

Close the instance, confirm that it selected the expected adjacent directory, and back up anything you need. Then delete only that binary and exact configuration directory. Delete a shared `portable.txt` marker only when no remaining binary in the folder relies on it. If the instance selected an override or home fallback, preserve that directory separately; never delete a guessed candidate. Project `.ac/` trees are outside this cleanup and remain in their projects.

## See also

- [Settings reference](../reference/settings.md) — what lives in `settings.json`
- [`PRIVACY.md`](../../PRIVACY.md) — what data each instance writes to disk
