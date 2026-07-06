# Releasing

For maintainers cutting a new AgentsCommander release. Three phases: bump version → push tag → verify the draft release.

This page is the canonical version-bumping procedure. Do not edit the version locations by hand — the script keeps the five files in sync.

## 1. Bump the version

Every release starts with a version bump.

```bash
npm run version:bump -- patch        # 0.8.x  ->  0.8.(x+1)
npm run version:bump -- minor        # 0.x.y  ->  0.(x+1).0
npm run version:bump -- major        # x.y.z  ->  (x+1).0.0
npm run version:bump -- 0.9.0        # explicit X.Y.Z
```

The script writes the same version to every checked location:

| File | Field |
|---|---|
| `package.json` | `version` |
| `package-lock.json` | root `version` and `packages[""].version` |
| `src-tauri/Cargo.toml` | `[package]` version |
| `Cargo.lock` | internal Cargo crate entry (`agentscommander-new`) version |
| `src-tauri/tauri.conf.json` | `version` |

The frontend titlebar reads its version from `tauri.conf.json` at build time, so bumping that one file is enough to update what users see — no source files need manual edits.

## 2. Verify the bump

Before committing, verify every location agrees:

```bash
npm run version:check
```

`version:check` is the same check CI runs on every PR / push that touches a version-relevant file. Running it locally catches a future regression in the bump script before CI does.

## 3. Commit the version bump

Stage every file the script touched and commit in one shot so CI sees them together:

```bash
git add package.json package-lock.json src-tauri/Cargo.toml Cargo.lock src-tauri/tauri.conf.json
git commit -m "chore: bump version to X.Y.Z"
```

The branch-naming rule in [`CONTRIBUTING.md`](../CONTRIBUTING.md) applies. For a public release this commit lands on `main` through a PR (or directly if you have permissions) — never `--no-verify`.

## 4. Push the tag

Releases are automated via GitHub Actions. Push a tag to trigger a build:

```bash
git tag v0.8.43
git push origin v0.8.43
```

This creates a **draft release** with:

- Auto-generated changelog (from commits since the previous tag)
- Windows installers (SignPath signing planned; artifacts may be unsigned until integration is complete)
- Windows raw executables:
  - `target/release/agentscommander.exe`
  - `target/release/agentscommander_testeable.exe`
- Linux `.AppImage`
- macOS `.dmg` (Apple Silicon + Intel) — unsigned today

The workflow file is `.github/workflows/release.yml`.
Every release matrix row passes `--config src-tauri/tauri.prod.conf.json`;
the macOS rows add their `--target` after the production config.

## 5. Verify and publish

The release shows up under [Releases](https://github.com/mblua/AgentsCommander/releases) as a **draft**. Open it and verify:

- **Asset count.** Every platform produced an installer. Re-run the failing job if one is missing.
- **Windows signature status.** Until SignPath integration is active, the installer may be unsigned. Inspect the Authenticode status:
  ```powershell
  Get-AuthenticodeSignature "Agents Commander_X.Y.Z_x64-setup.exe"
  ```
- **Checksums.** Verify downloaded assets against `SHASUMS256.txt`. If a Windows artifact is unsigned, keep the release notes truthful about that status.
- **Signed Windows artifacts.** Once SignPath signing is active, right-click the installer, choose Properties > Digital Signatures, and confirm SignPath Foundation. `Get-AuthenticodeSignature` should report `Valid`.
- **Changelog.** Add curated highlights at the top (the auto-generated list goes underneath). Use the previous release as a tone reference.
- **Tag matches the bump.** If you tagged `v0.8.42` but the binary reports `0.8.41`, abort and re-bump.

Click **Publish release** when verified.

## 6. Update `CHANGELOG.md`

GitHub Releases is the source of truth for per-release detail, but `CHANGELOG.md` at the repo root carries the human-readable summary. Append a section under the new version with the highlights and a link to the release.

## Workgroup-specific builds

The shipper builds a workgroup-suffixed exe alongside the canonical one:

```
target/release/agentscommander.exe                # canonical
target/release/agentscommander_standalone_wg-N.exe # workgroup build
```

**Hard rule**: a workgroup build **never overwrites** the bare `agentscommander_standalone.exe`. The shipper's Step 8 enforces this. If you see the bare exe being replaced by a wg-N build, the shipper's workflow is misconfigured — stop and fix it before publishing.

## Hotfixes

For an urgent fix on an already-released line:

1. Branch from the release tag (e.g. `git checkout -b hotfix/X.Y.Z v0.8.42`).
2. Land the fix on `hotfix/X.Y.Z` (the branch is exempt from the issue-number rule per [`CONTRIBUTING.md`](../CONTRIBUTING.md)).
3. Cherry-pick to `main` or rebase as appropriate.
4. Bump to a patch version and tag a new release as above.

## See also

- [`CONTRIBUTING.md`](../CONTRIBUTING.md) — branch naming and Husky hook
- [`CODE_SIGNING_POLICY.md`](../CODE_SIGNING_POLICY.md) - pending SignPath policy
- [`CHANGELOG.md`](../CHANGELOG.md)
