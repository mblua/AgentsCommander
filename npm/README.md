# AgentsCommander from npm

## Recommended npm installation: global

When installing through npm, use `-g` so the `agentscommander` command is available from any directory:

```bash
npm install -g @mblua/agentscommander@latest
```

This installs the current `latest` release. Before installing, follow [Resolve versions and preserve configuration](#resolve-versions-and-preserve-configuration), then [Install and validate](#install-and-validate). Check the supported platforms and requirements below before installing.

The npm website's **Install** sidebar shows the generic local-install command. For the recommended global npm installation, include `-g` as shown above.

## Choosing the npm route

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

Before an install, update, or uninstall, resolve and report the exact existing package and binary versions, then the current `latest` registry release. For an install or update, that `latest` release is the version you install; for an uninstall, report that no replacement will be installed.

```bash
npm list -g @mblua/agentscommander --depth=0
npm root -g
npm view @mblua/agentscommander@latest version
```

`npm list` reports the installed package version and can exit nonzero when the package is absent. `npm root -g` exits 0 and prints the global `node_modules` directory. Use that root to inspect the installed `@mblua/agentscommander/package.json`, `install.js`, `run.js`, and native file under `bin/`. Correlate the package-manager record and `package.json` version with the install script's version and release URL, the launcher's exact native path, and the corresponding GitHub tag, asset, and checksum. `npm view` reports the selected registry version without installing it. If the local records, independent release evidence, native file, or selected version disagree or cannot be identified exactly, stop before mutation. `agentscommander --version` prints the native CLI version from `v0.31.0` onward; `v0.30.5` and older builds reject that flag, so probe a binary for a self-reported version only after its exact tag proves support.

The existing binary's exact resolver determines what must be preserved. The selected package's exact release tag determines post-install behavior. Verify both against the corresponding `v<version>` source tag; do not use `main` as evidence for a published npm package.

### npm `0.33.0`

`0.33.0` is the release described by this package. Its installer downloads the `v0.33.0` native release asset and verifies it against `SHASUMS256.txt`. On Windows and Linux it renames the asset to the unsuffixed executable in the package's `bin/` directory. On macOS it downloads the `agentscommander-mac-<arch>.app.tar.gz` bundle, extracts the single `.app` directory into `bin/`, and validates the executable named by the bundle's `CFBundleExecutable`. The launcher spawns that executable and injects no configuration override.

The `v0.33.0` native resolver selects a non-blank `AGENTSCOMMANDER_CONFIG_DIR` value verbatim when present. Otherwise, the normal unsuffixed npm executable selects the user's home directory plus `.agentscommander`, independently of its install location and build profile. It does not inspect an adjacent `portable.txt` marker or probe an adjacent configuration directory on this route. Suffixed executables use their adjacent `.agentscommander_<suffix>` directory and refuse to start when it is not writable.

**Version `0.33.0` keeps the `0.32.0` default location.** Versions `0.31.0` and `0.30.5` normally selected `.agentscommander` beside the native executable under `@mblua/agentscommander/bin/` when that location was writable, with their documented home fallback and refusal rules. Neither `0.32.0` nor `0.33.0` discovers, copies, moves or migrates any old configuration automatically. Before updating from `0.31.0` or older, identify and back up the old version's active configuration. If you intend to keep using that directory, set `AGENTSCOMMANDER_CONFIG_DIR` to its verified path; otherwise arrange the intended configuration at the new home location separately. Version `0.33.0` also drops the legacy pre-v2 `codingAgentProfiles` settings migration.

The older npm `0.30.3` resolver immediately selected an executable-adjacent directory when the parent and stem were available and did not support the configuration override or portable-marker/write-probe rules. npm never published `0.30.4`; that number exists only as a GitHub release. For other versions, inspect `src-tauri/src/config/mod.rs`, `src-tauri/src/config/profile.rs`, `npm/run.js`, and `npm/install.js` at the exact `v<version>` tag.

Before an npm update or uninstall, identify the active directory with the existing binary's verified rule and exact path. The presence of a directory alone is not proof of selection.

Copy the complete persistent configuration directory to a user-controlled backup and verify the copy before running npm. Stop before an update or uninstall if more than one plausible directory exists, existing state cannot be attributed safely, the exact existing version or launch context is unknown, a selected candidate is mounted/read-only and ephemeral, or any other evidence is ambiguous. If selection is unambiguous and the selected persistent directory does not exist, record that there is no existing configuration to preserve. See the versioned [configuration-selection rule](https://github.com/mblua/AgentsCommander/blob/main/docs/features/portable-instances.md#config-directory-rule).

## Install and validate

```bash
npm install -g @mblua/agentscommander@latest
```

The command installs the current `latest` release and exits 0 on success. The package's install script downloads that package version's raw asset and `SHASUMS256.txt` from `mblua/AgentsCommander`, computes SHA-256, and fails the install on a missing record or mismatch. This does not protect against compromise of the publisher or repository account because the asset and checksum share that trust boundary.

Validate the installed command:

```bash
npm list -g @mblua/agentscommander --depth=0
agentscommander --help
```

Both commands must exit 0. Confirm that `npm list` and the installed `package.json` report the `latest` version that `npm view @mblua/agentscommander@latest version` returned; re-check the installed scripts, native path, and checksum correlation described above. `--help` must print the AgentsCommander command help, and on `0.31.0` or newer `agentscommander --version` must print the same version `npm list` reports. The npm package is `@mblua/agentscommander`; the installed command is `agentscommander`.

### Windows Start Menu shortcut

On Windows, a global install also creates a per-user Start Menu shortcut named **AgentsCommander**, with the app icon, at `%APPDATA%\Microsoft\Windows\Start Menu\Programs\AgentsCommander.lnk`. It opens the installed `bin\agentscommander.exe`. A local install (without `-g`) creates no shortcut. To skip it, set `AGENTSCOMMANDER_NO_SHORTCUT=1` before installing. If the shortcut cannot be created, the install prints a warning and still succeeds. Installing again replaces the same shortcut.

## Uninstall

Identify, back up, and verify the version-selected configuration as described above, then run:

```bash
npm uninstall -g @mblua/agentscommander
```

Success exits 0. npm does not run uninstall scripts, so on Windows delete the Start Menu shortcut by hand:

```powershell
Remove-Item "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\AgentsCommander.lnk"
``` Restore a previous version or remove the saved configuration only as a separate, deliberate action.

For stable-release downloads and rollback rules, use the [manual alternatives](https://github.com/mblua/AgentsCommander/blob/main/docs/install-with-agent.md#manual-alternatives).
