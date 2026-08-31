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

## Preserve existing configuration

The npm launcher runs the native executable from the package's `bin/` directory, but that directory provides only a configuration candidate. AgentsCommander resolves one active configuration directory at process startup, in this order:

1. A nonblank `AGENTSCOMMANDER_CONFIG_DIR` selects its original value verbatim and skips the remaining probes.
2. Without that override, the candidate is `.<native-executable-stem>` beside the native executable. If `portable.txt` is beside that executable, a successful write probe selects the candidate; any probe failure is a startup error, with no home fallback. Without the marker, a successful probe selects the candidate, a conclusively unwritable candidate selects the home fallback, and an indeterminate result is a startup error.
3. If no usable native-executable parent and stem can be resolved, AgentsCommander also uses the home fallback when one is available. For the normal production identity, that fallback is `$HOME/.agentscommander-new`.

An empty or whitespace-only public override is ignored. See the complete [configuration-selection rule](https://github.com/mblua/AgentsCommander/blob/main/docs/features/portable-instances.md#config-directory-rule).

Before an npm update or uninstall, resolve the global package directory:

```bash
npm root -g
```

The command exits 0 and prints the global `node_modules` directory. Use it to locate `@mblua/agentscommander/bin/` and the native executable, then identify the active directory from the precedence above and the existing launch environment. The presence of an adjacent directory alone does not prove that it was selected.

Copy the complete active configuration directory to a user-controlled backup and verify the copy before running npm. Stop before any update or uninstall if more than one plausible configuration directory exists, the launch environment is unknown, a relative override cannot be resolved from the actual launch context, or any other evidence leaves the selection ambiguous. If the selection is unambiguous and the selected directory does not exist, record that there is no existing configuration to preserve.

## Install and validate

```bash
npm install -g @mblua/agentscommander
```

The command exits 0 on success. The package's install script downloads the package version's raw asset and `SHASUMS256.txt` from `mblua/AgentsCommander`, computes SHA-256, and fails the install on a missing record or mismatch. This does not protect against compromise of the publisher or repository account because the asset and checksum share that trust boundary.

Validate the installed command:

```bash
agentscommander --help
```

Success exits 0 and prints the AgentsCommander command help. The npm package is `@mblua/agentscommander`; the installed command is `agentscommander`.

## Uninstall

Identify, back up, and verify the active selected configuration as described above, then run:

```bash
npm uninstall -g @mblua/agentscommander
```

Success exits 0. Restore a previous version or remove the saved configuration only as a separate, deliberate action.

For stable-release downloads and rollback rules, use the [manual alternatives](https://github.com/mblua/AgentsCommander/blob/main/docs/install-with-agent.md#manual-alternatives).
