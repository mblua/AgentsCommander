# Release preparation plan: v0.37.0

Status: `REVIEW_REQUIRED`. This generated bundle is not approval to implement, tag, publish, release, or deploy.

Repository: `mblua/AgentsCommander`  
Release issue: https://github.com/mblua/AgentsCommander/issues/2331  
Candidate: `0.37.0` / `v0.37.0`  
Planning base: `90bf6bb01b2a58e57525139cc903da0ba5ca7004`  
Predecessor: `v0.36.0`

## 1. Exact review identity

The reviewed object is the complete generated bundle. Certify the exact `SHA256SUMS` bytes and separately record its SHA-256. No artifact may be regenerated, reformatted, or copied through a newline-changing tool after certification.

The annotated-tag message is the exact bytes of `release-authority-v1.txt`. Its `review-set-sha256` binds the plan, evidence manifest, changelog input, Release body, and both asset ledgers without a circular self-hash. The candidate tree must contain every bundle file at `docs/releases/v0.37.0/` before tagging.

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

- `docs/releases/v0.37.0/CHANGELOG.release.md`
- `docs/releases/v0.37.0/candidate-assets.v1.json`
- `docs/releases/v0.37.0/input-manifest.v1.json`
- `docs/releases/v0.37.0/predecessor-assets.v1.json`
- `docs/releases/v0.37.0/release-authority-v1.txt`
- `docs/releases/v0.37.0/release-body.md`
- `docs/releases/v0.37.0/release-plan.md`
- `docs/releases/v0.37.0/SHA256SUMS`

The hardening allowlist is closed at the release workflow plus these evidence paths. No release-hardening plan file is part of the contract: AgentsCommander issue #2183 removed `plans/` from the repository and dropped the derived plan route with it. Any implementation record is governance content kept outside the repository, and this bundle never hashes or incorporates it.

## 2. Frozen read-only facts

- Git remote main and GitHub API agree at `90bf6bb01b2a58e57525139cc903da0ba5ca7004`.
- Ordered planning-base parents: [63ceae2765e1c63760561e0be77bece777d3fe9b, b846da1ccc5e9de2bd95b166eeb7315b5f98675c].
- Base `.github/workflows/release.yml` blob: `1813662691aea512b4c8c087a388b0e9e87e66fa`; content SHA-256: `8c032478e665a51f17de2c061e793dacafe3351312fa2c0eadc7a8103a6e04db`.
- Predecessor annotated object: `52aca156ca758faa658d05fe9f2d4957d6c7a99e`; peeled commit: `4007c00793b66ac2daef11fe7bf5a119b8131d4b`.
- Predecessor immutable GitHub Release id: `391490080`.
- npm latest: `0.36.0`; candidate tag, Release, and npm version are absent at both discovery snapshots.
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

- `package.json` `/version`: `0.36.0` (blob `7bb68d49c9acfb0cb0a5551b0996c71a9c6e059c`)
- `package-lock.json` `/version`: `0.36.0` (blob `8102f8c8f64e53b9ce2e65ec56a7395cd9e8c8f0`)
- `package-lock.json` `/packages//version`: `0.36.0` (blob `8102f8c8f64e53b9ce2e65ec56a7395cd9e8c8f0`)
- `npm/package.json` `/version`: `0.36.0` (blob `ab5a7c87ec663344b2119b21192453ae07d452ba`)
- `npm/install.js` `const VERSION`: `0.36.0` (blob `adf1c02bb103aefadef0d4daa876115d46cac0c3`)
- `src-tauri/Cargo.toml` `[package].version`: `0.36.0` (blob `90cd0fe74947ee049b8bac5dab5a33dc5f136ae8`)
- `Cargo.lock` `agentscommander.version`: `0.36.0` (blob `0e895e7c732a11af3af2dbd300e5ffc86ebfd3ac`)
- `src-tauri/tauri.conf.json` `/version`: `0.36.0` (blob `9523111791eed0c0ef9c10760805deeb0e1723f8`)

Approved scope:

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
- maintenance: test: serialize copied CLI copy and spawn in phase A (refs #2322) ([#2322](https://github.com/mblua/AgentsCommander/issues/2322), [PR #2324](https://github.com/mblua/AgentsCommander/pull/2324))

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

Before implementation, independently query remote main through raw Git and the GitHub ref API. Both must equal `90bf6bb01b2a58e57525139cc903da0ba5ca7004`; the GitHub commit API must return the ordered parent list [63ceae2765e1c63760561e0be77bece777d3fe9b, b846da1ccc5e9de2bd95b166eeb7315b5f98675c]; the contents API must return workflow blob `1813662691aea512b4c8c087a388b0e9e87e66fa`. Any mismatch is `FROZEN_INPUT_CHANGED`: discard the bundle and run the generator again from a new config/output path.

This gate is not reused after required merges. Later gates bind `FINAL_CANDIDATE_MAIN` instead.

## 5. Hardening PR contract

Branch from the exact planning base. The exact changed-path allowlist is:

```text
.github/workflows/release.yml
docs/releases/v0.37.0/CHANGELOG.release.md
docs/releases/v0.37.0/candidate-assets.v1.json
docs/releases/v0.37.0/input-manifest.v1.json
docs/releases/v0.37.0/predecessor-assets.v1.json
docs/releases/v0.37.0/release-authority-v1.txt
docs/releases/v0.37.0/release-body.md
docs/releases/v0.37.0/release-plan.md
docs/releases/v0.37.0/SHA256SUMS
```

The allowlist is closed: no wildcard and no other changed path is permitted. Its final entry is the canonical plan path derived solely from the release issue number and candidate version; it is not a bundle artifact. The bundle files must be copied byte-for-byte from the reviewed output and verified against `SHA256SUMS` on the PR head. The workflow implementation must be generic for every normal `vX.Y.Z`; candidate-specific values live only in `docs/releases/$TAG/`.

For the selected merge method, record `HARDENING_HEAD` only after exact-head review. After protected merge, require a two-parent merge commit and assert the ordered parent vector is exactly:

```text
[90bf6bb01b2a58e57525139cc903da0ba5ca7004, HARDENING_HEAD]
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

Replace the exact current `Unreleased` body once: remove it from `Unreleased` and insert it under `## 0.37.0`. The expected complete result is `CHANGELOG.release.md`; use `release-body.md` verbatim for the GitHub Release. Duplicate or residual copies fail.

Every version-side scope/changelog declaration below is mandatory; a missing scope issue, summary substring, changelog substring, or whole-word alternative fails closed:

- No release-specific scope/changelog semantic assertions are declared.

### Package and asset gates

Run the repository version tool for `0.37.0`; do not hand-edit version surfaces. Require all eight parsed surfaces to equal the candidate on the version PR head. Run `npm pack --dry-run`, create the tarball, inspect it, and smoke-install from that exact tarball before tagging.

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

After explicit authorization, create one annotated tag at `FINAL_CANDIDATE_MAIN` using the exact `docs/releases/v0.37.0/release-authority-v1.txt` bytes. Before push, require the local tag object type to be `tag`, its payload to match byte-for-byte, and its peeled commit to equal `FINAL_CANDIDATE_MAIN`.

After push, resolve the two remote records and execute exact assertions against the local annotated object and peeled commit. Do not merely display them. A missing record, extra record, changed object, or changed peeled commit stops all later claims.

The tag workflow then applies the every-job bootstrap, reuses only the exact draft for the exact tag on rerun, uploads/verifies the exact ledger, makes the Release immutable and public before npm, publishes npm once with provenance, and performs a clean install from the registry.

## 9. Recovery and completion

- Before GitHub publication, a failed run may resume only with the exact same annotated-object/peeled pair, payload, final candidate, draft id, and asset digests.
- After an immutable public Release exists, never delete, replace, or recreate it. If npm is absent, resume only the npm job from the exact verified Release and package tarball.
- If npm already contains the version, require exact repository/tag/provenance/tarball identity; otherwise stop for a new version.
- Never use `--clobber` to hide a digest mismatch. Idempotent overwrite is allowed only when the existing asset already belongs to the same guarded draft and the replacement digest is the exact expected digest.

Completion requires all 17 candidate assets and checksums, a public immutable GitHub Release, verified attestations, npm dist-tag/version/provenance/signature, clean npm install, and the destination executable reporting `0.37.0`. WG23 independently repeats GitHub/npm/executable verification before declaring the release complete.

## 10. Required negative suite

The workflow and preparation generator must reject: event SHA equated to tag object; missing candidate-tree evidence; a missing, changed, wildcarded, duplicated, or extra canonical-plan allowlist route; canonical-plan entry into `review-set-v1`, `SHA256SUMS`, or the bundle archive; a caller-supplied canonical-plan config key; reordered/duplicated execution; stale planning-base final gate; ancestry-only topology; absent/moved/lightweight/duplicate tag records; post-tag absence semantics; insufficient uploader permission or excess OIDC; duplicated/weakened changelog; mutable/mismatched GitHub CLI; missing npm registry/version/token gates; unsatisfied reviewer authority; malformed manifests/payloads; changed main; candidate collisions; ambiguous GitHub/npm errors; duplicate/missing/extra assets or facts; path traversal; output escape; caller-controlled CLI fixture injection; credential material in any input/evidence/error/artifact; and wrong `review-set-v1` order, byte, separator, terminator, or length-prefix semantics.
