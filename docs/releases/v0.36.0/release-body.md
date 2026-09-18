# Agents Commander v0.36.0

### Added

- **The "Update available" toast has a Copy button** that copies the upgrade command to the clipboard. The toast stays open so the command remains readable, and repeated copies reuse one confirmation toast instead of stacking. ([#2135](https://github.com/mblua/AgentsCommander/issues/2135))

### Fixed

- **The CI ring no longer blanks for minutes under GitHub's secondary rate limit.** The remote sweeper reserves every `gh` call against a 30-calls-per-minute budget, prioritizes repositories with confirmed running CI, and backs off a secondary 403 from 60 s instead of jumping to the 900 s cap. ([#2152](https://github.com/mblua/AgentsCommander/issues/2152))
- **Blind-gap notices no longer fire on healthy polling rounds.** The gap threshold rises to 600 s and the default branch-staleness interval drops to 260 s, so the notice means two consecutive lost checks. Installs that already persist `branchStalenessIntervalSecs: 300` keep that value. ([#2149](https://github.com/mblua/AgentsCommander/issues/2149))
- **CI activity counts only runs on the repository's current branch**, matched by exact name, so a branch sitting on the default branch's tip reports no CI and sends no notice. ([#2126](https://github.com/mblua/AgentsCommander/issues/2126))
- **No branch-stale notice on the default branch itself.** The repo chip still shows the orange bar with its behind-count, and an orchestrator row takes the working wash while its repo chip shows CI running. ([#2131](https://github.com/mblua/AgentsCommander/issues/2131))
- **The CI working tint also reaches the Orchestrators strip**, so one orchestrator can no longer appear tinted in the room tree and untinted in the strip. ([#2151](https://github.com/mblua/AgentsCommander/issues/2151))
- **Short status-line bursts after a long silence no longer keep idle sessions awake.** Output following at least 60 s of silence is held as a pending burst and counts as activity only once it reaches 1024 bytes or lasts 3 s, so auto-close and busy/idle edges ignore a coding agent's periodic update check. The per-agent `idleBurst` settings are overridable in `agents.local.json`. ([#2124](https://github.com/mblua/AgentsCommander/issues/2124))
- **Internal system notices survive case-skewed project paths.** A project path spelled with different letter case than the folder on disk no longer drops context alerts and remote-activity notices; filesystem identity still decides the match. ([#2113](https://github.com/mblua/AgentsCommander/issues/2113))
- **The project `.ac/.gitignore` ignores `seed-manifest.toml`**, migrating the previous un-ignore block while preserving user bytes. ([#2090](https://github.com/mblua/AgentsCommander/issues/2090))
- **Documentation corrections** for the seed manifest, portable readme storage after the configuration-directory change, macOS screenshot support, and the remote-activity settings, chip signals and notices. ([#2112](https://github.com/mblua/AgentsCommander/issues/2112), [#2080](https://github.com/mblua/AgentsCommander/issues/2080), [#2087](https://github.com/mblua/AgentsCommander/issues/2087), [#2085](https://github.com/mblua/AgentsCommander/issues/2085))
- **Frontend static-analysis findings were cleared and two test suites made deterministic**, with no behavior change. ([#2108](https://github.com/mblua/AgentsCommander/issues/2108), [#2011](https://github.com/mblua/AgentsCommander/issues/2011), [#1582](https://github.com/mblua/AgentsCommander/issues/1582))

## Included scope

- maintenance: Bound selection waits to the asserted state instead of yield spins ([#1582](https://github.com/mblua/AgentsCommander/issues/1582), [PR #2132](https://github.com/mblua/AgentsCommander/pull/2132))
- maintenance: Derive the PTY TTL fixture timestamps from a single instant ([#2011](https://github.com/mblua/AgentsCommander/issues/2011), [PR #2163](https://github.com/mblua/AgentsCommander/pull/2163))
- docs: Match the portable readme storage guidance to the post-#1868 resolver ([#2080](https://github.com/mblua/AgentsCommander/issues/2080), [PR #2130](https://github.com/mblua/AgentsCommander/pull/2130))
- docs: Document remote-activity settings, chip signals and injected notices ([#2085](https://github.com/mblua/AgentsCommander/issues/2085), [PR #2118](https://github.com/mblua/AgentsCommander/pull/2118))
- docs: Document macOS screenshot capture support ([#2087](https://github.com/mblua/AgentsCommander/issues/2087), [PR #2123](https://github.com/mblua/AgentsCommander/pull/2123))
- fix: Ignore .ac/seed-manifest.toml in the project .ac/.gitignore and migrate the legacy block ([#2090](https://github.com/mblua/AgentsCommander/issues/2090), [PR #2104](https://github.com/mblua/AgentsCommander/pull/2104))
- maintenance: Clear five SonarCloud frontend findings without behavior changes ([#2108](https://github.com/mblua/AgentsCommander/issues/2108), [PR #2147](https://github.com/mblua/AgentsCommander/pull/2147))
- docs: Fix stale seed-manifest passages after #2090 and #1480 ([#2112](https://github.com/mblua/AgentsCommander/issues/2112), [PR #2165](https://github.com/mblua/AgentsCommander/pull/2165))
- fix: Tolerate case-skewed project paths when matching internal system notices ([#2113](https://github.com/mblua/AgentsCommander/issues/2113), [PR #2164](https://github.com/mblua/AgentsCommander/pull/2164))
- fix: Ignore short idle status-line bursts for auto-close and busy edges ([#2124](https://github.com/mblua/AgentsCommander/issues/2124), [PR #2128](https://github.com/mblua/AgentsCommander/pull/2128))
- fix: Scope CI activity to the repository's current branch ([#2126](https://github.com/mblua/AgentsCommander/issues/2126), [PR #2127](https://github.com/mblua/AgentsCommander/pull/2127))
- fix: Suppress the branch-stale notice on the default branch and tint the orchestrator row on CI ([#2131](https://github.com/mblua/AgentsCommander/issues/2131), [PR #2142](https://github.com/mblua/AgentsCommander/pull/2142))
- feature: Add a Copy button that copies the upgrade command from the update toast ([#2135](https://github.com/mblua/AgentsCommander/issues/2135), [PR #2150](https://github.com/mblua/AgentsCommander/pull/2150))
- fix: Raise the blind-gap threshold to 600 s and the staleness interval to 260 s ([#2149](https://github.com/mblua/AgentsCommander/issues/2149), [PR #2153](https://github.com/mblua/AgentsCommander/pull/2153))
- fix: Apply the CI working tint on the Orchestrators strip ([#2151](https://github.com/mblua/AgentsCommander/issues/2151), [PR #2154](https://github.com/mblua/AgentsCommander/pull/2154))
- fix: Cap gh spend and stop blanking the CI ring on a secondary rate-limit 403 ([#2152](https://github.com/mblua/AgentsCommander/issues/2152), [PR #2159](https://github.com/mblua/AgentsCommander/pull/2159))

## Install from npm

```text
npx @mblua/agentscommander@0.36.0
```

## Verification identity

- Release issue: https://github.com/mblua/AgentsCommander/issues/2168
- Reviewed evidence set: `bound by the exact release-authority-v1 review-set-sha256 field`
- Candidate assets: 17
- Predecessor: `v0.35.0`

The workflow must publish this body verbatim, make the GitHub Release public and immutable before npm, and attach provenance for the exact assets and package.
