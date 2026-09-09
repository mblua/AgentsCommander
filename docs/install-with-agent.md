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

Before planning an install, resolve the current `main` commit from GitHub, report its full commit SHA, and read this guide again at that pinned commit. Resolve the latest stable release independently; do not infer it from npm, a branch, a prerelease, or a draft. Follow the canonical storage and upgrade contract below for the exact existing and selected versions; `main` is not evidence for a published binary. If the guide, commit, release metadata, tag source, asset list, and checksum file do not agree, stop.

For a release tagged `v<version>`, these are the only assets mapped for this workflow:

| Host and route | Exact asset name | Handling rule |
|---|---|---|
| Windows x86_64, agent-managed | `agentscommander-windows-x86_64.exe` | Verify this filename, then place it under the approved name `agentscommander.exe`; do not run it under the release filename |
| Windows x86_64, interactive setup | `Agents.Commander_<version>_x64-setup.exe` | Use only when the user approves the installer, its destination, and any privilege request |
| Linux x86_64, Debian/Ubuntu family | `Agents.Commander_<version>_amd64.deb` | Use `apt` after checksum verification and separate elevation/system-wide consent |
| Linux x86_64, Fedora/RHEL family | `Agents.Commander-<version>-1.x86_64.rpm` | Use `dnf` after checksum verification and separate elevation/system-wide consent |
| Linux x86_64, other distributions | `Agents.Commander_<version>_amd64.AppImage` | Continue only after the Linux support warning and explicit confirmation |

`<version>` is the stable tag without its leading `v`. Match an asset name exactly; a wildcard match is not approval. Do not select `.dmg`, `testeable`, packaged archives, source archives, raw Linux/macOS binaries, or another release asset for this workflow.

## Select the Linux route

On an authorized host, read `/etc/os-release` as data without executing it. Report `ID` and `ID_LIKE`. Use `ID` first: `debian` or `ubuntu` selects DEB; `fedora` or `rhel` selects RPM. For derivatives without a directly mapped `ID`, use recognized whitespace-separated `ID_LIKE` family tokens: `debian`/`ubuntu` selects DEB and `fedora`/`rhel` selects RPM. If neither identifies a mapped family, select AppImage. Conflicting family evidence, including an `ID` from one mapped family and `ID_LIKE` from the other, stops for clarification. If distribution information is missing or ambiguous, stop rather than guess.

Every route requires native x86_64/AMD64, the Linux partial/in-progress warning, and explicit confirmation. If the selected exact asset is absent from the independently resolved stable release, stop; do not switch formats. Check that the selected route's required tools (`apt` and `dpkg-deb`, or `dnf` and `rpm`) are available; otherwise report the missing dependency and stop. Do not install tools or repair dependencies automatically.

## Storage, backup, and upgrades

[Issue #1118 is the canonical owner of storage, backup, upgrade, and uninstall-state behavior](https://github.com/mblua/AgentsCommander/issues/1118). Follow that contract for the exact existing and selected versions; do not infer published-binary behavior from `main`, invent a storage destination, or duplicate migration rules here.

Establish the existing installation's exact executable path, version, and provenance from consistent read-only evidence: installed package/product records for installers, installed npm package and launcher records for npm, or the original release asset and matching digest for raw files/AppImages. Corroborate those records with the official release tag, exact asset, and checksum. Report explicitly when no existing installation is found. Do not guess a `--version` flag; use `--help` for executable validation and package/release records for version evidence.

Before an update or uninstall, identify and protect existing data under #1118, make the required backup, and verify it. Obtain a separately reviewed rollback for updates. If version/provenance, state ownership, preservation, or canonical guidance is unavailable, conflicting, ambiguous, or insufficient for the existing installation, stop that update/uninstall before mutation. Do not silently move or delete user state.

## Coding Agent contract

### 1. Inspect without changing the machine

Before downloading, creating a directory, installing, overwriting, changing `PATH`, or launching an artifact:

1. Detect and report the OS name and version, native CPU architecture, and process architecture if it differs. On Linux, report the distribution evidence and select the route above.
2. Look for an existing AgentsCommander command, executable, package, and installation directory without performing a broad or destructive filesystem scan. Establish and report the exact existing binary version and provenance with the route-specific evidence above before planning changes. Stop an update or uninstall if that evidence is unavailable, conflicting, or ambiguous.
3. Apply the support table above. Stop on an unsupported combination. On Linux, explain the partial tier and wait for explicit confirmation before continuing. On macOS, stop the normal install and offer only the tester/contributor path below.
4. Resolve and report the pinned guide commit, exact selected stable release tag and URL, exact mapped asset name and URL, and the exact matching record from that release's `SHASUMS256.txt`.
5. Apply #1118 to the existing and selected versions and report the data-preservation, backup, and upgrade/uninstall requirements. Stop if canonical guidance is insufficient or ambiguous.
6. Report the exact destination, every command you plan to run, files or directories you plan to create or overwrite, privilege level, `PATH` or system-wide effects, configuration-preservation or migration plan, validation commands, and rollback steps.
7. Explain that current Windows artifacts may be unsigned and that checksum verification is not publisher-compromise protection.
8. Wait for clear approval of that plan.

Missing, ambiguous, or conflicting evidence is a stop condition. Do not guess.

### 2. Keep sensitive choices separate

Approval of the basic plan does not authorize any of these actions. Ask separately before:

- elevation or an administrator prompt;
- a system-wide install or any `PATH` change;
- overwriting an executable, installation directory, or configuration;
- running a Windows artifact whose Authenticode status is not `Valid`;
- continuing on Linux after the partial-support warning; or
- entering the macOS tester/contributor path.

Prefer a user-writable destination and the least privilege that completes the approved plan. Protect existing data and verify the required backup under #1118 before an update or uninstall. When updating an existing executable, keep a restorable copy until validation succeeds.

### 3. Download, verify, then run

After approval:

1. Download the exact asset and `SHASUMS256.txt` from the same stable release into the approved staging directory. Use HTTPS. A normal GitHub-controlled release-asset redirect is allowed; a third-party mirror is not.
2. Require exactly one checksum record whose filename equals the selected asset name.
3. Compute the asset's SHA-256 digest and compare the complete 64-character value. On a missing, duplicate, malformed, or mismatched record, do not run the asset; report the failure and remove only the unverified files created by this attempt.
4. Inspect Windows Authenticode status. A checksum match does not turn `NotSigned` into `Valid`; obtain the separate unsigned-software consent before launch.
5. Run only the approved commands. For the Windows raw asset, verify it before renaming it to `agentscommander.exe` in the approved destination. For Linux, follow the selected route below; package inspection must precede installation and its resolved package name must be approved.
6. Validate the installed executable's exact path with `--help`, then re-check the route-specific version/provenance evidence against the approved release and report the observed output and final locations. Do not claim success from a download alone.
7. If validation fails, execute only the approved rollback: for DEB/RPM, use only the approved package rollback below; for a file-only fresh install, remove only the newly installed file; for a file-only update, restore the approved previous executable. Preserve user state under #1118.

Never bypass SmartScreen, Gatekeeper, an execution policy, certificate checks, or another security control silently. Never elevate automatically, use `curl | shell`, use a mirror, build from source as a fallback, use emulation or a substitute asset, fall back to npm, or install or authenticate a Coding Agent CLI.

## Install and remove the selected Linux asset

Use these commands only after the preflight, same-release checksum verification, and approvals above. Replace placeholders with the exact verified filename or package name; do not run the placeholders literally.

For Debian/Ubuntu, inspect the downloaded package before installation:

```sh
dpkg-deb -f ./<exact-deb-filename> Package Version Architecture Depends
```

For Fedora/RHEL:

```sh
rpm -qp --queryformat '%{NAME} %{VERSION}-%{RELEASE} %{ARCH}\n' ./<exact-rpm-filename>
```

Check the metadata against the approved stable version and native architecture (`amd64` for DEB, `x86_64` for RPM; the mapped RPM has release `1`). Show the resolved package name in the plan and obtain approval for the concrete install/removal commands before installing. Stop on conflicting metadata. The package name comes from this record, not the display name or filename.

| Route | Install | Remove the verified package |
|---|---|---|
| Debian/Ubuntu | `sudo apt install ./<exact-deb-filename>` | `sudo apt remove <verified-package-name>` |
| Fedora/RHEL | `sudo dnf install ./<exact-rpm-filename>` | `sudo dnf remove <verified-package-name>` |

These are system-wide package transactions requiring separate elevation and system-wide consent. The package manager resolves declared dependencies and may propose additional package changes. Review the proposed transaction before confirming; stop and seek approval if its effects exceed the approved plan. Do not elevate or repair dependencies automatically.

After installation, use `dpkg-query -L <verified-package-name>` or `rpm -ql <verified-package-name>` to inspect package-owned files and identify the installed executable's exact path. Validate that path with `--help`. Recheck version and architecture using `dpkg-query -W -f='${Package} ${Version} ${Architecture}\n' <verified-package-name>` or `rpm -q --queryformat '%{NAME} %{VERSION}-%{RELEASE} %{ARCH}\n' <verified-package-name>`. Report the package record, executable path, and observed validation results.

On validation failure, use only the approved package rollback. Removing a fresh package uses its verified name as above; dependency transactions do not necessarily reverse automatically. Updates require a separately reviewed rollback and #1118 preservation guidance. Before any removal, apply the storage/backup and ambiguity stops above. Do not purge, autoremove, or delete user state.

For the AppImage route, after verification set `asset` to its exact filename and run `chmod +x "$asset"`; success produces no output. Launch and validate it with `--help` using its exact path. Removing only the new AppImage reverses a fresh file-only install; updates and uninstall-state handling follow #1118.

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

On Linux, set `asset` to the selected exact DEB, RPM, or AppImage filename (the DEB below is an example) and run:

```bash
asset='Agents.Commander_<version>_amd64.deb'
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

- On supported Windows x86_64, download one mapped Windows asset and `SHASUMS256.txt` from the same [stable release](https://github.com/mblua/AgentsCommander/releases/latest), verify it, then follow the handling rule above. The setup installer can be removed through **Windows Settings > Apps > Installed apps > Agents Commander > Uninstall**. Identify the exact existing version and follow #1118 for preservation, backup, updates, and uninstall-state handling; stop if its guidance is insufficient.
- On Linux x86_64, acknowledge the partial support tier, select DEB/RPM/AppImage using the distribution rules above, and verify the selected exact asset. Follow [the route-specific installation and removal steps](#install-and-remove-the-selected-linux-asset), including separate elevation/system-wide consent for packages and #1118 preservation requirements.
- npm remains available only as a secondary route for Windows x86_64 and Linux x86_64. It is not the recommended first install and must not be an automatic fallback. Read the [npm package boundary](../npm/README.md) before using it; [#1118 owns its storage and upgrade contract](https://github.com/mblua/AgentsCommander/issues/1118). Stop an existing-install change if that guidance is insufficient.

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
