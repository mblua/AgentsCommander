# Agents Commander v0.38.0

### Added

- **Peer wakes are held while you are typing.** A message injected into a session by another agent now waits instead of landing in the middle of your keystrokes. The hold releases after a quiet window, or immediately when you open the padlock added to the terminal status bar, which also shows how many messages are being held. The window is the new `typingHoldSeconds` setting in Settings > General (default 30 seconds, accepted 1-3600; an invalid draft is rejected, never coerced). Your own input is never blocked. ([#2336](https://github.com/mblua/AgentsCommander/issues/2336), [#2337](https://github.com/mblua/AgentsCommander/issues/2337), [#2335](https://github.com/mblua/AgentsCommander/issues/2335))
- **The Resource Monitor has an integral view**, with validated PID filtering, text search, sorting, several pinned expansions at once, a process summary and hierarchy, explicit feedback when observation is partial, accessible controls and a responsive layout. ([#2245](https://github.com/mblua/AgentsCommander/issues/2245), [#2244](https://github.com/mblua/AgentsCommander/issues/2244))

### Changed

- **Terminal snapshots are now enabled by default.** A fresh install and an existing settings file without the key both start with snapshots on; an explicit `false` is preserved and a malformed value still fails closed. The Settings checkbox copy now reads "Enabled by default." ([#2317](https://github.com/mblua/AgentsCommander/issues/2317), [#2318](https://github.com/mblua/AgentsCommander/issues/2318), [#2319](https://github.com/mblua/AgentsCommander/issues/2319), [#2309](https://github.com/mblua/AgentsCommander/issues/2309))
- **The main window remembers its position, size and maximized state.** The placement pair is persisted through one narrow writer, fullscreen and minimized stay transient, and unmaximizing restores the last usable normal rectangle. ([#2348](https://github.com/mblua/AgentsCommander/issues/2348), [#2349](https://github.com/mblua/AgentsCommander/issues/2349), [#2350](https://github.com/mblua/AgentsCommander/issues/2350), [#2347](https://github.com/mblua/AgentsCommander/issues/2347))

### Fixed

- **CI activity on a repository's default branch no longer rings the chip or notifies the orchestrator.** A resolved default branch reports idle whatever GitHub answers, so it adds no room CI activity and sends no `ci-started` or `ci-finished` notice; an unresolved default branch is unchanged. The chip tooltip no longer claims `no CI activity for this commit`, because an absent CI suffix cannot tell idle from unknown. Rate-limit backoff, failure warnings and branch staleness are untouched. ([#2326](https://github.com/mblua/AgentsCommander/issues/2326), [#2329](https://github.com/mblua/AgentsCommander/issues/2329))
- **A coding agent's context file no longer repeats the `Role.md` YAML frontmatter.** That block is display metadata for the agent listing, not role instructions, so it is stripped once at the single join point that writes every generated `AGENTS.md` and `CLAUDE.md`. The canonical `Role.md` on disk keeps its frontmatter. ([#2364](https://github.com/mblua/AgentsCommander/issues/2364))
- **Deferred menu injections no longer surface as an Application Error.** A recoverable deferral from either mailbox injection path is logged at debug instead; real PTY failures still raise Application Error unchanged. ([#1883](https://github.com/mblua/AgentsCommander/issues/1883))
- **Canonical `room-<n>-<team>/<agent>` identifiers are accepted where only the legacy `wg-` form was.** Terminal-snapshot targets and the api-helper session bridge now share one room-aware validator, with every length, delimiter and character restriction unchanged. ([#2316](https://github.com/mblua/AgentsCommander/issues/2316), [#2361](https://github.com/mblua/AgentsCommander/issues/2361))
- **Internal work with no user-visible change**: the cognitive-complexity CI gate and its baseline, capture pipeline and three-platform merge; groundwork for the compact sidebar (core signal, main and browser hosts, terminal pulse, and the `sidebarCompactHotkey` setting) and for reordering registered coding agents (persisted order and its narrow IPC command); added test coverage; and documentation updates. ([#2234](https://github.com/mblua/AgentsCommander/issues/2234), [#2236](https://github.com/mblua/AgentsCommander/issues/2236), [#2306](https://github.com/mblua/AgentsCommander/issues/2306), [#2289](https://github.com/mblua/AgentsCommander/issues/2289), [#2321](https://github.com/mblua/AgentsCommander/issues/2321), [#2323](https://github.com/mblua/AgentsCommander/issues/2323), [#2246](https://github.com/mblua/AgentsCommander/issues/2246), [#2327](https://github.com/mblua/AgentsCommander/issues/2327))

## Included scope

- fix: fix(mailbox): avoid Application Error for menu deferrals (refs #1883) ([#1883](https://github.com/mblua/AgentsCommander/issues/1883), [PR #2333](https://github.com/mblua/AgentsCommander/pull/2333))
- feature: feat(resource-monitor): integral view with PID filtering (refs #2245) ([#2245](https://github.com/mblua/AgentsCommander/issues/2245), [PR #2315](https://github.com/mblua/AgentsCommander/pull/2315))
- maintenance: docs(resource-monitor): document integral view (refs #2246) ([#2246](https://github.com/mblua/AgentsCommander/issues/2246), [PR #2355](https://github.com/mblua/AgentsCommander/pull/2355))
- maintenance: ci(2234): cognitive-complexity script skeleton, pinned threshold and suppression scan ([#2253](https://github.com/mblua/AgentsCommander/issues/2253), [PR #2294](https://github.com/mblua/AgentsCommander/pull/2294))
- maintenance: ci(2234): cognitive-complexity source lexer and identity rule ([#2254](https://github.com/mblua/AgentsCommander/issues/2254), [PR #2342](https://github.com/mblua/AgentsCommander/pull/2342))
- maintenance: ci(2234): add capture pipeline and verdict (refs #2255) ([#2255](https://github.com/mblua/AgentsCommander/issues/2255), [PR #2345](https://github.com/mblua/AgentsCommander/pull/2345))
- maintenance: ci(2234): baseline ratchet, three-platform merge and probe classifier ([#2256](https://github.com/mblua/AgentsCommander/issues/2256), [PR #2358](https://github.com/mblua/AgentsCommander/pull/2358))
- maintenance: ci(2234): pin the toolchain of the five measuring invocations ([#2257](https://github.com/mblua/AgentsCommander/issues/2257), [PR #2369](https://github.com/mblua/AgentsCommander/pull/2369))
- maintenance: ci(2234): wire the three Rust legs to the cognitive detector (report mode) ([#2258](https://github.com/mblua/AgentsCommander/issues/2258), [PR #2377](https://github.com/mblua/AgentsCommander/pull/2377))
- maintenance: ci(2234): capture the cognitive-complexity baseline and enforce the gate ([#2259](https://github.com/mblua/AgentsCommander/issues/2259), [PR #2388](https://github.com/mblua/AgentsCommander/pull/2388))
- feature: feat: supervise co-managed transcript readers (refs #2267) ([#2267](https://github.com/mblua/AgentsCommander/issues/2267), [PR #2340](https://github.com/mblua/AgentsCommander/pull/2340))
- feature: feat: bound Codex turns at task_complete (refs #2268) ([#2268](https://github.com/mblua/AgentsCommander/issues/2268), [PR #2351](https://github.com/mblua/AgentsCommander/pull/2351))
- feature: feat: classify candidates with Jev and block secrets before egress (refs #2269) ([#2269](https://github.com/mblua/AgentsCommander/issues/2269), [PR #2357](https://github.com/mblua/AgentsCommander/pull/2357))
- feature: feat(2270): Co-managed phase 7 - routing, provenance queue and supervisor ([#2270](https://github.com/mblua/AgentsCommander/issues/2270), [PR #2368](https://github.com/mblua/AgentsCommander/pull/2368))
- feature: feat(2271): co-managed session status indicator ([#2271](https://github.com/mblua/AgentsCommander/issues/2271), [PR #2385](https://github.com/mblua/AgentsCommander/pull/2385))
- feature: feat: Co-managed phase 9 - enablement UI ([#2272](https://github.com/mblua/AgentsCommander/issues/2272), [PR #2391](https://github.com/mblua/AgentsCommander/pull/2391))
- feature: feat(compact): compact core signal and main host (#2278) ([#2278](https://github.com/mblua/AgentsCommander/issues/2278), [PR #2325](https://github.com/mblua/AgentsCommander/pull/2325))
- feature: feat(2279): keep compact terminal pulse alive ([#2279](https://github.com/mblua/AgentsCommander/issues/2279), [PR #2344](https://github.com/mblua/AgentsCommander/pull/2344))
- feature: feat(browser): add compact sidebar parity (refs #2280) ([#2280](https://github.com/mblua/AgentsCommander/issues/2280), [PR #2346](https://github.com/mblua/AgentsCommander/pull/2346))
- feature: feat(settings): add sidebar compact toggle hotkey (#2281) ([#2281](https://github.com/mblua/AgentsCommander/issues/2281), [PR #2362](https://github.com/mblua/AgentsCommander/pull/2362))
- feature: feat(compact): two-button compact toolbar, rail autoexpansion, inert width presets (#2282) ([#2282](https://github.com/mblua/AgentsCommander/issues/2282), [PR #2387](https://github.com/mblua/AgentsCommander/pull/2387))
- maintenance: test(loops): pin LoopAPI create/update request payload contract (#2289) ([#2289](https://github.com/mblua/AgentsCommander/issues/2289), [PR #2383](https://github.com/mblua/AgentsCommander/pull/2383))
- feature: feat(2312): persist explicit registered coding-agent order (#2306 P1) ([#2312](https://github.com/mblua/AgentsCommander/issues/2312), [PR #2354](https://github.com/mblua/AgentsCommander/pull/2354))
- feature: feat(2313): persist registered coding-agent moves across settings writers (#2306 P2) ([#2313](https://github.com/mblua/AgentsCommander/issues/2313), [PR #2373](https://github.com/mblua/AgentsCommander/pull/2373))
- fix: fix(2316): accept canonical room FQNs in the snapshot target validator ([#2316](https://github.com/mblua/AgentsCommander/issues/2316), [PR #2352](https://github.com/mblua/AgentsCommander/pull/2352))
- fix: fix(#2317): enable terminal snapshots by default with safe settings semantics ([#2317](https://github.com/mblua/AgentsCommander/issues/2317), [PR #2363](https://github.com/mblua/AgentsCommander/pull/2363))
- fix: fix(#2318): update terminal snapshot Settings UI for default-on behavior ([#2318](https://github.com/mblua/AgentsCommander/issues/2318), [PR #2381](https://github.com/mblua/AgentsCommander/pull/2381))
- maintenance: docs: terminal snapshots are on by default (#2319) ([#2319](https://github.com/mblua/AgentsCommander/issues/2319), [PR #2384](https://github.com/mblua/AgentsCommander/pull/2384))
- fix: test(2321): serialize default-root binary spawn ([#2321](https://github.com/mblua/AgentsCommander/issues/2321), [PR #2370](https://github.com/mblua/AgentsCommander/pull/2370))
- maintenance: test: serialize copied CLI copy and spawn in phase B suites (refs #2323) ([#2323](https://github.com/mblua/AgentsCommander/issues/2323), [PR #2334](https://github.com/mblua/AgentsCommander/pull/2334))
- maintenance: docs: align CI signal docs with default-branch suppression ([#2327](https://github.com/mblua/AgentsCommander/issues/2327), [PR #2343](https://github.com/mblua/AgentsCommander/pull/2343))
- fix: fix(2329): suppress CI activity on the resolved default branch ([#2329](https://github.com/mblua/AgentsCommander/issues/2329), [PR #2341](https://github.com/mblua/AgentsCommander/pull/2341))
- feature: feat(2336): defer peer wakes while the user is typing ([#2336](https://github.com/mblua/AgentsCommander/issues/2336), [PR #2353](https://github.com/mblua/AgentsCommander/pull/2353))
- feature: feat(2337): padlock status-bar control with held-injection count ([#2337](https://github.com/mblua/AgentsCommander/issues/2337), [PR #2372](https://github.com/mblua/AgentsCommander/pull/2372))
- fix: fix(2348): persist and apply the main window placement pair ([#2348](https://github.com/mblua/AgentsCommander/issues/2348), [PR #2359](https://github.com/mblua/AgentsCommander/pull/2359))
- fix: fix(2349): observe and flush main window placement from the renderer ([#2349](https://github.com/mblua/AgentsCommander/issues/2349), [PR #2367](https://github.com/mblua/AgentsCommander/pull/2367))
- maintenance: docs(2350): document main window placement and display state ([#2350](https://github.com/mblua/AgentsCommander/issues/2350), [PR #2380](https://github.com/mblua/AgentsCommander/pull/2380))
- fix: fix(session-bridge): accept room-aware FQNs in api-helper results (#2361) ([#2361](https://github.com/mblua/AgentsCommander/issues/2361), [PR #2386](https://github.com/mblua/AgentsCommander/pull/2386))
- fix: fix(2364): strip Role.md YAML frontmatter from generated agent context ([#2364](https://github.com/mblua/AgentsCommander/issues/2364), [PR #2371](https://github.com/mblua/AgentsCommander/pull/2371))

## Install from npm

```text
npx @mblua/agentscommander@0.38.0
```

## Verification identity

- Release issue: https://github.com/mblua/AgentsCommander/issues/2395
- Reviewed evidence set: `bound by the exact release-authority-v1 review-set-sha256 field`
- Candidate assets: 17
- Predecessor: `v0.37.0`

The workflow must publish this body verbatim, make the GitHub Release public and immutable before npm, and attach provenance for the exact assets and package.
