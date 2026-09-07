# Agents Commander v0.30.5

### Changed

- **Releases publish to npm again: the release pipeline was repaired and hardened end to end during its first full execution.** The guard no longer probes a repository setting its workflow token can never read ([#1811](https://github.com/mblua/AgentsCommander/issues/1811)), every build step now runs on the macOS runners' bash 3.2 ([#1813](https://github.com/mblua/AgentsCommander/issues/1813)), the windows runner's own `NPM_CONFIG_PREFIX` no longer aborts the npm-registry guard and bundle assets are matched by their GitHub-sanitized upload names with explicit fail-closed errors ([#1815](https://github.com/mblua/AgentsCommander/issues/1815)), the single bundler updater archive satisfies every ledger `.app.tar.gz` alias and the draft coordinator absorbs the releases-list read-after-write race ([#1817](https://github.com/mblua/AgentsCommander/issues/1817)), and a fresh run purges stale draft assets before its byte-exact uploads ([#1819](https://github.com/mblua/AgentsCommander/issues/1819)). Version 0.30.4 exists only as a GitHub Release: its npm publication was blocked by a toolchain-contract mismatch in release verification, so this version supersedes it on npm and the registry goes 0.30.3 → 0.30.5.

### Fixed

- **A session no longer keeps the amber pending-review dot forever while the backend considers it active.** The sidebar's waiting mirror is reconciled from the backend session list on every poll, so a latched `pendingReview` state the backend no longer reports now clears instead of surviving polling and application restarts. ([#1779](https://github.com/mblua/AgentsCommander/issues/1779))

## Included scope

- fix: fix(#1779): reconcile the sidebar waiting mirror from the backend session list ([#1779](https://github.com/mblua/AgentsCommander/issues/1779), [PR #1809](https://github.com/mblua/AgentsCommander/pull/1809))
- maintenance: ci(1811): drop the immutable-releases admin probe the workflow token cannot make ([#1811](https://github.com/mblua/AgentsCommander/issues/1811), [PR #1812](https://github.com/mblua/AgentsCommander/pull/1812))
- maintenance: ci(1813): make the build job bash-3.2 portable for the macOS runners ([#1813](https://github.com/mblua/AgentsCommander/issues/1813), [PR #1814](https://github.com/mblua/AgentsCommander/pull/1814))
- maintenance: ci(1815): fix the three build-leg defects in never-exercised producer paths ([#1815](https://github.com/mblua/AgentsCommander/issues/1815), [PR #1816](https://github.com/mblua/AgentsCommander/pull/1816))
- maintenance: ci(1817): alias every ledger app.tar.gz to the single bundler archive; retry draft list visibility ([#1817](https://github.com/mblua/AgentsCommander/issues/1817), [PR #1818](https://github.com/mblua/AgentsCommander/pull/1818))
- maintenance: ci(1819): purge stale draft assets on a fresh run before byte-exact uploads ([#1819](https://github.com/mblua/AgentsCommander/issues/1819), [PR #1820](https://github.com/mblua/AgentsCommander/pull/1820))
- docs: docs(1823): fill Unreleased for 0.30.5 (pipeline repair, pending-review latch fix) ([#1823](https://github.com/mblua/AgentsCommander/issues/1823), [PR #1824](https://github.com/mblua/AgentsCommander/pull/1824))

## Install from npm

```text
npx @mblua/agentscommander@0.30.5
```

## Verification identity

- Release issue: https://github.com/mblua/AgentsCommander/issues/1823
- Reviewed evidence set: `bound by the exact release-authority-v1 review-set-sha256 field`
- Candidate assets: 17
- Predecessor: `v0.30.4`

The workflow must publish this body verbatim, make the GitHub Release public and immutable before npm, and attach provenance for the exact assets and package.
