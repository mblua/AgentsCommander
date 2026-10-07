# Agents Commander v0.44.0

### Added

- Room tasks expose complete snapshots and revision-checked status updates through `task-get` and `task-status-set`, including safe idempotent retries. (#2840, #2841)
- The terminal shows task descriptions and an accessible status tooltip; the sidebar also shows the complete current status. Clean backs up the task and its history together. (#2842, #2843, #2844)
- Session context lists project skills and team skills. Room replicas receive their room team's skills; origin agents receive skills from their same-project teams. Skill bodies remain loaded on demand. (#2872, #2873)
- Antigravity settings offer opt-in suggestions for context and weekly quota patterns, preserving custom patterns until the suggestion is selected. (#2836)

### Changed

- New memory rotations live under `memory-archive/`. Existing root-level archives stay untouched and readable. (#2888, #2889)
- Clean and newly created rooms have an empty task description; existing human and legacy descriptions remain until an explicit Clean. (#2935, #2936, #2937)

### Fixed

- CI alerts wait while typing hold is active; subsequent alerts wait for the preceding delivery to finish. (#2770)
- Windows atomic replacement supports long paths during terminal artifact repair and other path-identity operations. (#2923, #2926)
- Regression fixtures cover task snapshots and Windows reparse rejection without requiring link creation in that specific fixture. The simulated reparse fixture does not verify real symlink or junction integration. (#2932, #2924)

### Notes

- Task UI changes have automated coverage; physical keyboard, wheel, contrast and small-window tooltip checks remain incomplete. (#2842, #2843)

## Included scope

- fix: Defer CI alerts during typing hold ([#2770](https://github.com/mblua/AgentsCommander/issues/2770), [PR #2925](https://github.com/mblua/AgentsCommander/pull/2925))
- feature: Opt-in Antigravity context and quota suggestions ([#2836](https://github.com/mblua/AgentsCommander/issues/2836), [PR #2848](https://github.com/mblua/AgentsCommander/pull/2848))
- feature: Task snapshot and revision-checked status CLI ([#2840](https://github.com/mblua/AgentsCommander/issues/2840), [PR #2857](https://github.com/mblua/AgentsCommander/pull/2857))
- feature: Task snapshots over IPC and reliable refresh events ([#2841](https://github.com/mblua/AgentsCommander/issues/2841), [PR #2882](https://github.com/mblua/AgentsCommander/pull/2882))
- feature: Terminal task description and status tooltip ([#2842](https://github.com/mblua/AgentsCommander/issues/2842), [PR #2886](https://github.com/mblua/AgentsCommander/pull/2886))
- feature: Complete task status in sidebar ([#2843](https://github.com/mblua/AgentsCommander/issues/2843), [PR #2913](https://github.com/mblua/AgentsCommander/pull/2913))
- docs: Task status CLI and paired Clean documentation ([#2844](https://github.com/mblua/AgentsCommander/issues/2844), [PR #2929](https://github.com/mblua/AgentsCommander/pull/2929))
- feature: Project and team skills in session context ([#2872](https://github.com/mblua/AgentsCommander/issues/2872), [PR #2884](https://github.com/mblua/AgentsCommander/pull/2884))
- docs: Project and team skill discovery documentation ([#2873](https://github.com/mblua/AgentsCommander/issues/2873), [PR #2887](https://github.com/mblua/AgentsCommander/pull/2887))
- maintenance: Nest new memory rotations under memory-archive ([#2888](https://github.com/mblua/AgentsCommander/issues/2888), [PR #2890](https://github.com/mblua/AgentsCommander/pull/2890))
- docs: Nested memory archive documentation ([#2889](https://github.com/mblua/AgentsCommander/issues/2889), [PR #2891](https://github.com/mblua/AgentsCommander/pull/2891))
- fix: Windows long-path artifact replacement ([#2923](https://github.com/mblua/AgentsCommander/issues/2923), [PR #2927](https://github.com/mblua/AgentsCommander/pull/2927))
- fix: Simulated reparse rejection fixtures on Windows ([#2924](https://github.com/mblua/AgentsCommander/issues/2924), [PR #2928](https://github.com/mblua/AgentsCommander/pull/2928))
- fix: Windows long-path atomic replacement ([#2926](https://github.com/mblua/AgentsCommander/issues/2926), [PR #2934](https://github.com/mblua/AgentsCommander/pull/2934))
- fix: Task snapshots in sidebar regression fixtures ([#2932](https://github.com/mblua/AgentsCommander/issues/2932), [PR #2933](https://github.com/mblua/AgentsCommander/pull/2933))
- fix: Empty task descriptions in Clean menus ([#2935](https://github.com/mblua/AgentsCommander/issues/2935), [PR #2938](https://github.com/mblua/AgentsCommander/pull/2938))
- fix: Empty task bodies after Clean and creation ([#2936](https://github.com/mblua/AgentsCommander/issues/2936), [PR #2939](https://github.com/mblua/AgentsCommander/pull/2939))
- docs: Empty task defaults documentation ([#2937](https://github.com/mblua/AgentsCommander/issues/2937), [PR #2940](https://github.com/mblua/AgentsCommander/pull/2940))

## Install from npm

```text
npx @mblua/agentscommander@0.44.0
```

## Verification identity

- Release issue: https://github.com/mblua/AgentsCommander/issues/2944
- Reviewed evidence set: `bound by the exact release-authority-v1 review-set-sha256 field`
- Candidate assets: 17
- Predecessor: `v0.43.0`

The workflow must publish this body verbatim, make the GitHub Release public and immutable before npm, and attach provenance for the exact assets and package.
