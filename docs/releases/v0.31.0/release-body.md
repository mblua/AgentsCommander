# Agents Commander v0.31.0

### Added

- **Agents resume automatically after an app restart.** Settings > General gains an **On app restart** section with two checkboxes and two editable restart prompts. When enabled, AgentsCommander restores the saved sessions, wakes the replicas that were working when the app closed, waits until each one is ready for input, and types the configured prompt into it once, so a team picks up where it left off without anyone retyping into every terminal. The existing `Orchestrator wake state` control moves into that section. ([#1793](https://github.com/mblua/AgentsCommander/issues/1793), [#1801](https://github.com/mblua/AgentsCommander/issues/1801), [#1802](https://github.com/mblua/AgentsCommander/issues/1802), [#1803](https://github.com/mblua/AgentsCommander/issues/1803), [#1804](https://github.com/mblua/AgentsCommander/issues/1804))
- **The bar that marks the selected sidebar row now has user-settable width and colour.** Two new fields in Settings set the rail width (up to 14px) and its colour; both are persisted, published to the DOM and validated in the modal. The factory colour is now `#630707`. ([#1796](https://github.com/mblua/AgentsCommander/issues/1796), [#1828](https://github.com/mblua/AgentsCommander/issues/1828), [#1829](https://github.com/mblua/AgentsCommander/issues/1829), [#1830](https://github.com/mblua/AgentsCommander/issues/1830), [#1831](https://github.com/mblua/AgentsCommander/issues/1831), [#1844](https://github.com/mblua/AgentsCommander/issues/1844))
- **Muse Code joins the embedded coding-agent catalog** as a beta preset: command `muse`, macOS and Linux hosts only, no config seed and no update commands. ([#1860](https://github.com/mblua/AgentsCommander/issues/1860))
- **`agentscommander --version` (and `-V`) prints the CLI version.** The npm clean-install check in the release pipeline relies on it, and that pipeline's post-publish verification now tolerates registry propagation delays and compares the provenance registry URL canonically. ([#1826](https://github.com/mblua/AgentsCommander/issues/1826))
- **The Golden Rule block seeded to every room replica now lists six shared filesystem locations:** the project-level `plans`, `tools`, `errors` and `project-shared` directories under the project `.ac` root (read and write), plus the room's `TASK.md` (read-only) and `room-shared/` (read and write). AgentsCommander creates the directories; an already-materialized context picks them up with no migration. ([#1795](https://github.com/mblua/AgentsCommander/issues/1795))
- **New documentation page for running Claude Code with the third-party `codebase-memory-mcp` server**, directly or through an AgentsCommander profile cell. The earlier "why not MCP" stance is removed from the README and the FAQ. ([#1882](https://github.com/mblua/AgentsCommander/issues/1882))

### Changed

- **The Root Agent's right-click menu reaches parity with the replica rows.** Every session-row context menu is now rendered by one catalogue component, so the same items, separators and dismissal behaviour apply everywhere. ([#1871](https://github.com/mblua/AgentsCommander/issues/1871))

### Fixed

- **Blocked-menu notices no longer get lost.** Each session's communication state is reconciled from the 5-second listing poll, so a raised hand and a blocked menu no longer overwrite each other and a dropped event, a reconnect or a window reload heals within seconds ([#1856](https://github.com/mblua/AgentsCommander/issues/1856)); blocked menus surface as one pinned, aggregated toast carrying the latest notice text and requesting taskbar attention ([#1857](https://github.com/mblua/AgentsCommander/issues/1857)); the blocked-menu chip has its own glyph and colour, distinct from a raised hand, and the toast action button is styled ([#1858](https://github.com/mblua/AgentsCommander/issues/1858)); and collapsed projects, teams and orchestrator groups, as well as the sidebar filter, roll the blocked state up so it stays visible ([#1859](https://github.com/mblua/AgentsCommander/issues/1859)).
- **`test-reset --confirm-testeable` now names the process holding the single-instance mutex** (PID, image name, path and handle) instead of only reporting `testable_gui_active`, and the panic paths that leaked a testable process are closed. Windows only. ([#1773](https://github.com/mblua/AgentsCommander/issues/1773))
- **`npm test` can no longer exit non-zero while every test passes.** A debounced settings-preview call fired against an incomplete mock after the test finished. ([#1797](https://github.com/mblua/AgentsCommander/issues/1797))

## Included scope

- fix: fix(api): close the available_loopback_port() TOCTOU race (refs #1768) ([#1768](https://github.com/mblua/AgentsCommander/issues/1768), [PR #1870](https://github.com/mblua/AgentsCommander/pull/1870))
- fix: fix(testability): name the testable_gui_active mutex holder and close panic-path leaks (refs #1773) ([#1773](https://github.com/mblua/AgentsCommander/issues/1773), [PR #1890](https://github.com/mblua/AgentsCommander/pull/1890))
- feature: feat(#1795): seed six shared filesystem locations in the Golden Rule block ([#1795](https://github.com/mblua/AgentsCommander/issues/1795), [PR #1845](https://github.com/mblua/AgentsCommander/pull/1845))
- fix: fix(test): mock PtyAPI so npm test cannot exit non-zero with every test passing (refs #1797) ([#1797](https://github.com/mblua/AgentsCommander/issues/1797), [PR #1846](https://github.com/mblua/AgentsCommander/pull/1846))
- feature: feat(#1801): add the restart-resume settings contract and the persisted-working predicate ([#1801](https://github.com/mblua/AgentsCommander/issues/1801), [PR #1848](https://github.com/mblua/AgentsCommander/pull/1848))
- feature: feat(#1802): phase 1b - wake policy, target collection and the restart auto-resume pass ([#1802](https://github.com/mblua/AgentsCommander/issues/1802), [PR #1872](https://github.com/mblua/AgentsCommander/pull/1872))
- feature: feat(settings): add the On app restart section to Settings > General (refs #1803) ([#1803](https://github.com/mblua/AgentsCommander/issues/1803), [PR #1878](https://github.com/mblua/AgentsCommander/pull/1878))
- docs: docs(settings): document the On app restart settings (refs #1804) ([#1804](https://github.com/mblua/AgentsCommander/issues/1804), [PR #1880](https://github.com/mblua/AgentsCommander/pull/1880))
- maintenance: test(list-peers): pin D3 against a status-predicate selector, and fix an inverted fixture comment (refs #1822) ([#1822](https://github.com/mblua/AgentsCommander/issues/1822), [PR #1849](https://github.com/mblua/AgentsCommander/pull/1849))
- maintenance: ci(1826): harden publish-npm verification; add --version to the CLI root ([#1826](https://github.com/mblua/AgentsCommander/issues/1826), [PR #1827](https://github.com/mblua/AgentsCommander/pull/1827))
- feature: feat(sidebar): persist and tokenise the selected-row rail (refs #1796) ([#1828](https://github.com/mblua/AgentsCommander/issues/1828), [PR #1836](https://github.com/mblua/AgentsCommander/pull/1836))
- feature: feat(#1829): mirror the two rail fields in the TypeScript AppSettings contract ([#1829](https://github.com/mblua/AgentsCommander/issues/1829), [PR #1837](https://github.com/mblua/AgentsCommander/pull/1837))
- feature: feat(#1830): publish the selected-row rail settings to the DOM and edit them in the modal ([#1830](https://github.com/mblua/AgentsCommander/issues/1830), [PR #1838](https://github.com/mblua/AgentsCommander/pull/1838))
- docs: docs(npm): describe published 0.30.5 resolver on the package page (refs #1834) ([#1834](https://github.com/mblua/AgentsCommander/issues/1834), [PR #1843](https://github.com/mblua/AgentsCommander/pull/1843))
- feature: feat(#1844): default the selected row bar color to #630707 ([#1844](https://github.com/mblua/AgentsCommander/issues/1844), [PR #1847](https://github.com/mblua/AgentsCommander/pull/1847))
- fix: fix(1856): reconcile session communication from the polled listing ([#1856](https://github.com/mblua/AgentsCommander/issues/1856), [PR #1864](https://github.com/mblua/AgentsCommander/pull/1864))
- fix: fix(1857): derived aggregated blocked-menu toast, taskbar attention, pinned toasts, exit race ([#1857](https://github.com/mblua/AgentsCommander/issues/1857), [PR #1866](https://github.com/mblua/AgentsCommander/pull/1866))
- fix: fix(1858): give the blocked menu its own glyph and colour, and style .toast-item__action ([#1858](https://github.com/mblua/AgentsCommander/issues/1858), [PR #1869](https://github.com/mblua/AgentsCommander/pull/1869))
- fix: fix(1859): roll the blocked-menu state up through every level that can hide a row ([#1859](https://github.com/mblua/AgentsCommander/issues/1859), [PR #1874](https://github.com/mblua/AgentsCommander/pull/1874))
- feature: feat(catalog): add Muse Code to the embedded coding-agents catalog (#1860) ([#1860](https://github.com/mblua/AgentsCommander/issues/1860), [PR #1888](https://github.com/mblua/AgentsCommander/pull/1888))
- maintenance: refactor(sidebar): one catalogue component for session-row context menus; Root Agent menu at parity (#1871) ([#1871](https://github.com/mblua/AgentsCommander/issues/1871), [PR #1886](https://github.com/mblua/AgentsCommander/pull/1886))
- docs: docs: document codebase-memory-mcp for Claude Code and drop the MCP stance (#1882) ([#1882](https://github.com/mblua/AgentsCommander/issues/1882), [PR #1885](https://github.com/mblua/AgentsCommander/pull/1885))
- docs: docs(1893): fill Unreleased for 0.31.0 ([#1893](https://github.com/mblua/AgentsCommander/issues/1893), [PR #1894](https://github.com/mblua/AgentsCommander/pull/1894))

## Install from npm

```text
npx @mblua/agentscommander@0.31.0
```

## Verification identity

- Release issue: https://github.com/mblua/AgentsCommander/issues/1893
- Reviewed evidence set: `bound by the exact release-authority-v1 review-set-sha256 field`
- Candidate assets: 17
- Predecessor: `v0.30.5`

The workflow must publish this body verbatim, make the GitHub Release public and immutable before npm, and attach provenance for the exact assets and package.
