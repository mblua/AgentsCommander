# Release preparation plan: v0.40.0

Status: `REVIEW_REQUIRED`. This generated bundle is not approval to implement, tag, publish, release, or deploy.

Repository: `mblua/AgentsCommander`  
Release issue: https://github.com/mblua/AgentsCommander/issues/2673  
Candidate: `0.40.0` / `v0.40.0`  
Planning base: `db9d635e25448c4331094c942a7fb08b092c561e`  
Predecessor: `v0.39.0`

## 1. Exact review identity

The reviewed object is the complete generated bundle. Certify the exact `SHA256SUMS` bytes and separately record its SHA-256. No artifact may be regenerated, reformatted, or copied through a newline-changing tool after certification.

The annotated-tag message is the exact bytes of `release-authority-v1.txt`. Its `review-set-sha256` binds the plan, evidence manifest, changelog input, Release body, and both asset ledgers without a circular self-hash. The candidate tree must contain every bundle file at `docs/releases/v0.40.0/` before tagging.

### review-set-v1 byte construction

The authoritative machine-readable recipe is `input-manifest.v1.json.contracts.reviewSet` with schema `prepare-agentscommander-release/review-set/v1`. Recompute it as follows; do not hardcode the candidate digest:

1. Read these basenames in this exact order, with no directory prefix:

```text
CHANGELOG.release.md
candidate-assets.v1.json
input-manifest.v1.json
predecessor-assets.v1.json
release-body.md
release-plan.md
```

2. Each file input is its exact emitted UTF-8 byte sequence. Perform no parsing, reserialization, Unicode normalization, or line-ending conversion. Text permits LF byte `0a` only, forbids a BOM and CR, and ends in exactly one LF. Generated JSON is already canonical: keys recursively sorted by ECMAScript UTF-16 string order, `JSON.stringify(value, null, 2)`, then one LF.
3. For each file, compute SHA-256 over those exact bytes and encode the digest as 64 lowercase hexadecimal ASCII bytes.
4. Serialize one record as `digest || 0x20 0x20 || UTF-8 basename || 0x0a`. There is no preamble, postamble, NUL, inter-record data, or length prefix. Concatenate records directly in the stated order; the last record retains its LF.
5. SHA-256 the complete concatenated record bytes and encode the result as 64 lowercase hexadecimal characters. That value must equal the sole `review-set-sha256` field in `release-authority-v1.txt`.

Positive conformance vector:

- `alpha.txt`: content base64 `YWxwaGEK`; SHA-256 `b6a98d9ce9a2d9149288fa3df42d377c3e42737afdcdaf714e33c0a100b51060`
- `beta.json`: content base64 `e30K`; SHA-256 `ca3d163bab055381827226140568f3bef7eaac187cebd76878e0b63e9e442356`

- exact order: `alpha.txt -> beta.json`
- serialized record bytes, base64: `YjZhOThkOWNlOWEyZDkxNDkyODhmYTNkZjQyZDM3N2MzZTQyNzM3YWZkY2RhZjcxNGUzM2MwYTEwMGI1MTA2MCAgYWxwaGEudHh0CmNhM2QxNjNiYWIwNTUzODE4MjcyMjYxNDA1NjhmM2JlZjdlYWFjMTg3Y2ViZDc2ODc4ZTBiNjNlOWU0NDIzNTYgIGJldGEuanNvbgo=`
- expected review-set SHA-256: `9d3945ec3d10e79abc60f28fd95e94cfb9d7f6b95ade412ea7bac4c138956ff8`

An implementation must first reproduce every vector byte and digest, then compute the candidate digest from candidate-tree files. A vector mismatch, different order, changed file byte, one-space separator, CRLF terminator, prefix, suffix, or normalization is fatal.

Generated evidence paths allowed in the hardening PR:

- `docs/releases/v0.40.0/CHANGELOG.release.md`
- `docs/releases/v0.40.0/candidate-assets.v1.json`
- `docs/releases/v0.40.0/input-manifest.v1.json`
- `docs/releases/v0.40.0/predecessor-assets.v1.json`
- `docs/releases/v0.40.0/release-authority-v1.txt`
- `docs/releases/v0.40.0/release-body.md`
- `docs/releases/v0.40.0/release-plan.md`
- `docs/releases/v0.40.0/SHA256SUMS`

The hardening allowlist is closed at the release workflow plus these evidence paths. No release-hardening plan file is part of the contract: AgentsCommander issue #2183 removed `plans/` from the repository and dropped the derived plan route with it. Any implementation record is governance content kept outside the repository, and this bundle never hashes or incorporates it.

## 2. Frozen read-only facts

- Git remote main and GitHub API agree at `db9d635e25448c4331094c942a7fb08b092c561e`.
- Ordered planning-base parents: [44fe21a9899516637a321cfbd0c21768d77e7093, 7ccf7a6c16d6e93ed3e4e5baf98444b6e2c28703].
- Base `.github/workflows/release.yml` blob: `ed64253db473dd0de792d6c214b2eb164652c93b`; content SHA-256: `5fc231d824614084cfaa37ec5567dc5d32eadc797c5e38a99643134a2e508aad`.
- Predecessor annotated object: `8d50773ea8f61ae4c7b7f1cb5556110aa7bef7cc`; peeled commit: `f0d6b96c31cd8ff6780c553c690d506d70fd825d`.
- Predecessor immutable GitHub Release id: `394964500`.
- npm latest: `0.39.0`; candidate tag, Release, and npm version are absent at both discovery snapshots.
- Ruleset/reviewer authority: rules require approval; `mblua` has the documented admin exception only for unavailable self-review after every check passes.
- Tag immutability/binding policy: the repository lacks a selected tag-protection guarantee, so every job/rerun and final verifier fail on any object/peeled movement.
- Release asset upload design: direct uploader jobs receive job-local contents: write and no id-token.
- Frozen GitHub CLI: `gh 2.101.0`, archive `gh_2.101.0_linux_amd64.tar.gz`, SHA-256 `9bca2d1c16825f109907a23307628a2f0698fbf99662b73a5cf0b020293072b8`.

Resolved action pins:

- `actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09` (discovered from `fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09`)
- `actions/setup-node@a0853c24544627f65ddf259abe73b1d18a591444` (discovered from `a0853c24544627f65ddf259abe73b1d18a591444`)
- `actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a` (discovered from `043fb46d1a93c77aae656e7c1c64a875d1fc6a0a`)
- `dtolnay/rust-toolchain@4360b52568e2003a75bf9bc1d59f33a8e3fc893c` (discovered from `4360b52568e2003a75bf9bc1d59f33a8e3fc893c`)
- `swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6` (discovered from `6323deb102c322ba6fcbdcafc7e3dddab59af2b6`)
- `tauri-apps/tauri-action@84b9d35b5fc46c1e45415bdb6144030364f7ebc5` (discovered from `84b9d35b5fc46c1e45415bdb6144030364f7ebc5`)

Current version surfaces to advance with the repository's version tool:

- `package.json` `/version`: `0.39.0` (blob `d97ba2498296ce7065d12053a4b98f49c2b4747f`)
- `package-lock.json` `/version`: `0.39.0` (blob `3de04d655dd98308b6ff13f526a5520e4265ff3a`)
- `package-lock.json` `/packages//version`: `0.39.0` (blob `3de04d655dd98308b6ff13f526a5520e4265ff3a`)
- `npm/package.json` `/version`: `0.39.0` (blob `30b5e3f9a6fa96e597c96a8343b5d2dcc69a3c2f`)
- `npm/install.js` `const VERSION`: `0.39.0` (blob `b88e92d6a0b5b912a435224c0d3973e566810e5b`)
- `src-tauri/Cargo.toml` `[package].version`: `0.39.0` (blob `654617d42c836139baf2069f33bfd54364d9feb6`)
- `Cargo.lock` `agentscommander.version`: `0.39.0` (blob `238a785fc0d2d2d8bdf3d8e8cd8eb91722c9fa9c`)
- `src-tauri/tauri.conf.json` `/version`: `0.39.0` (blob `53b5412c9d380cf335ef85e409d46f98490f7268`)

Approved scope:

- maintenance: ci(release): publish every release to npm dist-tag next, promote to latest by hand (#2029) ([#2029](https://github.com/mblua/AgentsCommander/issues/2029), [PR #2596](https://github.com/mblua/AgentsCommander/pull/2596))
- feature: feat(agent-help): embedded layer, schema v1 and shared resolver (#2136) ([#2136](https://github.com/mblua/AgentsCommander/issues/2136), [PR #2478](https://github.com/mblua/AgentsCommander/pull/2478))
- feature: feat(agent-help): r2 schema and local overlay (#2137) ([#2137](https://github.com/mblua/AgentsCommander/issues/2137), [PR #2479](https://github.com/mblua/AgentsCommander/pull/2479))
- feature: feat(agent-help): get_agent_help IPC command (#2138) ([#2138](https://github.com/mblua/AgentsCommander/issues/2138), [PR #2489](https://github.com/mblua/AgentsCommander/pull/2489))
- feature: feat(#2139): agent-help remote layer (fetch, throttle, publish v1 file) ([#2139](https://github.com/mblua/AgentsCommander/issues/2139), [PR #2502](https://github.com/mblua/AgentsCommander/pull/2502))
- feature: feat(agent-help): tips window and Best Practices button (#2143) ([#2143](https://github.com/mblua/AgentsCommander/issues/2143), [PR #2508](https://github.com/mblua/AgentsCommander/pull/2508))
- feature: feat(agent-help): validate remote agent-help layer (#2144) ([#2144](https://github.com/mblua/AgentsCommander/issues/2144), [PR #2492](https://github.com/mblua/AgentsCommander/pull/2492))
- feature: feat(agent-help): materialize the shipped agent-help template (#2145) ([#2145](https://github.com/mblua/AgentsCommander/issues/2145), [PR #2480](https://github.com/mblua/AgentsCommander/pull/2480))
- feature: feat(#2430): cross-platform stem rule for command_token_basename ([#2430](https://github.com/mblua/AgentsCommander/issues/2430), [PR #2533](https://github.com/mblua/AgentsCommander/pull/2533))
- feature: feat(identity): portable profile hash canonicalization v2 (#2431) ([#2431](https://github.com/mblua/AgentsCommander/issues/2431), [PR #2546](https://github.com/mblua/AgentsCommander/pull/2546))
- feature: feat(identity): persist coding-agent descriptor (#2433) ([#2433](https://github.com/mblua/AgentsCommander/issues/2433), [PR #2553](https://github.com/mblua/AgentsCommander/pull/2553))
- feature: feat(config): tiered coding-agent reference resolution (#2434) ([#2434](https://github.com/mblua/AgentsCommander/issues/2434), [PR #2574](https://github.com/mblua/AgentsCommander/pull/2574))
- feature: feat(sidebar): tier badge for name/command profile matches (#2435) ([#2435](https://github.com/mblua/AgentsCommander/issues/2435), [PR #2666](https://github.com/mblua/AgentsCommander/pull/2666))
- feature: feat(settings): rekey coding-agent profile map by stable agent identity (#2450) ([#2450](https://github.com/mblua/AgentsCommander/issues/2450), [PR #2559](https://github.com/mblua/AgentsCommander/pull/2559))
- feature: feat(session): match carrier on Session/SessionInfo (#2451) ([#2451](https://github.com/mblua/AgentsCommander/issues/2451), [PR #2560](https://github.com/mblua/AgentsCommander/pull/2560))
- fix: fix(sidebar): render the Co-managed message on the replica row (#2452) ([#2452](https://github.com/mblua/AgentsCommander/issues/2452), [PR #2458](https://github.com/mblua/AgentsCommander/pull/2458))
- fix: fix(sidebar): co-managed row no longer stuck busy after the armed idle edge (#2453) ([#2453](https://github.com/mblua/AgentsCommander/issues/2453), [PR #2472](https://github.com/mblua/AgentsCommander/pull/2472))
- fix: fix(#2454): pin the Claude reader's first attach to the minted transcript ([#2454](https://github.com/mblua/AgentsCommander/issues/2454), [PR #2477](https://github.com/mblua/AgentsCommander/pull/2477))
- fix: fix(#2455): co-managed logging (epic #2442 p4) ([#2455](https://github.com/mblua/AgentsCommander/issues/2455), [PR #2493](https://github.com/mblua/AgentsCommander/pull/2493))
- fix: fix(#2456): re-raise Room reader demand when readiness changes ([#2456](https://github.com/mblua/AgentsCommander/issues/2456), [PR #2515](https://github.com/mblua/AgentsCommander/pull/2515))
- fix: fix(#2462): regenerate src-tauri/module-arcs.txt to match the code ([#2462](https://github.com/mblua/AgentsCommander/issues/2462), [PR #2466](https://github.com/mblua/AgentsCommander/pull/2466))
- maintenance: ci: module-arcs.txt staleness gate (#2463) ([#2463](https://github.com/mblua/AgentsCommander/issues/2463), [PR #2471](https://github.com/mblua/AgentsCommander/pull/2471))
- feature: feat(ci): module cycle regression gate with committed baseline (#2464) ([#2464](https://github.com/mblua/AgentsCommander/issues/2464), [PR #2474](https://github.com/mblua/AgentsCommander/pull/2474))
- feature: feat(ci): classify module cycle changes in the cycle gate (#2465) ([#2465](https://github.com/mblua/AgentsCommander/issues/2465), [PR #2476](https://github.com/mblua/AgentsCommander/pull/2476))
- fix: fix(sidebar): make rail section headers readable (#2467) ([#2467](https://github.com/mblua/AgentsCommander/issues/2467), [PR #2468](https://github.com/mblua/AgentsCommander/pull/2468))
- feature: feat(cli): room activity filters, CI run ids/PRs/unknown reason, docs (#2473) ([#2473](https://github.com/mblua/AgentsCommander/issues/2473), [PR #2490](https://github.com/mblua/AgentsCommander/pull/2490))
- maintenance: refactor: file naming phase A, registry constants (#2481) ([#2481](https://github.com/mblua/AgentsCommander/issues/2481), [PR #2517](https://github.com/mblua/AgentsCommander/pull/2517))
- maintenance: test(selection): selection-lock timing and synthetic perf bench (#2483) ([#2483](https://github.com/mblua/AgentsCommander/issues/2483), [PR #2505](https://github.com/mblua/AgentsCommander/pull/2505))
- maintenance: refactor(picker): batch the post-load picker updates (#2484) ([#2484](https://github.com/mblua/AgentsCommander/issues/2484), [PR #2522](https://github.com/mblua/AgentsCommander/pull/2522))
- maintenance: refactor(selection): read-write selection turn for the three previews (#2485) ([#2485](https://github.com/mblua/AgentsCommander/issues/2485), [PR #2531](https://github.com/mblua/AgentsCommander/pull/2531))
- maintenance: refactor(config): single config read for selection state (#2486) ([#2486](https://github.com/mblua/AgentsCommander/issues/2486), [PR #2550](https://github.com/mblua/AgentsCommander/pull/2550))
- feature: feat(co-managed): global coManagedEnabled switch, off by default (refs #2491) ([#2491](https://github.com/mblua/AgentsCommander/issues/2491), [PR #2514](https://github.com/mblua/AgentsCommander/pull/2514))
- feature: feat(pty): weekly quota source engine (#2494, epic #2482 p1) ([#2494](https://github.com/mblua/AgentsCommander/issues/2494), [PR #2503](https://github.com/mblua/AgentsCommander/pull/2503))
- feature: feat(#2495): wire the agent quota engine to settings, sessions and IPC ([#2495](https://github.com/mblua/AgentsCommander/issues/2495), [PR #2506](https://github.com/mblua/AgentsCommander/pull/2506))
- feature: feat(sidebar): weekly-quota fill rule on the agent chip (#2496) ([#2496](https://github.com/mblua/AgentsCommander/issues/2496), [PR #2513](https://github.com/mblua/AgentsCommander/pull/2513))
- feature: feat(sidebar): weekly-quota plumbing (#2497, epic #2482 p4) ([#2497](https://github.com/mblua/AgentsCommander/issues/2497), [PR #2518](https://github.com/mblua/AgentsCommander/pull/2518))
- maintenance: #2482 p5a: weekly-quota shape, suggested pattern and predicate ([#2498](https://github.com/mblua/AgentsCommander/issues/2498), [PR #2521](https://github.com/mblua/AgentsCommander/pull/2521))
- feature: feat(settings): weekly quota pattern field per agent (#2499, #2482 p5b) ([#2499](https://github.com/mblua/AgentsCommander/issues/2499), [PR #2532](https://github.com/mblua/AgentsCommander/pull/2532))
- feature: feat(sidebar): render weekly-quota fill on the origin agent chip (#2500) ([#2500](https://github.com/mblua/AgentsCommander/issues/2500), [PR #2535](https://github.com/mblua/AgentsCommander/pull/2535))
- feature: feat(sidebar): fill the room-replica agent chip from the weekly quota (#2501, #2482 p7) ([#2501](https://github.com/mblua/AgentsCommander/issues/2501), [PR #2536](https://github.com/mblua/AgentsCommander/pull/2536))
- fix: fix(sidebar): stop menu-lock observer from throwing after jsdom teardown ([#2504](https://github.com/mblua/AgentsCommander/issues/2504), [PR #2541](https://github.com/mblua/AgentsCommander/pull/2541))
- feature: feat(settings): configurable room number mask, backend (#2509) ([#2509](https://github.com/mblua/AgentsCommander/issues/2509), [PR #2524](https://github.com/mblua/AgentsCommander/pull/2524))
- feature: feat(settings): room number mask control with live example (#2510) ([#2510](https://github.com/mblua/AgentsCommander/issues/2510), [PR #2530](https://github.com/mblua/AgentsCommander/pull/2530))
- docs: docs(#2511): document roomNumberMask setting ([#2511](https://github.com/mblua/AgentsCommander/issues/2511), [PR #2529](https://github.com/mblua/AgentsCommander/pull/2529))
- fix: fix(sidebar): agent chip light-theme contrast and stronger dark palette (refs #2512) ([#2512](https://github.com/mblua/AgentsCommander/issues/2512), [PR #2549](https://github.com/mblua/AgentsCommander/pull/2549))
- fix: fix(co-managed): close raise/disable race on Room reader demand (#2516) ([#2516](https://github.com/mblua/AgentsCommander/issues/2516), [PR #2534](https://github.com/mblua/AgentsCommander/pull/2534))
- fix: fix(sidebar): root banner toggle white in dark theme, compact row keeps height (refs #2519) ([#2519](https://github.com/mblua/AgentsCommander/issues/2519), [PR #2520](https://github.com/mblua/AgentsCommander/pull/2520))
- maintenance: test(sidebar): data-ac-testid on the Coding Agent session-menu item (#2523) ([#2523](https://github.com/mblua/AgentsCommander/issues/2523), [PR #2537](https://github.com/mblua/AgentsCommander/pull/2537))
- fix: fix(co-managed): release Room reader demands when co-managed is globally unavailable (#2525) ([#2525](https://github.com/mblua/AgentsCommander/issues/2525), [PR #2548](https://github.com/mblua/AgentsCommander/pull/2548))
- maintenance: chore(build): lower repo cargo jobs from 12 to 4 (#2526) ([#2526](https://github.com/mblua/AgentsCommander/issues/2526), [PR #2527](https://github.com/mblua/AgentsCommander/pull/2527))
- fix: fix(loops): delivered toast shows loop name, room number and next delivery (refs #2538) ([#2538](https://github.com/mblua/AgentsCommander/issues/2538), [PR #2540](https://github.com/mblua/AgentsCommander/pull/2540))
- feature: feat(settings): reorder_coding_agent command with target index (#2542, epic #2539 p1) ([#2542](https://github.com/mblua/AgentsCommander/issues/2542), [PR #2547](https://github.com/mblua/AgentsCommander/pull/2547))
- feature: feat(settings): reorder_coding_agent transport and DnD helpers (#2543) ([#2543](https://github.com/mblua/AgentsCommander/issues/2543), [PR #2552](https://github.com/mblua/AgentsCommander/pull/2552))
- feature: feat(settings): drag-and-drop reorder of Coding Agents (#2544) ([#2544](https://github.com/mblua/AgentsCommander/issues/2544), [PR #2554](https://github.com/mblua/AgentsCommander/pull/2554))
- feature: feat(settings): chevron expand selects the agent rail (#2545) ([#2545](https://github.com/mblua/AgentsCommander/issues/2545), [PR #2558](https://github.com/mblua/AgentsCommander/pull/2558))
- maintenance: test(ui-automation): anchor responder timing after CLI start (#2551) ([#2551](https://github.com/mblua/AgentsCommander/issues/2551), [PR #2575](https://github.com/mblua/AgentsCommander/pull/2575))
- feature: feat(picker): selection-lock panel counting texts (p7 of #2475) ([#2556](https://github.com/mblua/AgentsCommander/issues/2556), [PR #2564](https://github.com/mblua/AgentsCommander/pull/2564))
- feature: feat(#2557): typed scope faults in the selection-lock removal preview ([#2557](https://github.com/mblua/AgentsCommander/issues/2557), [PR #2588](https://github.com/mblua/AgentsCommander/pull/2588))
- maintenance: test(agent_command): gate Windows-path assertions on Linux (#2561) ([#2561](https://github.com/mblua/AgentsCommander/issues/2561), [PR #2562](https://github.com/mblua/AgentsCommander/pull/2562))
- fix: fix: close detached terminal windows when the app closes (#2563) ([#2563](https://github.com/mblua/AgentsCommander/issues/2563), [PR #2570](https://github.com/mblua/AgentsCommander/pull/2570))
- fix: fix(sidebar): stronger quota-used red tint on agent chip (#2565) ([#2565](https://github.com/mblua/AgentsCommander/issues/2565), [PR #2569](https://github.com/mblua/AgentsCommander/pull/2569))
- feature: feat(picker): name each replica the protection count skipped, and why (#2572) ([#2572](https://github.com/mblua/AgentsCommander/issues/2572), [PR #2656](https://github.com/mblua/AgentsCommander/pull/2656))
- fix: fix(ui): plain restart error text and toasts on silent restart paths (#2573) ([#2573](https://github.com/mblua/AgentsCommander/issues/2573), [PR #2606](https://github.com/mblua/AgentsCommander/pull/2606))
- feature: Agent picker: drag and drop reorder like Settings (#2577) ([#2577](https://github.com/mblua/AgentsCommander/issues/2577), [PR #2590](https://github.com/mblua/AgentsCommander/pull/2590))
- fix: fix(sidebar): align rail scrollbars and apply Noir thin style (#2578) ([#2578](https://github.com/mblua/AgentsCommander/issues/2578), [PR #2585](https://github.com/mblua/AgentsCommander/pull/2585))
- fix: fix(resource-monitor): warnings above agent rows, naming the agent (#2581) ([#2581](https://github.com/mblua/AgentsCommander/issues/2581), [PR #2593](https://github.com/mblua/AgentsCommander/pull/2593))
- feature: feat(resource-monitor): column header strip over agent rows (#2582) ([#2582](https://github.com/mblua/AgentsCommander/issues/2582), [PR #2669](https://github.com/mblua/AgentsCommander/pull/2669))
- fix: fix(resource-monitor): remove the socket attribution placeholder end to end (#2583) ([#2583](https://github.com/mblua/AgentsCommander/issues/2583), [PR #2667](https://github.com/mblua/AgentsCommander/pull/2667))
- fix: fix(sidebar): keep rail Favorites header from shrinking (#2584) ([#2584](https://github.com/mblua/AgentsCommander/issues/2584), [PR #2662](https://github.com/mblua/AgentsCommander/pull/2662))
- fix: fix(agents): resolve coding agents with the user PATH when launched from the desktop (#2589) ([#2589](https://github.com/mblua/AgentsCommander/issues/2589), [PR #2603](https://github.com/mblua/AgentsCommander/pull/2603))
- maintenance: refactor(quota): key the weekly reading by agent command (#2591) ([#2591](https://github.com/mblua/AgentsCommander/issues/2591), [PR #2595](https://github.com/mblua/AgentsCommander/pull/2595))
- maintenance: refactor(sidebar): read the shared per-agent quota reading (#2592) ([#2592](https://github.com/mblua/AgentsCommander/issues/2592), [PR #2604](https://github.com/mblua/AgentsCommander/pull/2604))
- maintenance: refactor(sidebar): share one agent drag-reorder lifecycle (#2594) ([#2594](https://github.com/mblua/AgentsCommander/issues/2594), [PR #2600](https://github.com/mblua/AgentsCommander/pull/2600))
- feature: feat(settings): profile params field wraps and expands on focus (#2597) ([#2597](https://github.com/mblua/AgentsCommander/issues/2597), [PR #2599](https://github.com/mblua/AgentsCommander/pull/2599))
- fix: fix(#2598): explicit sort comparator and modal-specific tests for Sonar S2871/S2187 ([#2598](https://github.com/mblua/AgentsCommander/issues/2598), [PR #2602](https://github.com/mblua/AgentsCommander/pull/2602))
- maintenance: refactor(sonar): harness no-op comments and store complexity (refs #2607) ([#2607](https://github.com/mblua/AgentsCommander/issues/2607), [PR #2610](https://github.com/mblua/AgentsCommander/pull/2610))
- maintenance: refactor(sonar): 5 findings, complexity + void operator (#2608) ([#2608](https://github.com/mblua/AgentsCommander/issues/2608), [PR #2609](https://github.com/mblua/AgentsCommander/pull/2609))
- maintenance: refactor: reduce cognitive complexity in 5 components (Sonar S3776) ([#2611](https://github.com/mblua/AgentsCommander/issues/2611), [PR #2612](https://github.com/mblua/AgentsCommander/pull/2612))
- maintenance: refactor(#2613): reduce cognitive complexity of 5 Sonar S3776 functions ([#2613](https://github.com/mblua/AgentsCommander/issues/2613), [PR #2614](https://github.com/mblua/AgentsCommander/pull/2614))
- maintenance: ci(#2615): run class-coverage guard on every PR ([#2615](https://github.com/mblua/AgentsCommander/issues/2615), [PR #2618](https://github.com/mblua/AgentsCommander/pull/2618))
- maintenance: style: remove 26 unstyled class tokens, keep 8 test anchors (#2616) ([#2616](https://github.com/mblua/AgentsCommander/issues/2616), [PR #2626](https://github.com/mblua/AgentsCommander/pull/2626))
- maintenance: refactor: reduce Sonar complexity/nesting (#2617) ([#2617](https://github.com/mblua/AgentsCommander/issues/2617), [PR #2619](https://github.com/mblua/AgentsCommander/pull/2619))
- maintenance: refactor: reduce cognitive complexity of 5 Sonar S3776 functions (#2620) ([#2620](https://github.com/mblua/AgentsCommander/issues/2620), [PR #2621](https://github.com/mblua/AgentsCommander/pull/2621))
- maintenance: refactor: reduce cognitive complexity in 4 Sonar S3776 findings (#2622) ([#2622](https://github.com/mblua/AgentsCommander/issues/2622), [PR #2623](https://github.com/mblua/AgentsCommander/pull/2623))
- maintenance: refactor: reduce cognitive complexity in skills checker (#2624) ([#2624](https://github.com/mblua/AgentsCommander/issues/2624), [PR #2625](https://github.com/mblua/AgentsCommander/pull/2625))
- maintenance: refactor(scripts): reduce S3776 complexity in 01-skills-checker.mjs (#2627) ([#2627](https://github.com/mblua/AgentsCommander/issues/2627), [PR #2629](https://github.com/mblua/AgentsCommander/pull/2629))
- maintenance: refactor: 5 Sonar S3776 complexity findings (#2628) ([#2628](https://github.com/mblua/AgentsCommander/issues/2628), [PR #2630](https://github.com/mblua/AgentsCommander/pull/2630))
- maintenance: refactor(scripts): reduce S3776 complexity in room-rename-allowlist and parseKeyValueLine (#2631) ([#2631](https://github.com/mblua/AgentsCommander/issues/2631), [PR #2633](https://github.com/mblua/AgentsCommander/pull/2633))
- maintenance: refactor: 5 Sonar findings S3776/S2004/S4123 (#2632) ([#2632](https://github.com/mblua/AgentsCommander/issues/2632), [PR #2634](https://github.com/mblua/AgentsCommander/pull/2634))
- maintenance: refactor: Sonar batch 7 — S3776 runPulse/classifyRust/main/AgentPickerModal, S3735 watchdog void (#2635) ([#2635](https://github.com/mblua/AgentsCommander/issues/2635), [PR #2638](https://github.com/mblua/AgentsCommander/pull/2638))
- maintenance: refactor: 5 Sonar S3776 findings (#2636) ([#2636](https://github.com/mblua/AgentsCommander/issues/2636), [PR #2637](https://github.com/mblua/AgentsCommander/pull/2637))
- maintenance: refactor: sonar batch 12 — S3776 in agent-order prototype, QuitConfirmModal, spec-board App ([#2639](https://github.com/mblua/AgentsCommander/issues/2639), [PR #2640](https://github.com/mblua/AgentsCommander/pull/2640))
- maintenance: refactor: Sonar batch 8 complexity fixes (#2641) ([#2641](https://github.com/mblua/AgentsCommander/issues/2641), [PR #2642](https://github.com/mblua/AgentsCommander/pull/2642))
- security: fix(ci): add --ignore-scripts to 5 npm installs (Sonar S6505) ([#2645](https://github.com/mblua/AgentsCommander/issues/2645), [PR #2647](https://github.com/mblua/AgentsCommander/pull/2647))
- security: fix(web): validate remote token and window type, crypto jitter (Sonar S8475/S8480/S2245/S6505) ([#2646](https://github.com/mblua/AgentsCommander/issues/2646), [PR #2648](https://github.com/mblua/AgentsCommander/pull/2648))
- security: fix(ci): add --ignore-scripts to 5 more npm installs (Sonar S6505, batch 3) ([#2649](https://github.com/mblua/AgentsCommander/issues/2649), [PR #2650](https://github.com/mblua/AgentsCommander/pull/2650))
- security: fix: docker ignore-scripts, job-level permissions, log escape (Sonar S6505/S8264/S5145) ([#2651](https://github.com/mblua/AgentsCommander/issues/2651), [PR #2652](https://github.com/mblua/AgentsCommander/pull/2652))
- security: fix(docker): run session-bridge final stage as nobody (Sonar S6471) ([#2653](https://github.com/mblua/AgentsCommander/issues/2653), [PR #2654](https://github.com/mblua/AgentsCommander/pull/2654))
- fix: sidebar: Escape closes 2 backdrops like a backdrop click (S1082, #2655 phase D) ([#2660](https://github.com/mblua/AgentsCommander/issues/2660), [PR #2664](https://github.com/mblua/AgentsCommander/pull/2664))

## 3. One mandatory order

Perform these stages exactly once and in this order:

1. Freeze the exact evidence and generated bundle.
2. Obtain WG33 cold-plan `PASS` on the exact `SHA256SUMS` identity.
3. Certify the exact bytes and record the `SHA256SUMS` SHA-256.
4. Obtain explicit human approval naming that exact hash.
5. Purge the planning context required by the implementation workflow.
6. Cold-implement the generic release hardening and exact evidence bundle; independently review the exact PR head; merge it through the protected branch.
7. Create the version/evidence PR from that exact hardening merge, move the approved Unreleased changelog bytes, run the repository version tool, independently review the exact PR head, and merge it through the protected branch.
8. Run final authority, topology, package, workflow, collision, and remote-main gates.
9. Create and push one annotated tag with the exact `release-authority-v1.txt` payload.
10. Verify the GitHub workflow, immutable public Release, npm package/provenance/install, and destination executable.

No implementation, PR, merge, tag, or publication occurs before steps 1-5. There is no second hardening or version merge later in the sequence.

## 4. Planning-base gate

Before implementation, independently query remote main through raw Git and the GitHub ref API. Both must equal `db9d635e25448c4331094c942a7fb08b092c561e`; the GitHub commit API must return the ordered parent list [44fe21a9899516637a321cfbd0c21768d77e7093, 7ccf7a6c16d6e93ed3e4e5baf98444b6e2c28703]; the contents API must return workflow blob `ed64253db473dd0de792d6c214b2eb164652c93b`. Any mismatch is `FROZEN_INPUT_CHANGED`: discard the bundle and run the generator again from a new config/output path.

This gate is not reused after required merges. Later gates bind `FINAL_CANDIDATE_MAIN` instead.

## 5. Hardening PR contract

Branch from the exact planning base. The exact changed-path allowlist is:

```text
.github/workflows/release.yml
docs/releases/v0.40.0/CHANGELOG.release.md
docs/releases/v0.40.0/candidate-assets.v1.json
docs/releases/v0.40.0/input-manifest.v1.json
docs/releases/v0.40.0/predecessor-assets.v1.json
docs/releases/v0.40.0/release-authority-v1.txt
docs/releases/v0.40.0/release-body.md
docs/releases/v0.40.0/release-plan.md
docs/releases/v0.40.0/SHA256SUMS
```

The allowlist is closed: no wildcard and no other changed path is permitted. Its final entry is the canonical plan path derived solely from the release issue number and candidate version; it is not a bundle artifact. The bundle files must be copied byte-for-byte from the reviewed output and verified against `SHA256SUMS` on the PR head. The workflow implementation must be generic for every normal `vX.Y.Z`; candidate-specific values live only in `docs/releases/$TAG/`.

For the selected merge method, record `HARDENING_HEAD` only after exact-head review. After protected merge, require a two-parent merge commit and assert the ordered parent vector is exactly:

```text
[db9d635e25448c4331094c942a7fb08b092c561e, HARDENING_HEAD]
```

Set `HARDENING_MERGE_SHA` to that merge commit. A squash, rebase, reversed parent order, extra parent, or intervening main commit invalidates this bundle.

### Generic workflow bootstrap in every job

Every job checks out the pushed tag with full history before useful work. Its first executable step must implement assertions equivalent to:

```bash
set -euo pipefail
test "$GITHUB_REF_TYPE" = "tag"
test "$GITHUB_REF_NAME" = "${GITHUB_REF#refs/tags/}"
TAG="$GITHUB_REF_NAME"
[[ "$TAG" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]
HEAD_COMMIT="$(git rev-parse 'HEAD^{commit}')"
PEELED_COMMIT="$(git rev-parse "$TAG^{}")"
TAG_OBJECT="$(git rev-parse "$TAG^{tag}")"
test "$GITHUB_SHA" = "${{ github.sha }}"
test "$GITHUB_SHA" = "$HEAD_COMMIT"
test "$HEAD_COMMIT" = "$PEELED_COMMIT"
test "$TAG_OBJECT" != "$PEELED_COMMIT"
mapfile -t REMOTE_TAG < <(git ls-remote origin "refs/tags/$TAG" "refs/tags/$TAG^{}")
test "${#REMOTE_TAG[@]}" -eq 2
printf '%s\n' "${REMOTE_TAG[@]}" | grep -Fx "$TAG_OBJECT"$'\t'"refs/tags/$TAG"
printf '%s\n' "${REMOTE_TAG[@]}" | grep -Fx "$PEELED_COMMIT"$'\t'"refs/tags/$TAG^{}"
```

`GITHUB_SHA`, `${{ github.sha }}`, `HEAD`, and the peeled commit are one identity. The annotated object is a second identity. `guard` exports both; every later job compares its local and remote pair to those immutable outputs. Printing the pair without assertions is forbidden.

The strict `release-authority-v1` parser must reject reordered, missing, extra, duplicate, unknown, or malformed fields; recompute the review-set digest from candidate-tree files; and compare every base/predecessor/source/policy value with the manifest.

### Tag-state semantics

- Before tag creation only: candidate absence is required across remote Git, GitHub Releases, and npm.
- In the tag workflow and every rerun: absence is failure. Exactly one existing annotated-object record and one peeled record must equal the guard pair and canonical payload.
- A different object on the same peeled commit is a moved tag and fails.

No claim of “no bypass” or immutable ref is allowed. Bind the tag equivalently by exporting the exact annotated-object/peeled pair from guard, re-querying and asserting it before useful work in every job/rerun, and repeating it after workflow completion. Any movement invalidates the run and requires a new version if publication occurred.

### Release construction and permissions

Keep this order:

```text
guard -> platform producers -> checksums -> publish-github -> publish-npm
```

`publish-github` makes the Release public only after the exact 17-entry candidate ledger, SHA-256 file, downloadable required assets, and attestations verify. `publish-npm` runs only after the GitHub Release is public and immutable. Use string-valued `make_latest: 'true'`.

`build`, `checksums`, and `publish-github` each receive job-local `contents: write` because they directly upload or mutate Release assets/state; none receives `id-token`.

Only `publish-npm` receives `id-token: write`; it receives `contents: read`, uses `actions/setup-node` at an immutable commit with `node-version: 22` and `registry-url: https://registry.npmjs.org`, installs exact npm `11.6.2`, asserts both versions, and publishes with provenance. It must fail if `NODE_AUTH_TOKEN`, `NPM_TOKEN`, or the legacy package token is present. No other job receives OIDC.

Jobs that run GitHub attestation verification receive `attestations: read` and explicit `GH_TOKEN: ${{ github.token }}`. Before either attestation path, download only `gh_2.101.0_linux_amd64.tar.gz`, verify SHA-256 `9bca2d1c16825f109907a23307628a2f0698fbf99662b73a5cf0b020293072b8`, install from that verified archive, and require `gh version 2.101.0` before use.

Each macOS architecture is built once. The raw binary, app archive, and installer for that architecture must derive from that one build and one recorded digest manifest; no second macOS build may generate a competing asset.

Derive the immediate predecessor dynamically by strict normal-SemVer comparison, then require its exact annotated object, peeled commit, immutable public Release, and asset ledger. Do not use mutable `latest` alone as predecessor authority.

### Changelog and Release body

Replace the exact current `Unreleased` body once: remove it from `Unreleased` and insert it under `## 0.40.0`. The expected complete result is `CHANGELOG.release.md`; use `release-body.md` verbatim for the GitHub Release. Duplicate or residual copies fail.

Every version-side scope/changelog declaration below is mandatory; a missing scope issue, summary substring, changelog substring, or whole-word alternative fails closed:

- No release-specific scope/changelog semantic assertions are declared.

### Package and asset gates

Run the repository version tool for `0.40.0`; do not hand-edit version surfaces. Require all eight parsed surfaces to equal the candidate on the version PR head. Run `npm pack --dry-run`, create the tarball, inspect it, and smoke-install from that exact tarball before tagging.

Predecessor ledger: exactly 17 unique uploaded nonempty assets. Candidate ledger: exactly 17 unique names derived from predecessor version substitution plus exactly these declared candidate-only assets:

- No candidate-only assets are declared.

Any missing, extra, duplicate, zero-size, digest mismatch, producer mismatch, or name mismatch fails.

## 6. Version/evidence PR and exact topology

Branch from exactly `HARDENING_MERGE_SHA`. The exact changed-path allowlist is:

```text
CHANGELOG.md
package.json
package-lock.json
npm/package.json
npm/install.js
src-tauri/Cargo.toml
Cargo.lock
src-tauri/tauri.conf.json
```

The root `CHANGELOG.md` must equal generated `CHANGELOG.release.md`. All generated evidence already committed by the hardening PR remains byte-identical. Record `VERSION_HEAD` only after exact-head review and every required check/version-sync/package gate passes.

After protected merge, require a two-parent merge commit and assert the ordered parent vector is exactly:

```text
[HARDENING_MERGE_SHA, VERSION_HEAD]
```

Set `VERSION_MERGE_SHA` and `FINAL_CANDIDATE_MAIN` to that commit. Ancestry alone is insufficient.

The repository currently has no second eligible identity. After every required check and version-sync passes on the exact head, `mblua` may use the documented admin merge path solely to satisfy unavailable self-review. Admin authority must never override a failed, missing, pending, or stale check.

## 7. Final pre-tag gates

Immediately before tagging, all of these must pass against fresh remote/API state:

- remote main and GitHub default-branch ref both equal `FINAL_CANDIDATE_MAIN`, not the old planning base;
- exact hardening and version ordered parent vectors match section 5 and 6;
- the workflow and generated bundle exist at the final candidate with exact reviewed hashes;
- workflow parser/permissions/action/CLI/npm/attestation fixtures pass;
- every version/package/changelog/asset contract passes;
- candidate tag, Release, and npm version remain unambiguously absent;
- required status checks passed on each exact reviewed PR head;
- review authority matches the documented policy, with no unmodeled bypass.

Any failure spends no version: stop without creating a tag.

## 8. Annotated tag and executable post-push proof

After explicit authorization, create one annotated tag at `FINAL_CANDIDATE_MAIN` using the exact `docs/releases/v0.40.0/release-authority-v1.txt` bytes. Before push, require the local tag object type to be `tag`, its payload to match byte-for-byte, and its peeled commit to equal `FINAL_CANDIDATE_MAIN`.

After push, resolve the two remote records and execute exact assertions against the local annotated object and peeled commit. Do not merely display them. A missing record, extra record, changed object, or changed peeled commit stops all later claims.

The tag workflow then applies the every-job bootstrap, reuses only the exact draft for the exact tag on rerun, uploads/verifies the exact ledger, makes the Release immutable and public before npm, publishes npm once with provenance, and performs a clean install from the registry.

## 9. Recovery and completion

- Before GitHub publication, a failed run may resume only with the exact same annotated-object/peeled pair, payload, final candidate, draft id, and asset digests.
- After an immutable public Release exists, never delete, replace, or recreate it. If npm is absent, resume only the npm job from the exact verified Release and package tarball.
- If npm already contains the version, require exact repository/tag/provenance/tarball identity; otherwise stop for a new version.
- Never use `--clobber` to hide a digest mismatch. Idempotent overwrite is allowed only when the existing asset already belongs to the same guarded draft and the replacement digest is the exact expected digest.

Completion requires all 17 candidate assets and checksums, a public immutable GitHub Release, verified attestations, npm dist-tag/version/provenance/signature, clean npm install, and the destination executable reporting `0.40.0`. WG23 independently repeats GitHub/npm/executable verification before declaring the release complete.

## 10. Required negative suite

The workflow and preparation generator must reject: event SHA equated to tag object; missing candidate-tree evidence; a missing, changed, wildcarded, duplicated, or extra canonical-plan allowlist route; canonical-plan entry into `review-set-v1`, `SHA256SUMS`, or the bundle archive; a caller-supplied canonical-plan config key; reordered/duplicated execution; stale planning-base final gate; ancestry-only topology; absent/moved/lightweight/duplicate tag records; post-tag absence semantics; insufficient uploader permission or excess OIDC; duplicated/weakened changelog; mutable/mismatched GitHub CLI; missing npm registry/version/token gates; unsatisfied reviewer authority; malformed manifests/payloads; changed main; candidate collisions; ambiguous GitHub/npm errors; duplicate/missing/extra assets or facts; path traversal; output escape; caller-controlled CLI fixture injection; credential material in any input/evidence/error/artifact; and wrong `review-set-v1` order, byte, separator, terminator, or length-prefix semantics.
