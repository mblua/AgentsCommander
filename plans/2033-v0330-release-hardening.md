# Release v0.33.0 implementation record

Issue: #2033. The user requested a routine release from current origin/main.

The unchanged existing publication workflow consumes the exact eight files in
`docs/releases/v0.33.0/`. The public V1 candidate generator collected live Git,
GitHub and npm evidence twice before the version bump; both passes agreed.
The bundle covers the 21 merged scope PRs recorded in its input manifest.
Preparation base: `fcf96beb` (merge of #2034, which filled Unreleased).
The GitHub CLI pin is `2.101.0`, the latest stable immutable cli/cli Release at
preparation time, above every published advisory patch (13 advisories, floor `2.98.0`).

SHA256SUMS digest: `1f0aba7eaa8706c1a21e995d9c391b127fd61a869aab8b3411fce47b93dab25a`.

This PR synchronizes the seven version files/eight values with the existing
version bumper, installs the generated changelog, and updates npm/README.md
for the macOS `.app` bundle launcher and the suffixed-executable refusal rule.

Validation: version:check, evidence SHA-256 and exact Git-byte verification,
npm pack, npm publish --dry-run, and all required PR CI before landing.
The documented administrator self-review exception applies because no second
human reviewer is configured; it does not waive any failed check. No independent
review receipt or GUI acceptance result is claimed.

After landing, revalidate current main, absence of the new Git tag, GitHub Release
and npm version, and the exact bundle. Create one annotated tag using the exact
release-authority-v1.txt bytes from Git with --cleanup=verbatim. The existing
workflow builds the declared assets, verifies checksums, publishes the immutable
GitHub Release, and publishes npm using the existing OIDC Trusted Publisher.
Completion requires direct GitHub/npm verification and a clean-install version
smoke check. Do not publish npm manually, replace assets, or force a tag.
