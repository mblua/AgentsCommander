# Install AgentsCommander with a Coding Agent

For developers who already use a trusted Coding Agent, this contract gets you to an approved, checksum-verified AgentsCommander installation or a safe stop before your machine changes.

## Support gates

An artifact on a GitHub release proves that the build exists. It does not make that platform supported.

| Host | Native architecture | Support tier | Normal installation |
|---|---|---|---|
| Windows 10 version 1809 or newer, or Windows 11 | x86_64 / AMD64 | Fully supported; primary development and release-validation platform | Yes |
| Linux | x86_64 / AMD64 | Partial and in progress; broader distribution and feature coverage is incomplete | Only after a warning and explicit confirmation |
| macOS | Any | Not supported yet because maintainer and test capacity is insufficient | No; stop unless the user explicitly chooses the tester/contributor path |
| Any other OS or architecture | Any | Unsupported | No; stop without substitution, emulation, or fallback |

Linux does not currently include every Windows capability. Verified Windows-only features include global-hotkey screenshot capture, native window listing and capture, the control-plane window-screenshot route, and detection of a running GUI before CLI configuration writes. See [Screenshot capture](features/screenshot-capture.md), [Window capture](features/window-capture.md), and the [CLI reference](reference/cli.md#coding-agent).

## Use only pinned official evidence

Use only these sources:

- repository: `https://github.com/mblua/AgentsCommander`;
- canonical guide on `main`: `https://github.com/mblua/AgentsCommander/blob/main/docs/install-with-agent.md`;
- pinned guide: `https://github.com/mblua/AgentsCommander/blob/<full-commit>/docs/install-with-agent.md`;
- latest stable release: `https://github.com/mblua/AgentsCommander/releases/latest`;
- release assets under `https://github.com/mblua/AgentsCommander/releases/download/v<version>/`.

Before planning an install, resolve the current `main` commit from GitHub, report its full commit SHA, and read this guide again at that pinned commit. Resolve the latest stable release independently; do not infer it from npm, a branch, a prerelease, or a draft. If the guide, commit, release metadata, asset list, and checksum file do not agree, stop.

For a release tagged `v<version>`, these are the only assets mapped for this workflow:

| Host and route | Exact asset name | Handling rule |
|---|---|---|
| Windows x86_64, agent-managed | `agentscommander-windows-x86_64.exe` | Verify this filename, then place it under the approved name `agentscommander.exe`; do not run it under the release filename |
| Windows x86_64, interactive setup | `Agents.Commander_<version>_x64-setup.exe` | Use only when the user approves the installer, its destination, and any privilege request |
| Linux x86_64, AppImage | `Agents.Commander_<version>_amd64.AppImage` | Continue only after the Linux support warning and explicit confirmation |

`<version>` is the stable tag without its leading `v`. Match an asset name exactly; a wildcard match is not approval. Do not select `.dmg`, `.deb`, `.rpm`, `testeable`, packaged archives, source archives, raw macOS binaries, or another release asset for this workflow.

## Identify the active configuration

AgentsCommander resolves one configuration directory when a process first needs it. The first decisive state wins:

1. A nonblank `AGENTSCOMMANDER_CONFIG_DIR` selects its original value verbatim and skips marker and write probes. An empty or whitespace-only value is ignored.
2. Without that override, AgentsCommander derives `.<native-executable-stem>` beside the real native executable and inspects `portable.txt` in that executable's directory.
   - With the marker, a successful write probe selects the adjacent candidate. Any write-probe failure, or an indeterminate marker state, is a startup error; the runtime never relocates to the home fallback.
   - Without the marker, a successful write probe selects the adjacent candidate. A conclusively unwritable candidate selects the home fallback; an indeterminate failure is a startup error, not permission to relocate.
3. If the runtime cannot derive a usable native-executable parent and stem, it uses the home fallback when one is available. For the normal production identity, that fallback is `$HOME/.agentscommander-new`.

Before an update or uninstall, identify this active selected directory from the existing binary's actual launch environment and runtime evidence, then copy the complete directory to a user-controlled backup and verify the copy. Directory presence alone is not proof of selection. If multiple plausible configuration directories exist or the result is ambiguous, stop before changing the executable, package, or installer state. A relative override also requires the actual launch working directory; stop if that context is unknown. See [Portable instances](features/portable-instances.md#config-directory-rule) for examples and project-path consequences.

An AppImage does not run its native payload from the external `.AppImage` file's directory. The AppImage runtime exposes that external path as `APPIMAGE`, but executes `AppRun` from a [temporary, read-only mounted AppDir](https://docs.appimage.org/reference/architecture.html); `APPDIR` identifies that [mountpoint](https://docs.appimage.org/packaging-guide/environment-variables.html). AgentsCommander's executable-adjacent candidate is therefore inside the mounted payload, not beside the external file. Preserve a selected nonblank override or, for the current unmarked release AppImage without an override, the selected production home fallback; never treat the external AppImage directory or a `portable.txt` placed there as the configuration location.

## Coding Agent contract

### 1. Inspect without changing the machine

Before downloading, creating a directory, installing, overwriting, changing `PATH`, or launching an artifact:

1. Detect and report the OS name and version, native CPU architecture, and process architecture if it differs.
2. Look for an existing AgentsCommander command, executable, package, installation directory, version, and active runtime-selected configuration using the precedence above. Do not perform a broad or destructive filesystem scan. Stop if multiple candidates or ambiguous evidence prevent a unique selection.
3. Apply the support table above. Stop on an unsupported combination. On Linux, explain the partial tier and wait for explicit confirmation before continuing. On macOS, stop the normal install and offer only the tester/contributor path below.
4. Resolve and report the pinned guide commit, stable release tag and URL, exact mapped asset name and URL, and the exact matching record from that release's `SHASUMS256.txt`.
5. Report the exact destination, every command you plan to run, files or directories you plan to create or overwrite, privilege level, `PATH` or system-wide effects, configuration-preservation plan, validation commands, and rollback steps.
6. Explain that current Windows artifacts may be unsigned and that checksum verification is not publisher-compromise protection.
7. Wait for clear approval of that plan.

Missing, ambiguous, or conflicting evidence is a stop condition. Do not guess.

### 2. Keep sensitive choices separate

Approval of the basic plan does not authorize any of these actions. Ask separately before:

- elevation or an administrator prompt;
- a system-wide install or any `PATH` change;
- overwriting an executable, installation directory, or configuration;
- running a Windows artifact whose Authenticode status is not `Valid`;
- continuing on Linux after the partial-support warning; or
- entering the macOS tester/contributor path.

Prefer a user-writable destination and the least privilege that completes the approved plan. Back up the active selected configuration and verify the backup before an update or uninstall. When updating an existing executable, keep a restorable copy until validation succeeds.

### 3. Download, verify, then run

After approval:

1. Download the exact asset and `SHASUMS256.txt` from the same stable release into the approved staging directory. Use HTTPS. A normal GitHub-controlled release-asset redirect is allowed; a third-party mirror is not.
2. Require exactly one checksum record whose filename equals the selected asset name.
3. Compute the asset's SHA-256 digest and compare the complete 64-character value. On a missing, duplicate, malformed, or mismatched record, do not run the asset; report the failure and remove only the unverified files created by this attempt.
4. Inspect Windows Authenticode status. A checksum match does not turn `NotSigned` into `Valid`; obtain the separate unsigned-software consent before launch.
5. Run only the approved commands. For the Windows raw asset, verify it before renaming it to `agentscommander.exe` in the approved destination.
6. Validate with the installed executable's exact path and `--version` or `--help`, then report the observed output and final locations. Do not claim success from a download alone.
7. If validation fails, execute only the approved rollback: restore the previous executable for an update, or remove only the new installation files for a fresh install. Keep existing configuration unless the user separately asks to remove it.

Never bypass SmartScreen, Gatekeeper, an execution policy, certificate checks, or another security control silently. Never elevate automatically, use `curl | shell`, use a mirror, build from source as a fallback, use emulation or a substitute asset, fall back to npm, or install or authenticate a Coding Agent CLI.

## Verify a downloaded asset manually

On Windows, set `$Asset` to the exact downloaded filename and run:

```powershell
Import-Module (Join-Path $PSHOME "Modules\Microsoft.PowerShell.Utility\Microsoft.PowerShell.Utility.psd1") -ErrorAction Stop
$Asset = "Agents.Commander_<version>_x64-setup.exe"
$Records = @(Get-Content -LiteralPath ".\SHASUMS256.txt" |
  Where-Object { $_ -match "^[0-9a-f]{64}  $([regex]::Escape($Asset))$" })
if ($Records.Count -ne 1) { throw "Expected one checksum record for $Asset" }
$Expected = ($Records[0] -split "  ", 2)[0]
$Actual = (Get-FileHash -Algorithm SHA256 -LiteralPath ".\$Asset").Hash.ToLowerInvariant()
if ($Actual -cne $Expected) { throw "SHA-256 mismatch for $Asset" }
"SHA-256 verified: $Asset"
```

Success prints `SHA-256 verified: <asset-name>`. Inspect signature status separately:

```powershell
Import-Module (Join-Path $PSHOME "Modules\Microsoft.PowerShell.Security\Microsoft.PowerShell.Security.psd1") -ErrorAction Stop
Get-AuthenticodeSignature -LiteralPath ".\Agents.Commander_<version>_x64-setup.exe"
```

Current Windows releases may report `NotSigned` until [epic #717](https://github.com/mblua/AgentsCommander/issues/717) is complete. See the [code-signing policy](../CODE_SIGNING_POLICY.md).

On Linux, set `asset` to the exact AppImage filename and run:

```bash
asset='Agents.Commander_<version>_amd64.AppImage'
mapfile -t records < <(awk -v name="$asset" \
  'NF == 2 && $2 == name && length($1) == 64 && $1 !~ /[^0-9a-f]/ { print }' \
  SHASUMS256.txt)
[ "${#records[@]}" -eq 1 ] || { echo "expected one checksum record for $asset" >&2; exit 1; }
expected="${records[0]%%  *}"
actual="$(sha256sum -- "$asset" | awk '{print $1}')"
[ "$actual" = "$expected" ] || { echo "SHA-256 mismatch for $asset" >&2; exit 1; }
printf 'SHA-256 verified: %s\n' "$asset"
```

Success exits 0 and prints `SHA-256 verified: <asset-name>`.

The release checksum detects corruption or a file that differs from the checksum record. Because the asset and `SHASUMS256.txt` come from the same GitHub release, it does not protect you if an attacker can replace both through a compromised publisher or repository account. A valid Authenticode signature is a separate publisher-identity signal; a checksum match is not a substitute for it.

## Manual alternatives

Manual installation is secondary to the reviewed Coding Agent plan:

- On supported Windows x86_64, download one mapped Windows asset and `SHASUMS256.txt` from the same [stable release](https://github.com/mblua/AgentsCommander/releases/latest), verify it, then follow the handling rule above. The setup installer can be removed through **Windows Settings > Apps > Installed apps > Agents Commander > Uninstall**. Identify and back up the active selected configuration before an update or uninstall.
- On Linux x86_64, acknowledge the partial support tier first. After verifying the AppImage, set `asset` to its exact filename and run `chmod +x "$asset"`; success produces no output. Launch it from its exact path. Removing the new AppImage rolls back a fresh file-only install. Its external directory is not the configuration directory: preserve the active override or home fallback selected as described above.
- npm remains available only as a secondary route for Windows x86_64 and Linux x86_64. It is not the recommended first install and must not be an automatic fallback. Read the [npm package boundary](../npm/README.md) before using it.

## Help extend Linux and macOS support

Linux and macOS developers can help turn reproducible gaps into fixes. macOS remains unsupported: choose this path only as a tester or contributor, not as a normal installation.

Open a [GitHub issue](https://github.com/mblua/AgentsCommander/issues) with this report:

```text
OS and version:
Native architecture:
AgentsCommander version or exact release asset:
Exact steps to reproduce:
Expected result:
Actual result:
Relevant logs:
```

Remove tokens, credentials, private paths, and repository content from logs. If you can fix the gap, follow [`CONTRIBUTING.md`](../CONTRIBUTING.md).
