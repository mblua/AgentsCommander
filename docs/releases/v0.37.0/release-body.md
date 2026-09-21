# Agents Commander v0.37.0

### Fixed

- **`list-peers-lean --snapshot-targets` no longer aborts on a neighbouring directory it cannot verify.** A linked or junctioned room, a stale `__agent_*` replica, or a sibling project directory that fails identity verification is now skipped and counted instead of failing the whole command with `Error: unsafe_path` and exit 1. The counts are reported on stderr as integers only (`snapshot_targets_note skipped_project_children=… skipped_rooms=… skipped_replicas=…`), a `[]` result now says why on stderr, and a genuinely rejected `--root` names the rule that rejected it. Every emitted target still passes the same unchanged identity verification, and stdout JSON is unchanged. ([#2228](https://github.com/mblua/AgentsCommander/issues/2228), [#2223](https://github.com/mblua/AgentsCommander/issues/2223))
- **`terminal-snapshot` now says which argument it rejected and why.** Every CLI-originated rejection adds `field=` (when exactly one argument is at fault, from `token`, `root`, `to`, `format`, `output`, `timeout`) and `reason=` to the `terminal_snapshot_error` line, so the six previously identical `invalid_request` failures are distinguishable. `code=` values, exit codes, stdout and the HTTP API are unchanged, and the new tokens are fixed literals that never carry a path or a token. ([#2227](https://github.com/mblua/AgentsCommander/issues/2227), [#2223](https://github.com/mblua/AgentsCommander/issues/2223))

## Included scope

- fix: fix(2074): raise the post-publish propagation budget and name the published-but-lagging case ([#2074](https://github.com/mblua/AgentsCommander/issues/2074), [PR #2193](https://github.com/mblua/AgentsCommander/pull/2193))
- fix: fix(2129): name the remote in the branch-stale notice, never the branch twice ([#2129](https://github.com/mblua/AgentsCommander/issues/2129), [PR #2174](https://github.com/mblua/AgentsCommander/pull/2174))
- fix: fix(2141): no branch-stale notice on a branch with no commits of its own ([#2141](https://github.com/mblua/AgentsCommander/issues/2141), [PR #2189](https://github.com/mblua/AgentsCommander/pull/2189))
- maintenance: docs(config-seed): note the BUILTIN_AGENT_SUPPORT gate on factory masters and re-seed (refs #2146) ([#2146](https://github.com/mblua/AgentsCommander/issues/2146), [PR #2217](https://github.com/mblua/AgentsCommander/pull/2217))
- maintenance: chore(sidebar): remove unconsumed sessionsStore.groupedSessions (refs #2148) ([#2148](https://github.com/mblua/AgentsCommander/issues/2148), [PR #2242](https://github.com/mblua/AgentsCommander/pull/2242))
- maintenance: chore(2155): gate new test-code duplication before the CI matrices ([#2155](https://github.com/mblua/AgentsCommander/issues/2155), [PR #2166](https://github.com/mblua/AgentsCommander/pull/2166))
- fix: fix(#2156): idle-burst filter ON by default for recognised coding agents ([#2156](https://github.com/mblua/AgentsCommander/issues/2156), [PR #2191](https://github.com/mblua/AgentsCommander/pull/2191))
- feature: feat(config-seed): OS-aware variants for all five tiers (refs #2162) ([#2162](https://github.com/mblua/AgentsCommander/issues/2162), [PR #2207](https://github.com/mblua/AgentsCommander/pull/2207))
- feature: feat(loops): validate Loop targets at startup and surface unresolvable ones, refs #2171 ([#2171](https://github.com/mblua/AgentsCommander/issues/2171), [PR #2221](https://github.com/mblua/AgentsCommander/pull/2221))
- fix: fix(screenshot): place macOS overlays in logical points (#2173) ([#2173](https://github.com/mblua/AgentsCommander/issues/2173), [PR #2175](https://github.com/mblua/AgentsCommander/pull/2175))
- fix: fix(2176): resolve the loop wake agent from the replica's pinned agent and profile ([#2176](https://github.com/mblua/AgentsCommander/issues/2176), [PR #2200](https://github.com/mblua/AgentsCommander/pull/2200))
- feature: feat(2179): pulsed NonStop alarm in sound.ts, gated by the global mute ([#2179](https://github.com/mblua/AgentsCommander/issues/2179), [PR #2187](https://github.com/mblua/AgentsCommander/pull/2187))
- feature: feat(2180): non_stop_alarm contract and its single listener ([#2180](https://github.com/mblua/AgentsCommander/issues/2180), [PR #2198](https://github.com/mblua/AgentsCommander/pull/2198))
- maintenance: refactor(2181): emit the non-stop alarm event instead of a Win32 beep ([#2181](https://github.com/mblua/AgentsCommander/issues/2181), [PR #2208](https://github.com/mblua/AgentsCommander/pull/2208))
- fix: fix(2182): the NonStop sound alert is no longer Windows-only ([#2182](https://github.com/mblua/AgentsCommander/issues/2182), [PR #2209](https://github.com/mblua/AgentsCommander/pull/2209))
- maintenance: chore(2183): move plans/ out of the repository and enforce the ban ([#2183](https://github.com/mblua/AgentsCommander/issues/2183), [PR #2196](https://github.com/mblua/AgentsCommander/pull/2196))
- fix: fix(2185): silence the expected rejections of the tokenless token fixture ([#2185](https://github.com/mblua/AgentsCommander/issues/2185), [PR #2186](https://github.com/mblua/AgentsCommander/pull/2186))
- fix: fix(build): skip the testable binary copy on non-Windows, refs #2188 ([#2188](https://github.com/mblua/AgentsCommander/issues/2188), [PR #2197](https://github.com/mblua/AgentsCommander/pull/2197))
- feature: feat(settings): new-install defaults for rail colour and restart wake (#2190) ([#2190](https://github.com/mblua/AgentsCommander/issues/2190), [PR #2192](https://github.com/mblua/AgentsCommander/pull/2192))
- fix: fix(#2202): a room waiting on CI counts as working on every user-facing surface ([#2202](https://github.com/mblua/AgentsCommander/issues/2202), [PR #2215](https://github.com/mblua/AgentsCommander/pull/2215))
- maintenance: chore(2210): drop the orphaned 1283 local-session proof scripts ([#2210](https://github.com/mblua/AgentsCommander/issues/2210), [PR #2211](https://github.com/mblua/AgentsCommander/pull/2211))
- maintenance: test(activity_log): make coalescer prune test deterministic, refs #2212 ([#2212](https://github.com/mblua/AgentsCommander/issues/2212), [PR #2220](https://github.com/mblua/AgentsCommander/pull/2220))
- fix: fix(scripts): give dependency-cruiser spawnSync an explicit maxBuffer (refs #2213) ([#2213](https://github.com/mblua/AgentsCommander/issues/2213), [PR #2243](https://github.com/mblua/AgentsCommander/pull/2243))
- fix: fix(#2218): ago test helper panics instead of saturating to now ([#2218](https://github.com/mblua/AgentsCommander/issues/2218), [PR #2229](https://github.com/mblua/AgentsCommander/pull/2229))
- maintenance: ci(2219): inventory restored gate-debug target before cargo ([#2219](https://github.com/mblua/AgentsCommander/issues/2219), [PR #2302](https://github.com/mblua/AgentsCommander/pull/2302))
- fix: fix(#2222): reveal the terminal when the already-selected row is clicked ([#2222](https://github.com/mblua/AgentsCommander/issues/2222), [PR #2226](https://github.com/mblua/AgentsCommander/pull/2226))
- fix: fix(terminal-snapshot): attribute invalid_request to a field and a reason (#2227) ([#2227](https://github.com/mblua/AgentsCommander/issues/2227), [PR #2230](https://github.com/mblua/AgentsCommander/pull/2230))
- fix: fix(cli): fail-soft --snapshot-targets discovery and name the path rules (#2228) ([#2228](https://github.com/mblua/AgentsCommander/issues/2228), [PR #2231](https://github.com/mblua/AgentsCommander/pull/2231))
- feature: feat(loops): persist per-Loop sessionStart and deliver Fresh via restart (refs #2237) ([#2237](https://github.com/mblua/AgentsCommander/issues/2237), [PR #2252](https://github.com/mblua/AgentsCommander/pull/2252))
- feature: feat(loops): carry sessionStart on Loop create and update IPC (refs #2238) ([#2238](https://github.com/mblua/AgentsCommander/issues/2238), [PR #2262](https://github.com/mblua/AgentsCommander/pull/2262))
- feature: feat(loops): --session-start on loop create and update, and fix the cli_loop ETXTBSY race (refs #2239) ([#2239](https://github.com/mblua/AgentsCommander/issues/2239), [PR #2286](https://github.com/mblua/AgentsCommander/pull/2286))
- feature: feat(loops): persistent Accumulate control in the Loop modals (refs #2240) ([#2240](https://github.com/mblua/AgentsCommander/issues/2240), [PR #2290](https://github.com/mblua/AgentsCommander/pull/2290))
- maintenance: docs(loops): document fresh session start (refs #2241) ([#2241](https://github.com/mblua/AgentsCommander/issues/2241), [PR #2307](https://github.com/mblua/AgentsCommander/pull/2307))
- feature: feat(blocking-menus): publish Claude and Codex patterns (#2248) ([#2248](https://github.com/mblua/AgentsCommander/issues/2248), [PR #2249](https://github.com/mblua/AgentsCommander/pull/2249))
- feature: feat(blocking-menus): detect Claude choices and OpenCode permissions (refs #2251) ([#2251](https://github.com/mblua/AgentsCommander/issues/2251), [PR #2263](https://github.com/mblua/AgentsCommander/pull/2263))
- feature: feat: capture provider records for Co-managed rooms (refs #2264); internal Co-managed foundation, disabled by default, production routing pending ([#2264](https://github.com/mblua/AgentsCommander/issues/2264), [PR #2299](https://github.com/mblua/AgentsCommander/pull/2299))
- feature: feat: add Co-managed room config and Jev settings (refs #2265); internal Co-managed foundation, disabled by default, production routing pending ([#2265](https://github.com/mblua/AgentsCommander/issues/2265), [PR #2305](https://github.com/mblua/AgentsCommander/pull/2305))
- feature: feat: add capture sink and durable state (refs #2266); internal Co-managed foundation, disabled by default, production routing pending ([#2266](https://github.com/mblua/AgentsCommander/issues/2266), [PR #2308](https://github.com/mblua/AgentsCommander/pull/2308))
- feature: feat(sidebar): V3 Root Agent halo and rail width token (#2277) ([#2277](https://github.com/mblua/AgentsCommander/issues/2277), [PR #2295](https://github.com/mblua/AgentsCommander/pull/2295))
- maintenance: test(2291): sync cli_loop isolation contract with run_output route ([#2291](https://github.com/mblua/AgentsCommander/issues/2291), [PR #2293](https://github.com/mblua/AgentsCommander/pull/2293))
- feature: feat(blocking-menus): detect Claude mid-response disconnect (refs #2292) ([#2292](https://github.com/mblua/AgentsCommander/issues/2292), [PR #2298](https://github.com/mblua/AgentsCommander/pull/2298))
- fix: fix(quit): add main-window quit gate (refs #2296) ([#2296](https://github.com/mblua/AgentsCommander/issues/2296), [PR #2310](https://github.com/mblua/AgentsCommander/pull/2310))
- fix: fix(quit): close clients through main quit gate (refs #2297) ([#2297](https://github.com/mblua/AgentsCommander/issues/2297), [PR #2330](https://github.com/mblua/AgentsCommander/pull/2330))
- fix: fix(#2300): exempt quit_application from IPC overdue alerts ([#2300](https://github.com/mblua/AgentsCommander/issues/2300), [PR #2320](https://github.com/mblua/AgentsCommander/pull/2320))
- maintenance: style(rust): format menu guard tests merged in #2292 (#2301) ([#2301](https://github.com/mblua/AgentsCommander/issues/2301), [PR #2303](https://github.com/mblua/AgentsCommander/pull/2303))
- maintenance: docs: prototype coding agent order in both views (refs #2311) ([#2311](https://github.com/mblua/AgentsCommander/issues/2311), [PR #2339](https://github.com/mblua/AgentsCommander/pull/2339))
- maintenance: test: serialize copied CLI copy and spawn in phase A (refs #2322) ([#2322](https://github.com/mblua/AgentsCommander/issues/2322), [PR #2324](https://github.com/mblua/AgentsCommander/pull/2324))

## Install from npm

```text
npx @mblua/agentscommander@0.37.0
```

## Verification identity

- Release issue: https://github.com/mblua/AgentsCommander/issues/2331
- Reviewed evidence set: `bound by the exact release-authority-v1 review-set-sha256 field`
- Candidate assets: 17
- Predecessor: `v0.36.0`

The workflow must publish this body verbatim, make the GitHub Release public and immutable before npm, and attach provenance for the exact assets and package.
