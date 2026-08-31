# Portable instances

For developers who want distinct AgentsCommander configurations on the same machine — for example a `prod` config and a `team-a` config side by side.

Configuration resolution is version-specific. This page separates the published `v0.30.3` behavior from the newer resolver on `main`. The `main` resolver is unpublished as of `v0.30.3`; do not attribute it to a release unless that exact release tag contains it.

## Config directory rule

AgentsCommander resolves one configuration directory when a process first needs it. Which rule it uses depends on the exact binary version.

### Published release `v0.30.3`

The `v0.30.3` release resolver is:

1. If `current_exe()` returns a path with a parent and file stem, select `<native-executable-folder>/.<native-executable-stem>` immediately.
2. Use the home directory only when that executable parent and stem cannot be derived. The normal production fallback is `$HOME/.agentscommander-new`.

Release builds of `v0.30.3` do not read `AGENTSCOMMANDER_CONFIG_DIR`. They do not inspect `portable.txt`, probe candidate writability, or fall back to home because an adjacent candidate is read-only. A marker or environment variable must not be used as evidence that `v0.30.3` selected another path. The similarly named `AGENTSCOMMANDER_TEST_CONFIG_DIR` is a debug-build test affordance, not a public release override.

For a `v0.30.3` AppImage, the native executable is inside the temporary, read-only AppImage mount. The resolver therefore selects a directory inside that mount; it does not select a persistent directory beside the external `.AppImage` file or fall back to `$HOME`. Writes can fail, and the selected mount path disappears after unmounting.

### Unpublished `main` resolver

The newer resolver in `main` adds this precedence, but it is not `v0.30.3` behavior:

1. A nonblank `AGENTSCOMMANDER_CONFIG_DIR` selects its original value verbatim and skips marker and write probes. An empty or whitespace-only value is ignored; prefer an absolute value so the selected path is unambiguous.
2. Without the override, AC derives the adjacent candidate and inspects `portable.txt` beside the native executable.
   - **Marker present:** a successful write probe selects the adjacent candidate. Any write-probe failure, or an indeterminate marker state, stops startup. There is no home fallback.
   - **Marker absent:** a successful write probe selects the adjacent candidate. A conclusively unwritable candidate selects the home fallback. An indeterminate write failure stops startup rather than guessing.
3. If the runtime cannot derive a usable executable parent and stem, AC uses the home fallback when one is available.

For the normal production identity, this `main` fallback is `$HOME/.agentscommander-new`; the `dev` identity uses `$HOME/.agentscommander-new-dev`. A marker cannot override the public environment variable because the override is evaluated first.

### Any other release

Inspect the exact release tag's `src-tauri/src/config/mod.rs` and `src-tauri/src/config/profile.rs` before selecting, preserving, moving, or deleting configuration. Apply the `v0.30.3` rule only to `v0.30.3`, and apply the `main` rule only to a development build from source or a later tag verified to contain it. Do not invent a version threshold.

These examples apply only when the exact version's resolver selects the adjacent candidate:

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

1. Record the raw native executable's exact version.
2. Copy it to a user-writable folder and rename it with an underscore suffix, such as `agentscommander_myteam.exe`.
3. Apply the rule for that exact version:
   - For `v0.30.3`, run the renamed native executable. It selects the adjacent directory immediately; `portable.txt` and `AGENTSCOMMANDER_CONFIG_DIR` have no release-build effect.
   - For a development build using the unpublished `main` resolver, confirm that no nonblank public override is present, then create an empty regular `portable.txt` beside the executable. If selection fails, move the tree to a writable location; do not remove the marker merely to obtain a home fallback.
   - For any other release, inspect its exact tag before proceeding.
4. Confirm the selected directory from runtime evidence before treating the copy as isolated.

Under the `main` resolver, one marker can serve multiple binaries in a folder. Under either verified resolver, each binary derives its adjacent candidate from its own file stem. Distinct selected directories isolate settings, sessions, logs, and tokens; the suffix separately determines the mutex and port.

## Why you might want this

- **Stage / prod parity.** Run `agentscommander_stage.exe` against your test repos while the canonical `agentscommander.exe` stays focused on shipping work.
- **Per-team instances.** Give each instance a distinct selected configuration, coding-agent credentials, and project list.
- **Reset experiments cheaply.** Back up the selected portable directory, experiment in a renamed copy, and remove only that confirmed copy when done.

## Portable project paths

Every project registration has a canonical absolute path and may have a companion path relative to the selected instance base:

- an absolute executable with a selected adjacent configuration uses the native executable's directory as the base;
- under the unpublished `main` resolver, an absolute `AGENTSCOMMANDER_CONFIG_DIR` uses the override directory's parent as the base; and
- the home fallback, a relative `main` override, or another degraded location has no instance base, so project registrations remain absolute-only and their relative companions are `null` or unavailable.

`v0.30.3` release builds have no public override. Their normal native-binary case uses the adjacent executable directory as the base; their home fallback has no base.

For an adjacent instance, move the raw native executable, its selected `.agentscommander_<suffix>/` directory, and project folders together. A project whose relative form still resolves is picked up at its new absolute path, and AC reconciles `settings.json` on the next load. The relative form is anchored to the selected instance base, never the process working directory.

Relocation carries a project across the move when:

- the project keeps the same position relative to the selected instance base, and
- the base and project still share one filesystem root, such as the same Windows drive letter or UNC share.

It does not help when only one side moves or when the project is on another drive or share. Such a project has no relative form, keeps working through its absolute path, and does not relocate with the portable tree.

**Packaging layout.** The executable used by the resolver is the real native executable, not a wrapper or downloaded container file. On Windows raw-binary installs, it is the folder containing `agentscommander*.exe`. For an unsupported macOS tester/contributor bundle, it is `Foo.app/Contents/MacOS`, not the `.app` root. An AppImage executes its payload from a [temporary, read-only mount](https://docs.appimage.org/reference/architecture.html); the directory containing the external `.AppImage` file is not the native executable directory. `v0.30.3` selects its candidate in that mount and does not relocate to home, so neither that configuration path nor its instance base is a persistent portable location. A `portable.txt` beside the external AppImage has no effect on `v0.30.3`.

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

Close the instance, record its exact version, confirm the selected path under that version's rule, and back up anything you need. Then delete only that binary and exact configuration directory. Delete a shared `portable.txt` marker only when a verified resolver used it and no remaining binary relies on it. If an unpublished-`main` build selected an override or any version selected a home fallback, preserve that directory separately; never delete a guessed candidate.

For a `v0.30.3` AppImage update or uninstall, stop before mutation if the selected candidate is in the mounted read-only AppDir, any existing application state is found, more than one plausible candidate exists, or any selection evidence is ambiguous. The mounted candidate is not a persistent directory that can be safely preserved or removed. Report the blocker instead of inventing an external-file or home-directory path.

## See also

- [Settings reference](../reference/settings.md) — what lives in `settings.json`
- [`PRIVACY.md`](../../PRIVACY.md) — what data each instance writes to disk
