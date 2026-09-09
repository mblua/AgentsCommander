# Release v0.31.0 — implementation record

Release issue: [#1893](https://github.com/mblua/AgentsCommander/issues/1893).

## Why this version

Main carries 22 merged PRs since `v0.30.5`, several of them user-facing features (restart
auto-resume #1793, selected-row rail settings #1796, Muse Code preset #1860, shared Golden Rule
locations #1795, Root Agent menu parity #1871, `agentscommander --version` #1826), so the next
normal SemVer is the minor `0.31.0`. Predecessor on GitHub and npm is `v0.30.5` / `0.30.5`
(npm `latest`; 17 release assets).

## Release identity

`docs/releases/v0.31.0/` and `plans/1621-v0310-release-hardening.md` on main were a stale,
never-shipped candidate prepared under #1621 for scope #1614, which shipped in 0.30.4. Its
predecessor and planning base no longer exist as recorded, so it could not pass the deployed
guard. This PR replaces that bundle and plan with evidence generated from live facts; #1621 is
closed as superseded by #1893. No published version, tag or Release is touched.

## Evidence

`docs/releases/v0.31.0/` holds the eight V1 files emitted by
`prepare-agentscommander-release --candidate` from live facts at planning base
`f5c4720f46a164cb03a01b3d2283d73653133e15` (remote main, pre-bump), predecessor v0.30.5 (17 assets, tag object
`69e00f8f2c8cd3c72ef611d310a0487247616db1` → `535ceb1502049b813154ab32c6bb2197300c6f33`), gh CLI pin 2.100.0,
`workflow-base-blob af2a50413e83837a4ab32e0f0e0059172eb47cee` (current deployed `release.yml`). `SHA256SUMS` verified
after copying.

## Scope

The 22 PRs merged after `v0.30.5` (#1827, #1836, #1837, #1838, #1843, #1845, #1846, #1847,
#1848, #1849, #1864, #1866, #1869, #1870, #1872, #1874, #1878, #1880, #1885, #1886, #1888,
#1890) plus the changelog PR #1894, all listed in the bundle's scope source.

## Bump, checks and publication path

This PR adds the bundle, this plan, the 0.31.0 changelog section, the refreshed
version-anchored section of `npm/README.md` (the bumper never touches it; npmjs.com renders the
packaged README, #1834), and the `npm run version:bump -- 0.31.0` result (seven files, eight
values), gated by `npm run version:check` and the full required CI. Landing is followed by one
annotated tag whose message is exactly `docs/releases/v0.31.0/release-authority-v1.txt`, the
existing `release.yml` seven-job chain, npm publication via OIDC Trusted Publishing only, and
registry/install verification. No manual publication anywhere.
