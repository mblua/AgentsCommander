# Agents Commander v0.45.1

### Added

- Native selection screenshots are saved in the active replica's screenshots/ directory, created when needed. Existing captures and explicit window-screenshot --output destinations are unchanged. (#3070, #3074)

### Fixed

- Codex activity detection recognizes complete Working indicators on physical terminal rows when logical row reconstruction misses them. (#3059, #3065)
- An open replica context menu's halo follows its sidebar card when preceding cards change height. (#3069, #3076)

### Changed

- README and quickstart present reusable agent experience, the desktop view and installation options. Settings documentation explains canonical command fields and legacy fallback. (#3044, #3049, #3022, #3055)

### Notes

- Internal preparation adds optional profile-cell identity metadata, a persisted container-options preference and centralized settings constructors. Pair-hash derivation, container UI hiding and exclusive settings ownership are not activated by these changes. (#3050, #3054, #3062, #3064, #2993, #3053)

## Included scope

- maintenance: refactor(settings): normalize state constructors (refs #2993) ([#2993](https://github.com/mblua/AgentsCommander/issues/2993), [PR #3053](https://github.com/mblua/AgentsCommander/pull/3053))
- docs: docs: document canonical command fields (refs #3022) ([#3022](https://github.com/mblua/AgentsCommander/issues/3022), [PR #3055](https://github.com/mblua/AgentsCommander/pull/3055))
- docs: docs: refresh ecosystem README and quickstart (refs #3044) ([#3044](https://github.com/mblua/AgentsCommander/issues/3044), [PR #3049](https://github.com/mblua/AgentsCommander/pull/3049))
- feature: feat(config): add optional profile hash schema (refs #3050) ([#3050](https://github.com/mblua/AgentsCommander/issues/3050), [PR #3054](https://github.com/mblua/AgentsCommander/pull/3054))
- fix: fix(pty): recognize complete physical Codex Working rows (refs #3059) ([#3059](https://github.com/mblua/AgentsCommander/issues/3059), [PR #3065](https://github.com/mblua/AgentsCommander/pull/3065))
- feature: feat(settings): persist container options opt-in (refs #3062) ([#3062](https://github.com/mblua/AgentsCommander/issues/3062), [PR #3064](https://github.com/mblua/AgentsCommander/pull/3064))
- fix: fix(sidebar): keep context-menu halo on its origin refs #3069 ([#3069](https://github.com/mblua/AgentsCommander/issues/3069), [PR #3076](https://github.com/mblua/AgentsCommander/pull/3076))
- feature: feat(screenshot): save captures in replica screenshots folder (refs #3070) ([#3070](https://github.com/mblua/AgentsCommander/issues/3070), [PR #3074](https://github.com/mblua/AgentsCommander/pull/3074))

## Install from npm

```text
npx @mblua/agentscommander@0.45.1
```

## Verification identity

- Release issue: https://github.com/mblua/AgentsCommander/issues/3078
- Reviewed evidence set: `bound by the exact release-authority-v1 review-set-sha256 field`
- Candidate assets: 17
- Predecessor: `v0.45.0`

The workflow must publish this body verbatim, make the GitHub Release public and immutable before npm, and attach provenance for the exact assets and package.
