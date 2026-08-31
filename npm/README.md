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

`npm list` reports the installed package version and can exit nonzero when the package is absent. `npm root -g` exits 0 and prints the global `node_modules` directory. Use that root to locate `@mblua/agentscommander/bin/`, then query the exact native executable there with `--version`; do not rely only on whichever `agentscommander` happens to be on `PATH`. `npm view` reports the selected registry version without installing it. If the package metadata, native binary, and selected version disagree or cannot be identified exactly, stop before mutation.

The existing binary's exact resolver determines what must be preserved. The selected package's exact release tag determines post-install behavior. Verify both against the corresponding `v<version>` source tag; do not use `main` as evidence for a published npm package.

### npm `0.30.3`

The published `0.30.3` install script downloads the `v0.30.3` native release asset, and its launcher only spawns that binary from the package's `bin/` directory. It injects no configuration override. The native resolver:

1. immediately selects `<native-executable-folder>/.<native-executable-stem>` when the executable path has a parent and file stem; and
2. uses `$HOME/.agentscommander-new` only when that parent and stem cannot be derived.

Release `v0.30.3` does not read `AGENTSCOMMANDER_CONFIG_DIR`, inspect `portable.txt`, probe candidate writability, or fall back to home because the adjacent path is read-only. For a normal npm `0.30.3` install, the selected directory is therefore `.agentscommander` beside the native executable under `@mblua/agentscommander/bin/`.

The public override, marker, write probes, and conclusively-unwritable home fallback documented for the newer resolver are present on `main` but unpublished in `0.30.3`. Apply them only to a development build from matching source, or to a later npm version after verifying that version's exact release tag and package launcher. For any other published version, inspect `src-tauri/src/config/mod.rs`, `src-tauri/src/config/profile.rs`, `npm/run.js`, and `npm/install.js` at its exact tag; do not extrapolate either `v0.30.3` or `main` and do not invent a cutoff version.

Before an npm update or uninstall, identify the active directory with the existing binary's verified rule and exact path. The presence of a directory alone is not proof of selection.

Copy the complete persistent configuration directory to a user-controlled backup and verify the copy before running npm. Stop before an update or uninstall if more than one plausible directory exists, existing state cannot be attributed safely, the exact existing version or launch context is unknown, a selected candidate is mounted/read-only and ephemeral, or any other evidence is ambiguous. If selection is unambiguous and the selected persistent directory does not exist, record that there is no existing configuration to preserve. See the versioned [configuration-selection rule](https://github.com/mblua/AgentsCommander/blob/main/docs/features/portable-instances.md#config-directory-rule).

## Install and validate

```bash
npm install -g @mblua/agentscommander@<version>
```

Replace `<version>` with the exact version already inspected and approved; do not leave the install unpinned after resolving `latest`. The command exits 0 on success. The package's install script downloads that package version's raw asset and `SHASUMS256.txt` from `mblua/AgentsCommander`, computes SHA-256, and fails the install on a missing record or mismatch. This does not protect against compromise of the publisher or repository account because the asset and checksum share that trust boundary.

Validate the installed command:

```bash
agentscommander --version
agentscommander --help
```

Both commands must exit 0. Confirm that `--version` reports the approved package version; `--help` must print the AgentsCommander command help. The npm package is `@mblua/agentscommander`; the installed command is `agentscommander`.

## Uninstall

Identify, back up, and verify the version-selected configuration as described above, then run:

```bash
npm uninstall -g @mblua/agentscommander
```

Success exits 0. Restore a previous version or remove the saved configuration only as a separate, deliberate action.

For stable-release downloads and rollback rules, use the [manual alternatives](https://github.com/mblua/AgentsCommander/blob/main/docs/install-with-agent.md#manual-alternatives).
