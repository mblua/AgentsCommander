# Releasing

Use the existing release workflow and npm Trusted Publishing. The sequence is:
prepare evidence -> version PR and CI -> annotated tag -> GitHub and npm verification.

The `release-agentscommander-to-prod` agent skill performs this sequence. A normal
request can be: "Release the next patch from main to GitHub and npm." The agent
resolves current versions and scope, prepares the evidence and follows the checks
below. The npm trust is an existing configuration, not a per-release setup step.

## 1. Prepare evidence before the version bump

Read current remote main, its complete `Unreleased` changelog and the previous
public release. From the `prepare-agentscommander-release` skill, run:

```text
node scripts/release.mjs --candidate <config.version.v1.X.Y.Z.json>
```

The agent prepares the config and source files from live facts. This script lives
in the skill, not this application repository. The explicit candidate mode
produces the eight V1 files required by `.github/workflows/release.yml`:

- `CHANGELOG.release.md`
- `candidate-assets.v1.json`
- `input-manifest.v1.json`
- `predecessor-assets.v1.json`
- `release-authority-v1.txt`
- `release-body.md`
- `release-plan.md`
- `SHA256SUMS`

The positional V2 preparation command produces a different four-file review
handoff; it cannot supply the release workflow. Neither command publishes.

Verify the generated checksums, scope and version. Add the exact V1 files under
`docs/releases/vX.Y.Z/`, preserving LF bytes. Existing accepted bundles must not
be overwritten or relabeled. The workflow also requires the manifest-derived
`plans/<issue>-v<major><minor><patch>-release-hardening.md`; write a short release
implementation record there, linked to the bundle and intended checks.

## 2. Bump and verify all versions

Use the script rather than editing version fields by hand:

```bash
npm run version:bump -- patch
# Or: minor, major, or an explicit X.Y.Z
npm run version:check
```

Use the same target version that was prepared. The bumper updates seven files
containing eight values:

| File | Field |
|---|---|
| `package.json` | `version` |
| `package-lock.json` | root `version` and `packages[""].version` |
| `npm/package.json` | npm wrapper `version` |
| `npm/install.js` | `VERSION` constant |
| `src-tauri/Cargo.toml` | `[package]` version |
| `Cargo.lock` | internal `agentscommander-new` crate version |
| `src-tauri/tauri.conf.json` | `version` |

Move the prepared Unreleased body into the new root changelog section, leaving a
fresh Unreleased section. Include the evidence, canonical plan, changelog and all
seven version files in one release PR. Follow [CONTRIBUTING.md](../CONTRIBUTING.md)
for branch naming and land through protected main after required CI passes.

Mechanism-only changes do not bump the application version.

## 3. Push one annotated tag for the verified main commit

After the PR lands, verify its exact main commit, version synchronization and
evidence again. Query remote tags, GitHub Releases and npm for collisions; an
authentication or network error never establishes absence.

The deployed push workflow requires the annotation to equal the exact
`release-authority-v1.txt` bytes. Read them from Git to avoid Windows line-ending
conversion. Replace the placeholders with the verified commit and an authorized
scratch path:

```bash
git show <verified-main-commit>:docs/releases/vX.Y.Z/release-authority-v1.txt > <authorized-scratch>/tag-message.txt
git tag -a --cleanup=verbatim vX.Y.Z -F <authorized-scratch>/tag-message.txt <verified-main-commit>
git push origin refs/tags/vX.Y.Z
```

A lightweight tag or a generic annotation fails this workflow. Do not force,
move or delete a release tag. The guard checks that its peeled commit is reachable
from current remote main. Pushing this tag starts publication; it is not a test.

## 4. Follow the automated publication

The seven jobs run in order:

`guard -> release-coordinator -> build -> checksums -> publish-github -> verify-release -> publish-npm`

CI creates the draft, uploads and verifies the exact expected assets, publishes
an immutable GitHub Release, verifies it, then publishes the npm wrapper using
the existing trust. The wrapper needs the public GitHub binaries first.

Wait for the workflow and inspect failures before deciding whether a job can
resume. Never click Publish release or run npm publish manually to skip a failed
step. Published versions, tags and assets are not replaceable.

The matrix produces Windows installers and raw executables, Linux artifacts and
macOS artifacts for both architectures. Expected names and count come from the
candidate manifest. Verify each downloaded asset against `SHASUMS256.txt`.
Checksums and publisher signatures are separate facts; keep any unsigned status
truthful in release notes.

Build output does not define support tiers. Follow the
[canonical platform contract](install-with-agent.md#support-gates):
documented Windows x86_64 is fully supported, Linux x86_64 is partial/in progress,
and macOS is not supported yet. Every matrix row uses
`src-tauri/tauri.prod.conf.json`; macOS additionally selects its target.

## 5. Verify GitHub, npm and installation

Confirm the public immutable GitHub Release has exactly the expected assets.
Query npm directly for the version and latest dist-tag, verify its tarball and
provenance, and inspect the workflow's clean-install result. Complete the
install/version smoke check on a supported platform before reporting success.

A version visible on GitHub alone is not a completed release to npm.

## Room-specific builds

The shipper builds a room-suffixed exe alongside the canonical one:

```text
target/release/agentscommander.exe                # canonical
target/release/agentscommander_standalone_wg-N.exe # room build
```

**Hard rule**: a room build **never overwrites** the bare `agentscommander_standalone.exe`. The shipper's Step 8 enforces this. If you see the bare exe being replaced by a wg-N build, the shipper's workflow is misconfigured — stop and fix it before publishing.

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
