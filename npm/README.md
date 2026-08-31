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

The npm launcher runs the native executable from the package's `bin/` directory. AgentsCommander keeps its `.agentscommander*` configuration next to that native executable. Before an npm update or uninstall, resolve the global package directory and copy that adjacent configuration to a user-controlled backup location:

```bash
npm root -g
```

The command exits 0 and prints the global `node_modules` directory. Under it, inspect `@mblua/agentscommander/bin/` for the native executable and its adjacent `.agentscommander*` directory. Do not continue if you cannot identify and preserve an existing configuration.

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

Preserve the adjacent configuration first, then run:

```bash
npm uninstall -g @mblua/agentscommander
```

Success exits 0. Restore a previous version or remove the saved configuration only as a separate, deliberate action.

For stable-release downloads and rollback rules, use the [manual alternatives](https://github.com/mblua/AgentsCommander/blob/main/docs/install-with-agent.md#manual-alternatives).
