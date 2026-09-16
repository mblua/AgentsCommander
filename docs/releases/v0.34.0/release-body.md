# Agents Commander v0.34.0

### Added

- **`settings.json` saves keep a bounded backup history.** Every successful save archives the bytes it replaced into five rotating slots, `settings.backup.1.json` through `settings.backup.5.json`, beside `settings.json` in the same configuration directory. Slot 1 holds the version the most recent save replaced; slot 5 is the oldest kept. ([#2058](https://github.com/mblua/AgentsCommander/issues/2058))
- **A global npm install on Windows creates a per-user Start Menu shortcut.** `AgentsCommander.lnk` targets the installed executable and uses its icon. It is skipped for local installs and when `AGENTSCOMMANDER_NO_SHORTCUT=1` is set, never fails the install, and must be removed by hand because npm runs no uninstall scripts. ([#2053](https://github.com/mblua/AgentsCommander/issues/2053))
- **A global npm install on macOS makes the app reachable from Launchpad and Spotlight** through a `~/Applications/AgentsCommander.app` alias to the extracted bundle. It needs no elevation, is skipped for local installs and with `AGENTSCOMMANDER_NO_SHORTCUT=1`, leaves an unrelated application of the same name untouched, and never fails the install. ([#2065](https://github.com/mblua/AgentsCommander/issues/2065))
- **The project global context template seeds an Answering section.** The previous template is recognized and replaced with a backup on scan, so generated standalone templates stay retirable. ([#2031](https://github.com/mblua/AgentsCommander/issues/2031))

### Changed

- **Left click on an offline Agent Matrix row no longer opens the session launch flow.** It shows a notice explaining that replicas, not the matrix, are what gets launched, and that the matrix folder is available from the row's right-click menu. ([#2046](https://github.com/mblua/AgentsCommander/issues/2046))
- **The sidebar lock chip shows only the lock icon**, without the KEEP label. ([#2030](https://github.com/mblua/AgentsCommander/issues/2030))
- **The Coding Agent profile modal scrolls its Profile and Same Profile columns independently.** ([#2038](https://github.com/mblua/AgentsCommander/issues/2038))
- **Documentation updates**: the configuration-directory guides, the macOS and Linux support tiers, the deprecation of the legacy `codingAgentProfiles` settings, the OCA-008 tombstones for enabled Grok and disabled Muse, and Team wording in the use-case recipes. ([#1936](https://github.com/mblua/AgentsCommander/issues/1936), [#2017](https://github.com/mblua/AgentsCommander/issues/2017), [#2019](https://github.com/mblua/AgentsCommander/issues/2019), [#2009](https://github.com/mblua/AgentsCommander/issues/2009), [#2032](https://github.com/mblua/AgentsCommander/issues/2032))

### Fixed

- **A distinct submission is no longer silently deduplicated** when a critical-admission key outlives the completion the caller observes. ([#1580](https://github.com/mblua/AgentsCommander/issues/1580))
- **Assign-and-lock on a replica no longer fails with `stalePreview`.** ([#2051](https://github.com/mblua/AgentsCommander/issues/2051))
- **Delete Profile no longer removes configured profiles from other coding agents.** ([#2057](https://github.com/mblua/AgentsCommander/issues/2057))
- **The sidebar Ungrouped counter agrees with the Ungrouped panel list.** ([#2036](https://github.com/mblua/AgentsCommander/issues/2036))
- **npm publication no longer fails right after a successful publish** when the registry's `latest` dist-tag lags. ([#2042](https://github.com/mblua/AgentsCommander/issues/2042))
- **Reseed tests are deterministic under concurrent same-binary runs.** ([#2001](https://github.com/mblua/AgentsCommander/issues/2001))
- **CI uploads artifacts with a Node 24 release of `actions/upload-artifact`.** ([#2040](https://github.com/mblua/AgentsCommander/issues/2040))

## Included scope

- fix: Stop silently deduplicating a distinct submission when a critical-admission key outlives the completion the caller observes ([#1580](https://github.com/mblua/AgentsCommander/issues/1580), [PR #2023](https://github.com/mblua/AgentsCommander/pull/2023))
- docs: Update the configuration directory guides ([#1936](https://github.com/mblua/AgentsCommander/issues/1936), [PR #2063](https://github.com/mblua/AgentsCommander/pull/2063))
- fix: Isolate reseed tests under concurrent same-binary lib runs ([#2001](https://github.com/mblua/AgentsCommander/issues/2001), [PR #2055](https://github.com/mblua/AgentsCommander/pull/2055))
- docs: Align the OCA-008 tombstones with enabled Grok and disabled Muse ([#2009](https://github.com/mblua/AgentsCommander/issues/2009), [PR #2067](https://github.com/mblua/AgentsCommander/pull/2067))
- docs: State the macOS and Linux support tiers across the support statements ([#2017](https://github.com/mblua/AgentsCommander/issues/2017), [PR #2060](https://github.com/mblua/AgentsCommander/pull/2060))
- docs: Document the legacy codingAgentProfiles deprecation ([#2019](https://github.com/mblua/AgentsCommander/issues/2019), [PR #2049](https://github.com/mblua/AgentsCommander/pull/2049))
- feature: Show only the lock icon on the sidebar lock chip and drop the KEEP label ([#2030](https://github.com/mblua/AgentsCommander/issues/2030), [PR #2052](https://github.com/mblua/AgentsCommander/pull/2052))
- feature: Seed an Answering section into the project global context template ([#2031](https://github.com/mblua/AgentsCommander/issues/2031), [PR #2048](https://github.com/mblua/AgentsCommander/pull/2048))
- docs: Use Team instead of crew in the use-cases recipe ([#2032](https://github.com/mblua/AgentsCommander/issues/2032), [PR #2035](https://github.com/mblua/AgentsCommander/pull/2035))
- fix: Make the sidebar Ungrouped rail counter agree with the Ungrouped panel list ([#2036](https://github.com/mblua/AgentsCommander/issues/2036), [PR #2047](https://github.com/mblua/AgentsCommander/pull/2047))
- fix: Scroll the Profile and Same Profile columns of the Coding Agent modal independently ([#2038](https://github.com/mblua/AgentsCommander/issues/2038), [PR #2045](https://github.com/mblua/AgentsCommander/pull/2045))
- maintenance: Upgrade actions/upload-artifact to a Node 24 release ([#2040](https://github.com/mblua/AgentsCommander/issues/2040), [PR #2062](https://github.com/mblua/AgentsCommander/pull/2062))
- fix: Tolerate a lagging latest dist-tag in publish-npm right after a successful publish ([#2042](https://github.com/mblua/AgentsCommander/issues/2042), [PR #2043](https://github.com/mblua/AgentsCommander/pull/2043))
- feature: Explain replicas on an Agent Matrix left click instead of launching a session ([#2046](https://github.com/mblua/AgentsCommander/issues/2046), [PR #2050](https://github.com/mblua/AgentsCommander/pull/2050))
- fix: Stop replica assign-and-lock from always failing with stalePreview ([#2051](https://github.com/mblua/AgentsCommander/issues/2051), [PR #2056](https://github.com/mblua/AgentsCommander/pull/2056))
- feature: Create a per-user Start Menu shortcut on a global npm install on Windows ([#2053](https://github.com/mblua/AgentsCommander/issues/2053), [PR #2054](https://github.com/mblua/AgentsCommander/pull/2054))
- fix: Stop Delete Profile from removing configured profiles from every coding agent ([#2057](https://github.com/mblua/AgentsCommander/issues/2057), [PR #2059](https://github.com/mblua/AgentsCommander/pull/2059))
- feature: Rotate bounded settings.json backups on every save ([#2058](https://github.com/mblua/AgentsCommander/issues/2058), [PR #2070](https://github.com/mblua/AgentsCommander/pull/2070))
- feature: Make the app reachable from Launchpad and Spotlight after a global npm install on macOS ([#2065](https://github.com/mblua/AgentsCommander/issues/2065), [PR #2068](https://github.com/mblua/AgentsCommander/pull/2068))

## Install from npm

```text
npx @mblua/agentscommander@0.34.0
```

## Verification identity

- Release issue: https://github.com/mblua/AgentsCommander/issues/2071
- Reviewed evidence set: `bound by the exact release-authority-v1 review-set-sha256 field`
- Candidate assets: 17
- Predecessor: `v0.33.0`

The workflow must publish this body verbatim, make the GitHub Release public and immutable before npm, and attach provenance for the exact assets and package.
