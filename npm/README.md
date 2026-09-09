# AgentsCommander from npm

For developers who deliberately choose the secondary npm route, this page defines the supported hosts, install command, verification signal, and configuration precaution.

npm is not the recommended first install. Start with the reviewable [Coding Agent installation contract](https://github.com/mblua/AgentsCommander/blob/main/docs/install-with-agent.md), which detects the host, selects an approved stable-release asset, verifies its checksum, and waits for approval.

## Platform boundary

Use this package only on these documented combinations:

| Host | Native architecture | Status |
|---|---|---|
| Windows 10 version 1809 or newer, or Windows 11 | x86_64 / AMD64 | Fully supported |
| Linux | x86_64 / AMD64 | Partial and in progress; continue only after acknowledging the limitation |
| macOS | Any | Not supported; do not install through npm |
| Any other OS or architecture | Any | Unsupported; do not substitute an asset or use emulation |

The current npm installer does not enforce this complete allowlist: its asset mapping treats architectures other than `arm64` as `x86_64`. An asset selection or download is not a support promise. Follow the canonical [platform gates](https://github.com/mblua/AgentsCommander/blob/main/docs/install-with-agent.md#support-gates).

## Requirements

- Node.js 18 or newer.
- npm.
- One supported Coding Agent CLI installed and authenticated separately: Claude Code, Codex, Antigravity, or Pi.

AgentsCommander does not install or authenticate Coding Agent CLIs for you.

## Resolve versions and preserve configuration

Before an install, update, or uninstall, resolve and report the exact existing package and binary versions, then the exact registry release selected for comparison. For an install or update, that selected release must also be the version you install; for an uninstall, report that no replacement will be installed.

```bash
npm list -g @mblua/agentscommander --depth=0
npm root -g
npm view @mblua/agentscommander@latest version
```

`npm list` reports the installed package version and can exit nonzero when the package is absent. `npm root -g` exits 0 and prints the global `node_modules` directory. Use that root to inspect the installed `@mblua/agentscommander/package.json`, `install.js`, `run.js`, and native file under `bin/`. Correlate the package-manager record and `package.json` version with the install script's version and release URL, the launcher's exact native path, and the corresponding GitHub tag, asset, and checksum. `npm view` reports the selected registry version without installing it. If the local records, independent release evidence, native file, or selected version disagree or cannot be identified exactly, stop before mutation. `agentscommander --version` prints the native CLI version from `v0.31.0` onward; `v0.30.5` and older builds reject that flag, so probe a binary for a self-reported version only after its exact tag proves support.

The existing binary's exact resolver determines what must be preserved. The selected package's exact release tag determines post-install behavior. Verify both against the corresponding `v<version>` source tag; do not use `main` as evidence for a published npm package.

### npm `0.31.0`

`0.31.0` is the npm `latest` at this documentation commit. Its published install script downloads the `v0.31.0` native release asset, verifies it against `SHASUMS256.txt`, and renames it to the executable in the package's `bin/` directory; the launcher only spawns that binary and injects no configuration override. The `v0.31.0` native resolver, unchanged from `v0.30.5`:

1. selects the `AGENTSCOMMANDER_CONFIG_DIR` value verbatim when that variable is set and non-blank, running no probes;
2. otherwise derives the candidate `<native-executable-folder>/.<native-executable-stem>`, checks for a `portable.txt` marker beside the executable, and write-probes the candidate;
3. selects that candidate when the write probe succeeds, with or without the marker;
4. falls back to `$HOME/.agentscommander-new` only when the executable's parent and stem cannot be derived, or when the marker is absent and the candidate is conclusively unwritable; and
5. refuses to start on every other probe outcome (marker present but candidate unwritable, or an indeterminate probe), reporting an error that says to set `AGENTSCOMMANDER_CONFIG_DIR`.

The npm installer writes no `portable.txt`. For a normal npm `0.31.0` install with a writable global `node_modules`, the write probe succeeds and the selected directory is therefore `.agentscommander` beside the native executable under `@mblua/agentscommander/bin/`, the same location `0.30.3` selected, now confirmed by a probe instead of assumed.

The previous npm version, `0.30.5`, shares this resolver exactly. The older npm version `0.30.3` ships none of this: its resolver immediately selects `<native-executable-folder>/.<native-executable-stem>` whenever that parent and stem can be derived, uses `$HOME/.agentscommander-new` only when they cannot, and never reads `AGENTSCOMMANDER_CONFIG_DIR`, inspects `portable.txt`, probes writability, or falls back to home because the adjacent path is read-only. npm never published a `0.30.4` package; that number exists only as a GitHub release. For any other published version, inspect `src-tauri/src/config/mod.rs`, `src-tauri/src/config/profile.rs`, `npm/run.js`, and `npm/install.js` at its exact `v<version>` tag; do not extrapolate from `0.31.0`, `0.30.5`, `0.30.3`, or `main`.

Before an npm update or uninstall, identify the active directory with the existing binary's verified rule and exact path. The presence of a directory alone is not proof of selection.

Copy the complete persistent configuration directory to a user-controlled backup and verify the copy before running npm. Stop before an update or uninstall if more than one plausible directory exists, existing state cannot be attributed safely, the exact existing version or launch context is unknown, a selected candidate is mounted/read-only and ephemeral, or any other evidence is ambiguous. If selection is unambiguous and the selected persistent directory does not exist, record that there is no existing configuration to preserve. See the versioned [configuration-selection rule](https://github.com/mblua/AgentsCommander/blob/main/docs/features/portable-instances.md#config-directory-rule).

## Install and validate

```bash
npm install -g @mblua/agentscommander@<version>
```

Replace `<version>` with the exact version already inspected and approved; do not leave the install unpinned after resolving `latest`. The command exits 0 on success. The package's install script downloads that package version's raw asset and `SHASUMS256.txt` from `mblua/AgentsCommander`, computes SHA-256, and fails the install on a missing record or mismatch. This does not protect against compromise of the publisher or repository account because the asset and checksum share that trust boundary.

Validate the installed command:

```bash
npm list -g @mblua/agentscommander --depth=0
agentscommander --help
```

Both commands must exit 0. Confirm that `npm list` and the installed `package.json` report the approved package version; re-check the installed scripts, native path, and checksum correlation described above. `--help` must print the AgentsCommander command help, and on `0.31.0` or newer `agentscommander --version` must print the same version `npm list` reports. The npm package is `@mblua/agentscommander`; the installed command is `agentscommander`.

## Uninstall

Identify, back up, and verify the version-selected configuration as described above, then run:

```bash
npm uninstall -g @mblua/agentscommander
```

Success exits 0. Restore a previous version or remove the saved configuration only as a separate, deliberate action.

For stable-release downloads and rollback rules, use the [manual alternatives](https://github.com/mblua/AgentsCommander/blob/main/docs/install-with-agent.md#manual-alternatives).
