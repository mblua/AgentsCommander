# Agents Commander v0.42.0

### Added

- New Room has a searchable team picker and an optional task title. A blank title creates a Clean task; an explicit title keeps the USER prefix. (#2788)
- Windows Root agents receive an RTK installation skill. (#2806)

### Changed

- Welcome agent cards group installation controls inside the card and show AC Support levels: Stable, Beta or Experimental. (#2784)
- Agent runtime state moves to Git-ignored config.state.no-git.json on the first write. Decision settings remain in config.json; malformed configuration is rejected rather than silently reset. (#2786, #2807, #2816)

### Fixed

- Co-managed capture survives temporary Jev unavailability and offers retained candidates again when ready. (#2756)
- Claude statusline no longer requires jq. (#2789)
- New Team refuses creation with a pending repository URL. (#2814)
- CI completion notifications corroborate previously observed runs before announcing completion. (#2768)
- Maintenance: increased the held-delivery test budget, added desktop wallpapers, and reorganized RTK and Claude statusline documentation, including the Windows runtime prerequisite. (#2767, #2781, #2790, #2803)

## Included scope

- fix: Retain co-managed capture through Jev unavailability ([#2756](https://github.com/mblua/AgentsCommander/issues/2756), [PR #2783](https://github.com/mblua/AgentsCommander/pull/2783))
- maintenance: Increase held-delivery test budget ([#2767](https://github.com/mblua/AgentsCommander/issues/2767), [PR #2778](https://github.com/mblua/AgentsCommander/pull/2778))
- fix: Corroborate CI completion notifications ([#2768](https://github.com/mblua/AgentsCommander/issues/2768), [PR #2822](https://github.com/mblua/AgentsCommander/pull/2822))
- docs: Add desktop wallpapers ([#2781](https://github.com/mblua/AgentsCommander/issues/2781), [PR #2782](https://github.com/mblua/AgentsCommander/pull/2782))
- feature: Improve Welcome agent cards and AC Support levels ([#2784](https://github.com/mblua/AgentsCommander/issues/2784), [PR #2785](https://github.com/mblua/AgentsCommander/pull/2785))
- maintenance: Introduce config pair and shared state loader ([#2786](https://github.com/mblua/AgentsCommander/issues/2786), [PR #2805](https://github.com/mblua/AgentsCommander/pull/2805))
- feature: Search New Room teams and allow optional task titles ([#2788](https://github.com/mblua/AgentsCommander/issues/2788), [PR #2821](https://github.com/mblua/AgentsCommander/pull/2821))
- fix: Remove jq dependency from Claude statusline ([#2789](https://github.com/mblua/AgentsCommander/issues/2789), [PR #2791](https://github.com/mblua/AgentsCommander/pull/2791))
- docs: Reorganize RTK docs and document Windows runtime ([#2790](https://github.com/mblua/AgentsCommander/issues/2790), [PR #2792](https://github.com/mblua/AgentsCommander/pull/2792))
- docs: Relocate Claude statusline documentation ([#2803](https://github.com/mblua/AgentsCommander/issues/2803), [PR #2804](https://github.com/mblua/AgentsCommander/pull/2804))
- feature: Seed Windows Root RTK installation skill ([#2806](https://github.com/mblua/AgentsCommander/issues/2806), [PR #2812](https://github.com/mblua/AgentsCommander/pull/2812))
- maintenance: Migrate agent state into ignored config state file ([#2807](https://github.com/mblua/AgentsCommander/issues/2807), [PR #2810](https://github.com/mblua/AgentsCommander/pull/2810))
- fix: Reject New Team creation with pending repository URL ([#2814](https://github.com/mblua/AgentsCommander/issues/2814), [PR #2815](https://github.com/mblua/AgentsCommander/pull/2815))
- maintenance: Route agent config writers through config pair ([#2816](https://github.com/mblua/AgentsCommander/issues/2816), [PR #2820](https://github.com/mblua/AgentsCommander/pull/2820))

## Install from npm

```text
npx @mblua/agentscommander@0.42.0
```

## Verification identity

- Release issue: https://github.com/mblua/AgentsCommander/issues/2825
- Reviewed evidence set: `bound by the exact release-authority-v1 review-set-sha256 field`
- Candidate assets: 17
- Predecessor: `v0.41.0`

The workflow must publish this body verbatim, make the GitHub Release public and immutable before npm, and attach provenance for the exact assets and package.
