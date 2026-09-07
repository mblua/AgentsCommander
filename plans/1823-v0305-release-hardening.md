# Release v0.30.5 — implementation record

Release issue: [#1823](https://github.com/mblua/AgentsCommander/issues/1823).

## Why this version

v0.30.4's GitHub Release published (immutable, 17 assets, `gh release verify` green) but its npm
publication was permanently blocked: `verify-release` in the deployed workflow requires the
toolchain contract `exact-version-archive-sha256-install-self-check` while the v0.30.4 evidence
bundle declared `config-pin-latest-immutable-release-checksum-advisory-inventory-self-check`
(run 34047061557). The tag is frozen by release immutability, so v0.30.5 supersedes v0.30.4 on
npm; the registry goes 0.30.3 → 0.30.5. The preparation tool now emits the deployed workflow's
contract (skill-side fix; its test suite passes).

## Evidence

`docs/releases/v0.30.5/` holds the eight V1 files emitted by
`prepare-agentscommander-release --candidate` from live facts at planning base
`e3b528599ab9a95cdbe3e58f19ad88845acae375` (remote main, pre-bump), predecessor v0.30.4
(17 assets), gh CLI pin 2.100.0, `workflow-base-blob b17f2d4a5c441f0a427559e73b88d168ba165190`
(current deployed `release.yml`). `SHA256SUMS` verified after copying.

## Scope

`#1779` pending-review latch fix (PR #1809) plus the release-pipeline repairs
`#1811/#1813/#1815/#1817/#1819` (PRs #1812/#1814/#1816/#1818/#1820), all exercised in
production by runs 34038860830 → 34047061557, and the changelog PR #1824. Two PR-less
test-only commits for #1774 ride along without user-facing effect.

## Bump, checks and publication path

This PR adds the bundle, this plan, the 0.30.5 changelog section, and the
`npm run version:bump -- 0.30.5` result (seven files, eight values), gated by
`npm run version:check` and the full required CI. Landing is followed by one annotated tag
whose message is exactly `docs/releases/v0.30.5/release-authority-v1.txt`, the existing
`release.yml` seven-job chain, npm publication via OIDC Trusted Publishing only, and
registry/install verification. No manual publication anywhere.
