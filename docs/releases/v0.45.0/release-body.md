# Agents Commander v0.45.0

### Added

- The error popup offers a checkbox to mute future popups of the same error type until the application closes. Logging, Copy and Dismiss remain available; restarting clears the mute. (#3036, #3037, #3039, #3040)
- Opening a replica context menu highlights the originating sidebar card with a short accent pulse and a steady outline until the menu closes. (#3027, #3038)
- The coordinator CLI can replace or clear a task description with `task-set-body`, preserving the title and task status. (#2931, #2941)
- Settings, General, Terminal shows response-close preferences (enabled, idle seconds 1-3600). Runtime behavior for them is not active yet. (#2850, #2851, #2964, #2965)
- DEBUG logging attributes settings-lock acquisition, timeout and release to their owner and operation. INFO logging is unchanged. (#2985, #2988)
- Coding-agent entries carry a derived `id_hash` execution identity next to their unchanged stable `id`, and configured sessions persist a stable binding identity across restore and restart. (#2973, #2974, #2977, #2981, #2983, #2984, #2987, #2989)

### Changed

- Configuration files now write `baseCommand` and `additionalArguments`; the Settings fields are labeled "Base command" and "Additional arguments". The legacy `command` key is still read. (#3019, #3020, #3021, #3023, #3041, #3043)
- Codex stays Busy while its full "Working (... esc to interrupt)" indicator is visible, even with a frozen counter. (#2879, #3028)
- `project-paths.json` is the only source of project paths; existing settings migrate with a backup. CLI consumers read project state without saving settings. (#2951, #2952, #2953, #2954, #2955, #2959, #2966, #2967, #2969, #2971)
- Generated context tells agents to report delegated work to its assigner, teaches the `list-peers-lean --peer` filter, and on Windows prefers Git Bash directly. Unmodified seeded contexts update; customized ones are preserved. (#2871, #2943, #2990, #2991, #3024, #3026)
- Hovering a task title in the sidebar no longer shows the status overlay; status remains available above the terminal. (#2948, #2949)
- portable-pty is upgraded to 0.9.0; hidden Windows sessions answer the initial cursor query instead of waiting for a terminal to attach. (#3029, #3035)
- README describes current solo and team setups and links the official Discord and YouTube channels. (#3030, #3031)

### Fixed

- Saving agent settings preserves the backend-owned Codex HOME keys. (#2972, #2982)
- PAIR writes revalidate their target before effects and use exclusive staging files. (#2960, #2961)

### Notes

- Internal groundwork for recoverable configuration persistence, selection references and response-close admission landed dormant and does not change current behavior. (#2852, #2853, #2893, #2894, #2895, #2896, #2897, #2947, #2950, #2957, #2958, #2968, #2970, #2986)
- Frontend coverage is measured and published to Codecov. (#2962, #3025, #3032, #3033, #3042)

## Included scope

- feature: Persist response-close settings ([#2850](https://github.com/mblua/AgentsCommander/issues/2850), [PR #2964](https://github.com/mblua/AgentsCommander/pull/2964))
- feature: Response-close preferences in Settings ([#2851](https://github.com/mblua/AgentsCommander/issues/2851), [PR #2965](https://github.com/mblua/AgentsCommander/pull/2965))
- maintenance: Dormant response close guard ([#2852](https://github.com/mblua/AgentsCommander/issues/2852), [PR #2968](https://github.com/mblua/AgentsCommander/pull/2968))
- maintenance: Dormant response-close admission ([#2853](https://github.com/mblua/AgentsCommander/issues/2853), [PR #2986](https://github.com/mblua/AgentsCommander/pull/2986))
- fix: Windows context prefers Git Bash directly ([#2871](https://github.com/mblua/AgentsCommander/issues/2871), [PR #2943](https://github.com/mblua/AgentsCommander/pull/2943))
- fix: Keep Codex busy while Working is visible ([#2879](https://github.com/mblua/AgentsCommander/issues/2879), [PR #3028](https://github.com/mblua/AgentsCommander/pull/3028))
- maintenance: Independent source persistence ([#2893](https://github.com/mblua/AgentsCommander/issues/2893), [PR #2947](https://github.com/mblua/AgentsCommander/pull/2947))
- maintenance: Prepared physical persistence ([#2894](https://github.com/mblua/AgentsCommander/issues/2894), [PR #2950](https://github.com/mblua/AgentsCommander/pull/2950))
- maintenance: Shared source coordination ([#2895](https://github.com/mblua/AgentsCommander/issues/2895), [PR #2957](https://github.com/mblua/AgentsCommander/pull/2957))
- maintenance: Pair activity reservations ([#2896](https://github.com/mblua/AgentsCommander/issues/2896), [PR #2958](https://github.com/mblua/AgentsCommander/pull/2958))
- maintenance: Selection reference persistence ([#2897](https://github.com/mblua/AgentsCommander/issues/2897), [PR #2970](https://github.com/mblua/AgentsCommander/pull/2970))
- feature: Replace or clear task body from CLI ([#2931](https://github.com/mblua/AgentsCommander/issues/2931), [PR #2941](https://github.com/mblua/AgentsCommander/pull/2941))
- fix: Task status only above terminal ([#2948](https://github.com/mblua/AgentsCommander/issues/2948), [PR #2949](https://github.com/mblua/AgentsCommander/pull/2949))
- fix: Guard project path transactions ([#2951](https://github.com/mblua/AgentsCommander/issues/2951), [PR #2959](https://github.com/mblua/AgentsCommander/pull/2959))
- maintenance: Prepare raw project-path migration ([#2952](https://github.com/mblua/AgentsCommander/issues/2952), [PR #2966](https://github.com/mblua/AgentsCommander/pull/2966))
- fix: Read-only CLI project-path consumers ([#2953](https://github.com/mblua/AgentsCommander/issues/2953), [PR #2967](https://github.com/mblua/AgentsCommander/pull/2967))
- feature: Activate project-paths.json as sole authority ([#2954](https://github.com/mblua/AgentsCommander/issues/2954), [PR #2969](https://github.com/mblua/AgentsCommander/pull/2969))
- docs: Project paths documentation ([#2955](https://github.com/mblua/AgentsCommander/issues/2955), [PR #2971](https://github.com/mblua/AgentsCommander/pull/2971))
- fix: Guard PAIR targets and exclusive staging ([#2960](https://github.com/mblua/AgentsCommander/issues/2960), [PR #2961](https://github.com/mblua/AgentsCommander/pull/2961))
- maintenance: Frontend coverage measurement ([#2962](https://github.com/mblua/AgentsCommander/issues/2962), [PR #3025](https://github.com/mblua/AgentsCommander/pull/3025))
- fix: Preserve backend-owned Codex HOME keys ([#2972](https://github.com/mblua/AgentsCommander/issues/2972), [PR #2982](https://github.com/mblua/AgentsCommander/pull/2982))
- feature: Persist stable session bindings ([#2973](https://github.com/mblua/AgentsCommander/issues/2973), [PR #2983](https://github.com/mblua/AgentsCommander/pull/2983))
- feature: Compatible id_hash metadata ([#2974](https://github.com/mblua/AgentsCommander/issues/2974), [PR #2984](https://github.com/mblua/AgentsCommander/pull/2984))
- feature: Optional agent id_hash in TypeScript ([#2977](https://github.com/mblua/AgentsCommander/issues/2977), [PR #2989](https://github.com/mblua/AgentsCommander/pull/2989))
- feature: Derive and persist id_hash ([#2981](https://github.com/mblua/AgentsCommander/issues/2981), [PR #2987](https://github.com/mblua/AgentsCommander/pull/2987))
- feature: DEBUG settings lock attribution ([#2985](https://github.com/mblua/AgentsCommander/issues/2985), [PR #2988](https://github.com/mblua/AgentsCommander/pull/2988))
- fix: Teach native peer filtering in context ([#2990](https://github.com/mblua/AgentsCommander/issues/2990), [PR #2991](https://github.com/mblua/AgentsCommander/pull/2991))
- maintenance: Tolerate canonical command fields on read ([#3019](https://github.com/mblua/AgentsCommander/issues/3019), [PR #3023](https://github.com/mblua/AgentsCommander/pull/3023))
- maintenance: Write canonical command fields in configuration ([#3020](https://github.com/mblua/AgentsCommander/issues/3020), [PR #3041](https://github.com/mblua/AgentsCommander/pull/3041))
- feature: Write canonical command fields from frontend ([#3021](https://github.com/mblua/AgentsCommander/issues/3021), [PR #3043](https://github.com/mblua/AgentsCommander/pull/3043))
- fix: Report delegated work to its assigner ([#3024](https://github.com/mblua/AgentsCommander/issues/3024), [PR #3026](https://github.com/mblua/AgentsCommander/pull/3026))
- feature: Highlight replica context menu origin ([#3027](https://github.com/mblua/AgentsCommander/issues/3027), [PR #3038](https://github.com/mblua/AgentsCommander/pull/3038))
- maintenance: Upgrade portable-pty to 0.9.0 ([#3029](https://github.com/mblua/AgentsCommander/issues/3029), [PR #3035](https://github.com/mblua/AgentsCommander/pull/3035))
- docs: README ecosystem positioning ([#3030](https://github.com/mblua/AgentsCommander/issues/3030), [PR #3031](https://github.com/mblua/AgentsCommander/pull/3031))
- maintenance: Publish frontend coverage to Codecov ([#3032](https://github.com/mblua/AgentsCommander/issues/3032), [PR #3033](https://github.com/mblua/AgentsCommander/pull/3033))
- feature: Process-scoped error popup mutes ([#3036](https://github.com/mblua/AgentsCommander/issues/3036), [PR #3039](https://github.com/mblua/AgentsCommander/pull/3039))
- feature: Temporary error popup mute checkbox ([#3037](https://github.com/mblua/AgentsCommander/issues/3037), [PR #3040](https://github.com/mblua/AgentsCommander/pull/3040))

## Install from npm

```text
npx @mblua/agentscommander@0.45.0
```

## Verification identity

- Release issue: https://github.com/mblua/AgentsCommander/issues/3045
- Reviewed evidence set: `bound by the exact release-authority-v1 review-set-sha256 field`
- Candidate assets: 17
- Predecessor: `v0.44.0`

The workflow must publish this body verbatim, make the GitHub Release public and immutable before npm, and attach provenance for the exact assets and package.
