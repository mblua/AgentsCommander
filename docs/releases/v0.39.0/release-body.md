# Agents Commander v0.39.0

### Added

- **The sidebar can collapse to a compact rail.** A `>>` control on the groups rail collapses the sidebar to the rail, theme, settings and `<<` controls, and the terminal takes the released space. Selecting any group expands it again and restores the width it had before collapsing. A configurable shortcut toggles it (default `Ctrl+Shift+E`, set in Settings); it works with terminal focus and the matched key never reaches the terminal. ([#2236](https://github.com/mblua/AgentsCommander/issues/2236), [#2283](https://github.com/mblua/AgentsCommander/issues/2283), [#2284](https://github.com/mblua/AgentsCommander/issues/2284), [#2285](https://github.com/mblua/AgentsCommander/issues/2285))
- **Coding agents can be reordered.** Move controls on the Settings rows and the profile-assignment Step 1 cards change the order, and both lists now show the saved order instead of an alphabetical one. A failed move shows an inline error and never applies a guessed order. ([#2314](https://github.com/mblua/AgentsCommander/issues/2314))
- **`room activity --project <PROJECT>` lists every room of a project** with its working state, CI state (`running`, `idle` or `unknown`) and task title. It is read-only. ([#2374](https://github.com/mblua/AgentsCommander/issues/2374))
- **Loop modals say when saving restarts the schedule.** Editing a Loop shows a notice only when the save would move the next run; creating a Loop always says the schedule starts at creation. ([#2288](https://github.com/mblua/AgentsCommander/issues/2288))

### Removed

- **The Guide window (Hints and Tutorial) and its ActionBar button are gone.** Home is unchanged, and existing settings files keep loading. ([#2412](https://github.com/mblua/AgentsCommander/issues/2412))

### Fixed

- **A window maximized on a first run is now remembered.** With no placement saved yet, AC samples the window's rectangle at startup, so launch, maximize and close stores that rectangle with `"maximized"` and the next start comes back maximized. A saved rectangle still wins over the startup sample, and a window that is already maximized, fullscreen or minimized when AC starts still stores no rectangle. ([#2393](https://github.com/mblua/AgentsCommander/issues/2393))
- **The Resource Monitor no longer runs out of memory on a process-tree cycle.** Windows PID reuse could leave a parent-PID cycle that made the tree walk loop until the app aborted; each process is now visited once. ([#2443](https://github.com/mblua/AgentsCommander/issues/2443))
- **Codex turns are closed reliably.** The `turn_complete` closure is accepted as well as `task_complete`, and a late answer for a superseded turn is no longer emitted. ([#2356](https://github.com/mblua/AgentsCommander/issues/2356))
- **Config files publish on deep Windows paths.** Paths longer than `MAX_PATH` no longer fail with os error 3. ([#2378](https://github.com/mblua/AgentsCommander/issues/2378))
- **An auto-closed orchestrator keeps its context.** A CI or other internal wake resumes its previous session, and a failed reopen keeps the close markers so a retry still resumes. ([#2411](https://github.com/mblua/AgentsCommander/issues/2411), [#2413](https://github.com/mblua/AgentsCommander/issues/2413))
- **Process liveness is real on Linux and macOS.** Session and `daemon.pid` checks no longer treat every process as alive. ([#2382](https://github.com/mblua/AgentsCommander/issues/2382), [#2394](https://github.com/mblua/AgentsCommander/issues/2394))
- **The Telegram icon in the sidebar menu renders on Linux and macOS.** It was drawn at 0x0 in WebKit. ([#2399](https://github.com/mblua/AgentsCommander/issues/2399))
- **Typing-hold padlock corrections.** The held count has no `#` prefix, the padlock is open and dimmed while inactive and closed and colored while holding, and a manual close releases itself after twice the `typingHoldSeconds` window. ([#2379](https://github.com/mblua/AgentsCommander/issues/2379))

## Included scope

- maintenance: ci(2234): live proofs of the cognitive-complexity gate, and its documentation ([#2260](https://github.com/mblua/AgentsCommander/issues/2260), [PR #2392](https://github.com/mblua/AgentsCommander/pull/2392))
- docs: docs: Co-managed phase 10 documentation (#2273) ([#2273](https://github.com/mblua/AgentsCommander/issues/2273), [PR #2403](https://github.com/mblua/AgentsCommander/pull/2403))
- feature: feat(compact): compact sidebar CSS — overlay, compact row, content removal (#2283) ([#2283](https://github.com/mblua/AgentsCommander/issues/2283), [PR #2401](https://github.com/mblua/AgentsCommander/pull/2401))
- feature: feat(compact): banner toggle, activation button, hotkey hydration (#2284) ([#2284](https://github.com/mblua/AgentsCommander/issues/2284), [PR #2416](https://github.com/mblua/AgentsCommander/pull/2416))
- feature: feat(compact): configurable toggle hotkey, terminal veto and capture control (#2285) ([#2285](https://github.com/mblua/AgentsCommander/issues/2285), [PR #2427](https://github.com/mblua/AgentsCommander/pull/2427))
- feature: feat(loops): notice when saving rebaselines the schedule (#2288) ([#2288](https://github.com/mblua/AgentsCommander/issues/2288), [PR #2407](https://github.com/mblua/AgentsCommander/pull/2407))
- feature: feat(2314): reorder coding agents from Settings rows and Step 1 cards (#2306 P3) ([#2314](https://github.com/mblua/AgentsCommander/issues/2314), [PR #2390](https://github.com/mblua/AgentsCommander/pull/2390))
- fix: fix(codex): accept turn_complete alias and never re-emit a superseded turn (#2356) ([#2356](https://github.com/mblua/AgentsCommander/issues/2356), [PR #2446](https://github.com/mblua/AgentsCommander/pull/2446))
- feature: feat(#2374): room activity status CLI ([#2374](https://github.com/mblua/AgentsCommander/issues/2374), [PR #2398](https://github.com/mblua/AgentsCommander/pull/2398))
- fix: fix(config): use verbatim paths for ReplaceFileW in publish_temp_config (#2378) ([#2378](https://github.com/mblua/AgentsCommander/issues/2378), [PR #2441](https://github.com/mblua/AgentsCommander/pull/2441))
- fix: fix(terminal): typing-hold corrections - no #, gray open / colored closed padlock, manual hold auto-release (#2379) ([#2379](https://github.com/mblua/AgentsCommander/issues/2379), [PR #2400](https://github.com/mblua/AgentsCommander/pull/2400))
- fix: fix(#2382): real Unix process liveness in pid_is_alive ([#2382](https://github.com/mblua/AgentsCommander/issues/2382), [PR #2397](https://github.com/mblua/AgentsCommander/pull/2397))
- maintenance: test(capture): de-flake a_baseline_is_consumed_but_never_routed (#2389) ([#2389](https://github.com/mblua/AgentsCommander/issues/2389), [PR #2438](https://github.com/mblua/AgentsCommander/pull/2438))
- fix: fix(#2393): persist first-run maximize placement seed ([#2393](https://github.com/mblua/AgentsCommander/issues/2393), [PR #2404](https://github.com/mblua/AgentsCommander/pull/2404))
- fix: fix(#2394): real Unix liveness probe for daemon.pid ([#2394](https://github.com/mblua/AgentsCommander/issues/2394), [PR #2415](https://github.com/mblua/AgentsCommander/pull/2415))
- fix: fix(ui): size TelegramIcon at 14x14 so the sidebar Telegram icon renders in WebKit (#2399) ([#2399](https://github.com/mblua/AgentsCommander/issues/2399), [PR #2402](https://github.com/mblua/AgentsCommander/pull/2402))
- feature: feat(sidebar): Co-managed from context menu + ring around activity dot (#2408) ([#2408](https://github.com/mblua/AgentsCommander/issues/2408), [PR #2428](https://github.com/mblua/AgentsCommander/pull/2428))
- maintenance: test(styles): guard that every markup class has a CSS rule (#2410) ([#2410](https://github.com/mblua/AgentsCommander/issues/2410), [PR #2439](https://github.com/mblua/AgentsCommander/pull/2439))
- fix: fix(#2411): internal-system wake resumes an auto-closed orchestrator ([#2411](https://github.com/mblua/AgentsCommander/issues/2411), [PR #2414](https://github.com/mblua/AgentsCommander/pull/2414))
- feature: feat: remove the Guide window (Hints and Tutorial) and its ActionBar button ([#2412](https://github.com/mblua/AgentsCommander/issues/2412), [PR #2426](https://github.com/mblua/AgentsCommander/pull/2426))
- fix: fix(session): keep close markers when a coordinator create fails (refs #2413) ([#2413](https://github.com/mblua/AgentsCommander/issues/2413), [PR #2437](https://github.com/mblua/AgentsCommander/pull/2437))
- maintenance: ci: light CI on every run, full CI every 10th run, refs #2417 ([#2417](https://github.com/mblua/AgentsCommander/issues/2417), [PR #2418](https://github.com/mblua/AgentsCommander/pull/2418))
- maintenance: ci: drop push trigger from PR regression gates, refs #2419 ([#2419](https://github.com/mblua/AgentsCommander/issues/2419), [PR #2420](https://github.com/mblua/AgentsCommander/pull/2420))
- maintenance: ci: Linux clippy and cognitive gate on every PR, refs #2423 ([#2423](https://github.com/mblua/AgentsCommander/issues/2423), [PR #2424](https://github.com/mblua/AgentsCommander/pull/2424))
- maintenance: chore(scripts): fix stale header comment in check-cognitive-complexity (#2425) ([#2425](https://github.com/mblua/AgentsCommander/issues/2425), [PR #2440](https://github.com/mblua/AgentsCommander/pull/2440))
- fix: fix(resource_monitor): stop process-tree walk looping on a PID-reuse cycle (refs #2443) ([#2443](https://github.com/mblua/AgentsCommander/issues/2443), [PR #2445](https://github.com/mblua/AgentsCommander/pull/2445))
- docs: docs: file naming convention target state (refs #2448) ([#2448](https://github.com/mblua/AgentsCommander/issues/2448), [PR #2449](https://github.com/mblua/AgentsCommander/pull/2449))

## Install from npm

```text
npx @mblua/agentscommander@0.39.0
```

## Verification identity

- Release issue: https://github.com/mblua/AgentsCommander/issues/2459
- Reviewed evidence set: `bound by the exact release-authority-v1 review-set-sha256 field`
- Candidate assets: 17
- Predecessor: `v0.38.0`

The workflow must publish this body verbatim, make the GitHub Release public and immutable before npm, and attach provenance for the exact assets and package.
