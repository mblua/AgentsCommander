# Release preparation plan: v0.46.0

Status: `REVIEW_REQUIRED`. This generated bundle is not approval to implement, tag, publish, release, or deploy.

Repository: `mblua/AgentsCommander`  
Release issue: https://github.com/mblua/AgentsCommander/issues/3605  
Candidate: `0.46.0` / `v0.46.0`  
Planning base: `59812e1c631e971cee0e85c5ad62a6affa30001f`  
Predecessor: `v0.45.1`

## 1. Exact review identity

The reviewed object is the complete generated bundle. Certify the exact `SHA256SUMS` bytes and separately record its SHA-256. No artifact may be regenerated, reformatted, or copied through a newline-changing tool after certification.

The annotated-tag message is the exact bytes of `release-authority-v1.txt`. Its `review-set-sha256` binds the plan, evidence manifest, changelog input, Release body, and both asset ledgers without a circular self-hash. The candidate tree must contain every bundle file at `docs/releases/v0.46.0/` before tagging.

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

- `docs/releases/v0.46.0/CHANGELOG.release.md`
- `docs/releases/v0.46.0/candidate-assets.v1.json`
- `docs/releases/v0.46.0/input-manifest.v1.json`
- `docs/releases/v0.46.0/predecessor-assets.v1.json`
- `docs/releases/v0.46.0/release-authority-v1.txt`
- `docs/releases/v0.46.0/release-body.md`
- `docs/releases/v0.46.0/release-plan.md`
- `docs/releases/v0.46.0/SHA256SUMS`

The hardening allowlist is closed at the release workflow plus these evidence paths. No release-hardening plan file is part of the contract: AgentsCommander issue #2183 removed `plans/` from the repository and dropped the derived plan route with it. Any implementation record is governance content kept outside the repository, and this bundle never hashes or incorporates it.

## 2. Frozen read-only facts

- Git remote main and GitHub API agree at `59812e1c631e971cee0e85c5ad62a6affa30001f`.
- Ordered planning-base parents: [3e64aaf4c89133ab2ca2b4a4873f628c60a690e1, edbcf6b4aef40434de74470f0e29c3bf1d7207c5].
- Base `.github/workflows/release.yml` blob: `c616f0303d85368a751e1c419ed32b5a091a5203`; content SHA-256: `1e05d3de55672ed61855c5477fe0ec827887f864a5792558acaf6ac9d411de86`.
- Predecessor annotated object: `3f4537cc527d40ac3ebb04c3a7239d0cd5a33f55`; peeled commit: `29fad1be70cc730407ebdd41c9690d1ccb8a5cf7`.
- Predecessor immutable GitHub Release id: `416102038`.
- npm latest: `0.43.0`; candidate tag, Release, and npm version are absent at both discovery snapshots.
- Ruleset/reviewer authority: rules require approval; `mblua` has the documented admin exception only for unavailable self-review after every check passes.
- Tag immutability/binding policy: the repository lacks a selected tag-protection guarantee, so every job/rerun and final verifier fail on any object/peeled movement.
- Release asset upload design: direct uploader jobs receive job-local contents: write and no id-token.
- Frozen GitHub CLI: `gh 2.103.3`, archive `gh_2.103.3_linux_amd64.tar.gz`, SHA-256 `4c806978ffce1cec70853302a56d70ac6da4f19bae7adca4e1a8ab00633dd91d`.

Resolved action pins:

- `actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09` (discovered from `fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09`)
- `actions/setup-node@a0853c24544627f65ddf259abe73b1d18a591444` (discovered from `a0853c24544627f65ddf259abe73b1d18a591444`)
- `actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a` (discovered from `043fb46d1a93c77aae656e7c1c64a875d1fc6a0a`)
- `dtolnay/rust-toolchain@4360b52568e2003a75bf9bc1d59f33a8e3fc893c` (discovered from `4360b52568e2003a75bf9bc1d59f33a8e3fc893c`)
- `swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6` (discovered from `6323deb102c322ba6fcbdcafc7e3dddab59af2b6`)
- `tauri-apps/tauri-action@84b9d35b5fc46c1e45415bdb6144030364f7ebc5` (discovered from `84b9d35b5fc46c1e45415bdb6144030364f7ebc5`)

Current version surfaces to advance with the repository's version tool:

- `package.json` `/version`: `0.45.1` (blob `155a87f77d1965513e945a921b0eb28e7acbc256`)
- `package-lock.json` `/version`: `0.45.1` (blob `5b3b8857425392ce2db309f99d49a3a553dac64e`)
- `package-lock.json` `/packages//version`: `0.45.1` (blob `5b3b8857425392ce2db309f99d49a3a553dac64e`)
- `npm/package.json` `/version`: `0.45.1` (blob `48ecd8ab5cdae87f7cce942bc5f8f109bae74dd0`)
- `npm/install.js` `const VERSION`: `0.45.1` (blob `03292d08ba0455bd6050b22af8890c9ea4ece0df`)
- `src-tauri/Cargo.toml` `[package].version`: `0.45.1` (blob `119203c64714aa24b2bf34d40dcdab976038b04d`)
- `Cargo.lock` `agentscommander.version`: `0.45.1` (blob `b384f3ff6612af83a950d9a7722b595ac7b77840`)
- `src-tauri/tauri.conf.json` `/version`: `0.45.1` (blob `6830329663f097151f3ebf296744d57032def7a6`)

Approved scope:

- feature: feat(cli): send Telegram text without attachments (refs #518) ([#518](https://github.com/mblua/AgentsCommander/issues/518), [PR #3104](https://github.com/mblua/AgentsCommander/pull/3104))
- fix: fix: minimize SELF-HANDOFF resume policy (closes #1555) ([#1555](https://github.com/mblua/AgentsCommander/issues/1555), [PR #3369](https://github.com/mblua/AgentsCommander/pull/3369))
- fix: fix(config-seed): prioritize Matrix templates over project defaults (refs #2808) ([#2808](https://github.com/mblua/AgentsCommander/issues/2808), [PR #3143](https://github.com/mblua/AgentsCommander/pull/3143))
- feature: feat: add guarded response-close lifecycle and resume (refs #2854) ([#2854](https://github.com/mblua/AgentsCommander/issues/2854), [PR #3138](https://github.com/mblua/AgentsCommander/pull/3138))
- feature: feat: activate Settings-controlled response close (refs #2855) ([#2855](https://github.com/mblua/AgentsCommander/issues/2855), [PR #3165](https://github.com/mblua/AgentsCommander/pull/3165))
- maintenance: feat(config): preserve source references in persistence carriers (refs #2900) ([#2900](https://github.com/mblua/AgentsCommander/issues/2900), [PR #3144](https://github.com/mblua/AgentsCommander/pull/3144))
- maintenance: feat(profiles): bind profiles by source-local BASE hash (refs #2902) ([#2902](https://github.com/mblua/AgentsCommander/issues/2902), [PR #3117](https://github.com/mblua/AgentsCommander/pull/3117))
- maintenance: feat(config): add physical agent-file source APIs (refs #2905) ([#2905](https://github.com/mblua/AgentsCommander/issues/2905), [PR #3141](https://github.com/mblua/AgentsCommander/pull/3141))
- maintenance: chore(profiles): preserve pair identity metadata in TypeScript (refs #3052) ([#3052](https://github.com/mblua/AgentsCommander/issues/3052), [PR #3089](https://github.com/mblua/AgentsCommander/pull/3089))
- feature: feat(project): expose Project file difference count (refs #3060) ([#3060](https://github.com/mblua/AgentsCommander/issues/3060), [PR #3066](https://github.com/mblua/AgentsCommander/pull/3066))
- feature: feat: show quiet Project file difference count (refs #3061) ([#3061](https://github.com/mblua/AgentsCommander/issues/3061), [PR #3116](https://github.com/mblua/AgentsCommander/pull/3116))
- feature: feat(settings): hide container options by default (refs #3063) ([#3063](https://github.com/mblua/AgentsCommander/issues/3063), [PR #3077](https://github.com/mblua/AgentsCommander/pull/3077))
- feature: feat(team): import agents using source project identity (refs #3072) ([#3072](https://github.com/mblua/AgentsCommander/issues/3072), [PR #3082](https://github.com/mblua/AgentsCommander/pull/3082))
- feature: feat: preserve cross-project agents in New Team (refs #3073) ([#3073](https://github.com/mblua/AgentsCommander/issues/3073), [PR #3102](https://github.com/mblua/AgentsCommander/pull/3102))
- feature: feat: archive message source snapshots (refs #3081) ([#3081](https://github.com/mblua/AgentsCommander/issues/3081), [PR #3090](https://github.com/mblua/AgentsCommander/pull/3090))
- feature: feat: add shared room inactivity observer (refs #3086) ([#3086](https://github.com/mblua/AgentsCommander/issues/3086), [PR #3114](https://github.com/mblua/AgentsCommander/pull/3114))
- fix: fix: share confirmed room activity across consumers (refs #3087) ([#3087](https://github.com/mblua/AgentsCommander/issues/3087), [PR #3115](https://github.com/mblua/AgentsCommander/pull/3115))
- maintenance: feat: add scoped rail diagnostic ingress (refs #3092) ([#3092](https://github.com/mblua/AgentsCommander/issues/3092), [PR #3094](https://github.com/mblua/AgentsCommander/pull/3094))
- maintenance: chore: record rail gestures and refresh diagnostics (refs #3093) ([#3093](https://github.com/mblua/AgentsCommander/issues/3093), [PR #3103](https://github.com/mblua/AgentsCommander/pull/3103))
- fix: fix: recognize Codex folder-trust blocking menu (refs #3095) ([#3095](https://github.com/mblua/AgentsCommander/issues/3095), [PR #3096](https://github.com/mblua/AgentsCommander/pull/3096))
- maintenance: feat: prepare id_hash_debug storage (refs #3098) ([#3098](https://github.com/mblua/AgentsCommander/issues/3098), [PR #3105](https://github.com/mblua/AgentsCommander/pull/3105))
- maintenance: feat(config): tolerate id hash debug metadata (refs #3099) ([#3099](https://github.com/mblua/AgentsCommander/issues/3099), [PR #3106](https://github.com/mblua/AgentsCommander/pull/3106))
- maintenance: feat(config): derive original id hash debug inputs (refs #3100) ([#3100](https://github.com/mblua/AgentsCommander/issues/3100), [PR #3108](https://github.com/mblua/AgentsCommander/pull/3108))
- maintenance: feat: preserve hash debug metadata in frontend transport (refs #3101) ([#3101](https://github.com/mblua/AgentsCommander/issues/3101), [PR #3112](https://github.com/mblua/AgentsCommander/pull/3112))
- feature: feat: group message archives by day (refs #3120) ([#3120](https://github.com/mblua/AgentsCommander/issues/3120), [PR #3121](https://github.com/mblua/AgentsCommander/pull/3121))
- feature: feat: persist room inactivity confirmation preference (refs #3123) ([#3123](https://github.com/mblua/AgentsCommander/issues/3123), [PR #3127](https://github.com/mblua/AgentsCommander/pull/3127))
- feature: feat: notify room inactivity preference changes (refs #3124) ([#3124](https://github.com/mblua/AgentsCommander/issues/3124), [PR #3128](https://github.com/mblua/AgentsCommander/pull/3128))
- feature: feat: recover room idle settings across realms (refs #3125) ([#3125](https://github.com/mblua/AgentsCommander/issues/3125), [PR #3129](https://github.com/mblua/AgentsCommander/pull/3129))
- feature: feat: configure room inactivity delay in Settings (refs #3126) ([#3126](https://github.com/mblua/AgentsCommander/issues/3126), [PR #3130](https://github.com/mblua/AgentsCommander/pull/3130))
- maintenance: chore(pty): add focus input debug logs (refs #3131) ([#3131](https://github.com/mblua/AgentsCommander/issues/3131), [PR #3133](https://github.com/mblua/AgentsCommander/pull/3133))
- fix: fix(settings): replace environment source badges with red halo (refs #3134) ([#3134](https://github.com/mblua/AgentsCommander/issues/3134), [PR #3136](https://github.com/mblua/AgentsCommander/pull/3136))
- feature: feat: import only generic agent assets (refs #3142) ([#3142](https://github.com/mblua/AgentsCommander/issues/3142), [PR #3154](https://github.com/mblua/AgentsCommander/pull/3154))
- fix: fix(session): retain last prompt across resume (refs #3155) ([#3155](https://github.com/mblua/AgentsCommander/issues/3155), [PR #3158](https://github.com/mblua/AgentsCommander/pull/3158))
- fix: fix(terminal): preserve newer last prompt events (refs #3156) ([#3156](https://github.com/mblua/AgentsCommander/issues/3156), [PR #3161](https://github.com/mblua/AgentsCommander/pull/3161))
- fix: fix(sidebar): preserve first group rail click refs #3157 ([#3157](https://github.com/mblua/AgentsCommander/issues/3157), [PR #3160](https://github.com/mblua/AgentsCommander/pull/3160))
- maintenance: ci: add sanitizing baseline and text gates (refs #3167) ([#3167](https://github.com/mblua/AgentsCommander/issues/3167), [PR #3170](https://github.com/mblua/AgentsCommander/pull/3170))
- maintenance: ci: add layout and edge sanitizing gates (#3168) ([#3168](https://github.com/mblua/AgentsCommander/issues/3168), [PR #3173](https://github.com/mblua/AgentsCommander/pull/3173))
- fix: fix(cli): validate Root message filenames before enqueue (refs #3171) ([#3171](https://github.com/mblua/AgentsCommander/issues/3171), [PR #3285](https://github.com/mblua/AgentsCommander/pull/3285))
- maintenance: ci: versioned allowlist for sanitizing gates (closes #3182) ([#3182](https://github.com/mblua/AgentsCommander/issues/3182), [PR #3183](https://github.com/mblua/AgentsCommander/pull/3183))
- maintenance: ci(workflows): cut PR regression gates wall time (refs #3184) ([#3184](https://github.com/mblua/AgentsCommander/issues/3184), [PR #3185](https://github.com/mblua/AgentsCommander/pull/3185))
- fix: fix(mailbox): report failed self-handoffs to the UI and the agent (refs #3189) ([#3189](https://github.com/mblua/AgentsCommander/issues/3189), [PR #3300](https://github.com/mblua/AgentsCommander/pull/3300))
- maintenance: fix(#3248): reproduce permanent Closing veto and add close-block diagnostics ([#3248](https://github.com/mblua/AgentsCommander/issues/3248), [PR #3326](https://github.com/mblua/AgentsCommander/pull/3326))
- feature: feat: diagnose room creation latency with stage logs (refs #3262) ([#3262](https://github.com/mblua/AgentsCommander/issues/3262), [PR #3268](https://github.com/mblua/AgentsCommander/pull/3268))
- fix: fix(mailbox): bound teardown and Host PTY observation (refs #3271) ([#3271](https://github.com/mblua/AgentsCommander/issues/3271), [PR #3302](https://github.com/mblua/AgentsCommander/pull/3302))
- feature: feat: add immediate self-clear for every agent (refs #3275) ([#3275](https://github.com/mblua/AgentsCommander/issues/3275), [PR #3279](https://github.com/mblua/AgentsCommander/pull/3279))
- fix: fix: sync session snapshots and coordinator clocks before publishing (refs #3278) ([#3278](https://github.com/mblua/AgentsCommander/issues/3278), [PR #3280](https://github.com/mblua/AgentsCommander/pull/3280))
- fix: fix(pty): re-check internal notice admission after writer permit (refs #3286) ([#3286](https://github.com/mblua/AgentsCommander/issues/3286), [PR #3292](https://github.com/mblua/AgentsCommander/pull/3292))
- maintenance: ci: stop committing module-arcs.txt; generate it in CI (#3288) ([#3288](https://github.com/mblua/AgentsCommander/issues/3288), [PR #3296](https://github.com/mblua/AgentsCommander/pull/3296))
- security: fix(security): UUID request IDs, physical Root identity and pre-effect repair authority refs #3318 ([#3318](https://github.com/mblua/AgentsCommander/issues/3318), [PR #3323](https://github.com/mblua/AgentsCommander/pull/3323))
- fix: fix(mailbox): keep reject_message archives inside outbox/rejected (#3351) ([#3351](https://github.com/mblua/AgentsCommander/issues/3351), [PR #3419](https://github.com/mblua/AgentsCommander/pull/3419))
- feature: feat: embedded web browser in the terminal pane (#3367) ([#3367](https://github.com/mblua/AgentsCommander/issues/3367), [PR #3427](https://github.com/mblua/AgentsCommander/pull/3427))
- fix: fix(mailbox): report failed Codex clear and fence replay (refs #3371) ([#3371](https://github.com/mblua/AgentsCommander/issues/3371), [PR #3457](https://github.com/mblua/AgentsCommander/pull/3457))
- feature: feat: seed a default self-handoff skill (closes #3374) ([#3374](https://github.com/mblua/AgentsCommander/issues/3374), [PR #3376](https://github.com/mblua/AgentsCommander/pull/3376))
- fix: fix(mailbox): keep move_to_delivered archives inside outbox/delivered (#3417) ([#3417](https://github.com/mblua/AgentsCommander/issues/3417), [PR #3423](https://github.com/mblua/AgentsCommander/pull/3423))
- fix: fix(mailbox): fall back for Windows-invalid and overlong archive ids (refs #3426) ([#3426](https://github.com/mblua/AgentsCommander/issues/3426), [PR #3429](https://github.com/mblua/AgentsCommander/pull/3429))
- fix: fix(restore): honor persisted Root fresh intent (refs #3431) ([#3431](https://github.com/mblua/AgentsCommander/issues/3431), [PR #3452](https://github.com/mblua/AgentsCommander/pull/3452))
- fix: fix(mailbox): require regular settled receipt files (refs #3475) ([#3475](https://github.com/mblua/AgentsCommander/issues/3475), [PR #3497](https://github.com/mblua/AgentsCommander/pull/3497))
- maintenance: ci: reject new or growing Rust functions above Clippy too_many_lines (refs #3507) ([#3507](https://github.com/mblua/AgentsCommander/issues/3507), [PR #3514](https://github.com/mblua/AgentsCommander/pull/3514))
- maintenance: ci: enforce ten additional Clippy lints (refs #3512) ([#3512](https://github.com/mblua/AgentsCommander/issues/3512), [PR #3519](https://github.com/mblua/AgentsCommander/pull/3519))
- maintenance: ci: prevent growth in seven Clippy lint budgets (refs #3528) ([#3528](https://github.com/mblua/AgentsCommander/issues/3528), [PR #3531](https://github.com/mblua/AgentsCommander/pull/3531))

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

Before implementation, independently query remote main through raw Git and the GitHub ref API. Both must equal `59812e1c631e971cee0e85c5ad62a6affa30001f`; the GitHub commit API must return the ordered parent list [3e64aaf4c89133ab2ca2b4a4873f628c60a690e1, edbcf6b4aef40434de74470f0e29c3bf1d7207c5]; the contents API must return workflow blob `c616f0303d85368a751e1c419ed32b5a091a5203`. Any mismatch is `FROZEN_INPUT_CHANGED`: discard the bundle and run the generator again from a new config/output path.

This gate is not reused after required merges. Later gates bind `FINAL_CANDIDATE_MAIN` instead.

## 5. Hardening PR contract

Branch from the exact planning base. The exact changed-path allowlist is:

```text
.github/workflows/release.yml
docs/releases/v0.46.0/CHANGELOG.release.md
docs/releases/v0.46.0/candidate-assets.v1.json
docs/releases/v0.46.0/input-manifest.v1.json
docs/releases/v0.46.0/predecessor-assets.v1.json
docs/releases/v0.46.0/release-authority-v1.txt
docs/releases/v0.46.0/release-body.md
docs/releases/v0.46.0/release-plan.md
docs/releases/v0.46.0/SHA256SUMS
```

The allowlist is closed: no wildcard and no other changed path is permitted. Its final entry is the canonical plan path derived solely from the release issue number and candidate version; it is not a bundle artifact. The bundle files must be copied byte-for-byte from the reviewed output and verified against `SHA256SUMS` on the PR head. The workflow implementation must be generic for every normal `vX.Y.Z`; candidate-specific values live only in `docs/releases/$TAG/`.

For the selected merge method, record `HARDENING_HEAD` only after exact-head review. After protected merge, require a two-parent merge commit and assert the ordered parent vector is exactly:

```text
[59812e1c631e971cee0e85c5ad62a6affa30001f, HARDENING_HEAD]
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

Jobs that run GitHub attestation verification receive `attestations: read` and explicit `GH_TOKEN: ${{ github.token }}`. Before either attestation path, download only `gh_2.103.3_linux_amd64.tar.gz`, verify SHA-256 `4c806978ffce1cec70853302a56d70ac6da4f19bae7adca4e1a8ab00633dd91d`, install from that verified archive, and require `gh version 2.103.3` before use.

Each macOS architecture is built once. The raw binary, app archive, and installer for that architecture must derive from that one build and one recorded digest manifest; no second macOS build may generate a competing asset.

Derive the immediate predecessor dynamically by strict normal-SemVer comparison, then require its exact annotated object, peeled commit, immutable public Release, and asset ledger. Do not use mutable `latest` alone as predecessor authority.

### Changelog and Release body

Replace the exact current `Unreleased` body once: remove it from `Unreleased` and insert it under `## 0.46.0`. The expected complete result is `CHANGELOG.release.md`; use `release-body.md` verbatim for the GitHub Release. Duplicate or residual copies fail.

Every version-side scope/changelog declaration below is mandatory; a missing scope issue, summary substring, changelog substring, or whole-word alternative fails closed:

- No release-specific scope/changelog semantic assertions are declared.

### Package and asset gates

Run the repository version tool for `0.46.0`; do not hand-edit version surfaces. Require all eight parsed surfaces to equal the candidate on the version PR head. Run `npm pack --dry-run`, create the tarball, inspect it, and smoke-install from that exact tarball before tagging.

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

After explicit authorization, create one annotated tag at `FINAL_CANDIDATE_MAIN` using the exact `docs/releases/v0.46.0/release-authority-v1.txt` bytes. Before push, require the local tag object type to be `tag`, its payload to match byte-for-byte, and its peeled commit to equal `FINAL_CANDIDATE_MAIN`.

After push, resolve the two remote records and execute exact assertions against the local annotated object and peeled commit. Do not merely display them. A missing record, extra record, changed object, or changed peeled commit stops all later claims.

The tag workflow then applies the every-job bootstrap, reuses only the exact draft for the exact tag on rerun, uploads/verifies the exact ledger, makes the Release immutable and public before npm, publishes npm once with provenance, and performs a clean install from the registry.

## 9. Recovery and completion

- Before GitHub publication, a failed run may resume only with the exact same annotated-object/peeled pair, payload, final candidate, draft id, and asset digests.
- After an immutable public Release exists, never delete, replace, or recreate it. If npm is absent, resume only the npm job from the exact verified Release and package tarball.
- If npm already contains the version, require exact repository/tag/provenance/tarball identity; otherwise stop for a new version.
- Never use `--clobber` to hide a digest mismatch. Idempotent overwrite is allowed only when the existing asset already belongs to the same guarded draft and the replacement digest is the exact expected digest.

Completion requires all 17 candidate assets and checksums, a public immutable GitHub Release, verified attestations, npm dist-tag/version/provenance/signature, clean npm install, and the destination executable reporting `0.46.0`. WG23 independently repeats GitHub/npm/executable verification before declaring the release complete.

## 10. Required negative suite

The workflow and preparation generator must reject: event SHA equated to tag object; missing candidate-tree evidence; a missing, changed, wildcarded, duplicated, or extra canonical-plan allowlist route; canonical-plan entry into `review-set-v1`, `SHA256SUMS`, or the bundle archive; a caller-supplied canonical-plan config key; reordered/duplicated execution; stale planning-base final gate; ancestry-only topology; absent/moved/lightweight/duplicate tag records; post-tag absence semantics; insufficient uploader permission or excess OIDC; duplicated/weakened changelog; mutable/mismatched GitHub CLI; missing npm registry/version/token gates; unsatisfied reviewer authority; malformed manifests/payloads; changed main; candidate collisions; ambiguous GitHub/npm errors; duplicate/missing/extra assets or facts; path traversal; output escape; caller-controlled CLI fixture injection; credential material in any input/evidence/error/artifact; and wrong `review-set-v1` order, byte, separator, terminator, or length-prefix semantics.
