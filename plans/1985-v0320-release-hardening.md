# Release v0.32.0 implementation record

Issue: #1985. The user requested a routine release from current origin/main.

The unchanged existing publication workflow consumes the exact eight files in
`docs/releases/v0.32.0/`. The public V1 candidate generator collected live Git,
GitHub and npm evidence twice before the version bump; both passes agreed.
The bundle covers the 34 merged scope PRs recorded in its input manifest.
Preparation base: `0ba01f0a1a8218346397ecc96b7af450a203773f`.

SHA256SUMS digest: `f57ff184ebcc47605baa6c5f88644c068807b8d7ca8d0e74aa12ab0e56746821`.

This PR synchronizes the seven version files/eight values with the existing
version bumper, installs the generated changelog, and updates npm/README.md
for the new HOME configuration selection without automatic migration.

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
