# Agents Commander v0.43.0

### Added

- Project coding-agent catalogs support a project policy layer between instance settings and personal overrides. (#2833, #2834)
- Task storage adds status history and paired Clean backups; CLI and UI integration remain for later phases. (#2839)
- Coding-agent settings can store preflight commands and timeouts; command execution is not activated yet. (#2859)

### Changed

- Root Agent instructions are owned by the editable context file. Exact historical defaults migrate with a backup; customized and empty contexts remain authoritative. (#2832)
- Coding-agent installer actions are disabled by default and require an explicit opt-in. The catalog supplies platform-specific commands, with updated installation guidance. (#2800, #2801)
- Configuration regression checks cover state-key readers and prevent tracked configuration from receiving runtime state keys. (#2823)

### Fixed

- Updated Tauri and tao to remove a Windows keyboard reentrancy deadlock. The reported freeze was not reproduced on the old version, so its cause remains unconfirmed. (#2817)
- Enter on the focused Cancel button in New Room cancels rather than creating a room. The physical Windows keyboard check remains pending. (#2809)
- Terminal timestamps and dimensions appear in the upper-right corner. (#2829)

## Included scope

- fix: fix: default installer actions off behind feature flag (#2800) ([#2800](https://github.com/mblua/AgentsCommander/issues/2800), [PR #2856](https://github.com/mblua/AgentsCommander/pull/2856))
- docs: docs: document platform installers and acceptance (refs #2801) ([#2801](https://github.com/mblua/AgentsCommander/issues/2801), [PR #2866](https://github.com/mblua/AgentsCommander/pull/2866))
- fix: fix(sidebar): Enter on a focused button no longer creates the room in New Room modal (refs #2809) ([#2809](https://github.com/mblua/AgentsCommander/issues/2809), [PR #2828](https://github.com/mblua/AgentsCommander/pull/2828))
- fix: fix: raise tao to 0.37.1 to remove the keyboard reentrancy deadlock (#2817) ([#2817](https://github.com/mblua/AgentsCommander/issues/2817), [PR #2819](https://github.com/mblua/AgentsCommander/pull/2819))
- maintenance: refactor(config): C4 single-reader gate and no-write-back proofs (#2823) ([#2823](https://github.com/mblua/AgentsCommander/issues/2823), [PR #2824](https://github.com/mblua/AgentsCommander/pull/2824))
- fix: fix: move terminal status strip to top-right (#2829) ([#2829](https://github.com/mblua/AgentsCommander/issues/2829), [PR #2830](https://github.com/mblua/AgentsCommander/pull/2830))
- feature: fix(config): make Root context file-owned (refs #2832) ([#2832](https://github.com/mblua/AgentsCommander/issues/2832), [PR #2845](https://github.com/mblua/AgentsCommander/pull/2845))
- feature: feat: read project coding-agent catalog policy (refs #2833) ([#2833](https://github.com/mblua/AgentsCommander/issues/2833), [PR #2838](https://github.com/mblua/AgentsCommander/pull/2838))
- docs: docs: document project coding-agent catalog layer (refs #2834) ([#2834](https://github.com/mblua/AgentsCommander/issues/2834), [PR #2847](https://github.com/mblua/AgentsCommander/pull/2847))
- feature: feat: persist task status history and paired Clean backups (#2839) ([#2839](https://github.com/mblua/AgentsCommander/issues/2839), [PR #2846](https://github.com/mblua/AgentsCommander/pull/2846))
- feature: feat: persist preflight settings (refs #2859) ([#2859](https://github.com/mblua/AgentsCommander/issues/2859), [PR #2865](https://github.com/mblua/AgentsCommander/pull/2865))

## Install from npm

```text
npx @mblua/agentscommander@0.43.0
```

## Verification identity

- Release issue: https://github.com/mblua/AgentsCommander/issues/2874
- Reviewed evidence set: `bound by the exact release-authority-v1 review-set-sha256 field`
- Candidate assets: 17
- Predecessor: `v0.42.0`

The workflow must publish this body verbatim, make the GitHub Release public and immutable before npm, and attach provenance for the exact assets and package.
