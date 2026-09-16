# Release v0.34.0 implementation record

Issue: #2071. The user requested a routine release from current origin/main,
with explicit authorization to publish the GitHub Release and the npm version.

The unchanged existing publication workflow consumes the exact eight files in
`docs/releases/v0.34.0/`. The public V1 candidate generator collected live Git,
GitHub and npm evidence twice before the version bump; both passes agreed.
The bundle covers the 19 merged scope PRs recorded in its input manifest.
Preparation base: `2c499950` (merge of #2072, which filled Unreleased).
The GitHub CLI pin is `2.101.0`, the latest stable immutable cli/cli Release at
preparation time, above every published advisory patch (13 advisories, floor `2.98.0`).

SHA256SUMS digest: `4a6ea1bcdd277536d41d1c9994147a0f7a8ccc879b1ab9b9dab113179cebafee`.

This PR synchronizes the seven version files/eight values with the existing
version bumper, installs the generated changelog, and updates npm/README.md
for the `0.34.0` version-anchored section.

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
