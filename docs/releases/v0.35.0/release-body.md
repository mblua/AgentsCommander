# Agents Commander v0.35.0

### Added

- **Room repositories show remote CI and branch staleness.** When the GitHub CLI (`gh`) is available, a background sweeper checks whether CI is running on each room repository's exact HEAD and whether its default branch has commits the branch lacks. The repo chip shows a yellow ring while CI runs and an orange left bar when the branch is stale; an unknown state changes nothing, and without `gh` the feature stays inert. ([#2064](https://github.com/mblua/AgentsCommander/issues/2064), [#2082](https://github.com/mblua/AgentsCommander/issues/2082), [#2084](https://github.com/mblua/AgentsCommander/issues/2084))
- **The room orchestrator receives remote-activity notices** when CI starts, when CI finishes and when the branch goes stale, with editable message texts. ([#2083](https://github.com/mblua/AgentsCommander/issues/2083))
- **Screenshot capture works on macOS**, with a Screen Recording permission check at capture time and Retina-aware overlay sizing. Real-Mac acceptance is still pending. ([#2086](https://github.com/mblua/AgentsCommander/issues/2086))
- **A global npm install on Linux creates a desktop menu entry.** `agentscommander.desktop` is written under `$XDG_DATA_HOME/applications` (or `~/.local/share/applications`) with the packaged icon. It is skipped for local installs and with `AGENTSCOMMANDER_NO_SHORTCUT=1`, never fails the install, and must be removed by hand. ([#2066](https://github.com/mblua/AgentsCommander/issues/2066))

### Removed

- **The `portable.txt` marker no longer affects configuration selection.** Unsuffixed executables use `$HOME/.agentscommander`; suffixed executables use their adjacent `.agentscommander_<suffix>` directory and refuse to start when it is not writable. `AGENTSCOMMANDER_CONFIG_DIR` still wins. ([#1932](https://github.com/mblua/AgentsCommander/issues/1932))

### Fixed

- **TASK.md lock acquisition retries transient Windows access-denied and sharing-violation errors** under its existing deadline instead of failing. ([#1579](https://github.com/mblua/AgentsCommander/issues/1579))
- **Container shutdown workers are reclaimed after a shutdown overruns its deadline** instead of leaking. ([#1581](https://github.com/mblua/AgentsCommander/issues/1581))
- **CI runs every cargo invocation with `--locked`**, and several static-analysis findings were cleared without behavior changes. ([#2075](https://github.com/mblua/AgentsCommander/issues/2075), [#2088](https://github.com/mblua/AgentsCommander/issues/2088), [#2095](https://github.com/mblua/AgentsCommander/issues/2095), [#2102](https://github.com/mblua/AgentsCommander/issues/2102), [#2107](https://github.com/mblua/AgentsCommander/issues/2107), [#2109](https://github.com/mblua/AgentsCommander/issues/2109))
- A regression test now guards Delete Profile against deleting a held slot. ([#2061](https://github.com/mblua/AgentsCommander/issues/2061))

## Included scope

- fix: Retry transient Windows access-denied on the TASK.md lock open ([#1579](https://github.com/mblua/AgentsCommander/issues/1579), [PR #2094](https://github.com/mblua/AgentsCommander/pull/2094))
- fix: Reclaim container shutdown workers after a deadline overrun ([#1581](https://github.com/mblua/AgentsCommander/issues/1581), [PR #2117](https://github.com/mblua/AgentsCommander/pull/2117))
- maintenance: Remove the portable.txt marker from configuration selection ([#1932](https://github.com/mblua/AgentsCommander/issues/1932), [PR #2089](https://github.com/mblua/AgentsCommander/pull/2089))
- maintenance: Add the Delete Profile guard regression test and dedupe profile fixtures ([#2061](https://github.com/mblua/AgentsCommander/issues/2061), [PR #2081](https://github.com/mblua/AgentsCommander/pull/2081))
- feature: Create a Linux desktop entry on a global npm install ([#2066](https://github.com/mblua/AgentsCommander/issues/2066), [PR #2078](https://github.com/mblua/AgentsCommander/pull/2078))
- security: Run every CI cargo invocation with --locked ([#2075](https://github.com/mblua/AgentsCommander/issues/2075), [PR #2106](https://github.com/mblua/AgentsCommander/pull/2106))
- feature: Add the remote activity sweeper for CI and branch staleness ([#2082](https://github.com/mblua/AgentsCommander/issues/2082), [PR #2100](https://github.com/mblua/AgentsCommander/pull/2100))
- feature: Inject remote-activity notices into the room orchestrator ([#2083](https://github.com/mblua/AgentsCommander/issues/2083), [PR #2114](https://github.com/mblua/AgentsCommander/pull/2114))
- feature: Show the CI ring and branch-staleness bar on the repo chip ([#2084](https://github.com/mblua/AgentsCommander/issues/2084), [PR #2116](https://github.com/mblua/AgentsCommander/pull/2116))
- feature: Support screenshot capture on macOS ([#2086](https://github.com/mblua/AgentsCommander/issues/2086), [PR #2115](https://github.com/mblua/AgentsCommander/pull/2115))
- maintenance: Remove the exponential-backtracking function regex in the test-debt scan ([#2088](https://github.com/mblua/AgentsCommander/issues/2088), [PR #2099](https://github.com/mblua/AgentsCommander/pull/2099))
- maintenance: Move TauriTransport async initialization into a static create() ([#2095](https://github.com/mblua/AgentsCommander/issues/2095), [PR #2097](https://github.com/mblua/AgentsCommander/pull/2097))
- maintenance: Clear Sonar sort-comparator and overridden row-gap findings ([#2102](https://github.com/mblua/AgentsCommander/issues/2102), [PR #2103](https://github.com/mblua/AgentsCommander/pull/2103))
- maintenance: Use an explicit code-unit comparator for five Sonar sort findings ([#2107](https://github.com/mblua/AgentsCommander/issues/2107), [PR #2110](https://github.com/mblua/AgentsCommander/pull/2110))
- maintenance: Clear five Sonar cognitive-complexity findings ([#2109](https://github.com/mblua/AgentsCommander/issues/2109), [PR #2111](https://github.com/mblua/AgentsCommander/pull/2111))

## Install from npm

```text
npx @mblua/agentscommander@0.35.0
```

## Verification identity

- Release issue: https://github.com/mblua/AgentsCommander/issues/2119
- Reviewed evidence set: `bound by the exact release-authority-v1 review-set-sha256 field`
- Candidate assets: 17
- Predecessor: `v0.34.0`

The workflow must publish this body verbatim, make the GitHub Release public and immutable before npm, and attach provenance for the exact assets and package.
