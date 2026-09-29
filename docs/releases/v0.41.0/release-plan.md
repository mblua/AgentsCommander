# Release preparation plan: v0.41.0

Status: `REVIEW_REQUIRED`. This generated bundle is not approval to implement, tag, publish, release, or deploy.

Repository: `mblua/AgentsCommander`  
Release issue: https://github.com/mblua/AgentsCommander/issues/2777  
Candidate: `0.41.0` / `v0.41.0`  
Planning base: `4dcc873eefff46ef23421fc203a7981eb731bed3`  
Predecessor: `v0.40.0`

## 1. Exact review identity

The reviewed object is the complete generated bundle. Certify the exact `SHA256SUMS` bytes and separately record its SHA-256. No artifact may be regenerated, reformatted, or copied through a newline-changing tool after certification.

The annotated-tag message is the exact bytes of `release-authority-v1.txt`. Its `review-set-sha256` binds the plan, evidence manifest, changelog input, Release body, and both asset ledgers without a circular self-hash. The candidate tree must contain every bundle file at `docs/releases/v0.41.0/` before tagging.

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

- `docs/releases/v0.41.0/CHANGELOG.release.md`
- `docs/releases/v0.41.0/candidate-assets.v1.json`
- `docs/releases/v0.41.0/input-manifest.v1.json`
- `docs/releases/v0.41.0/predecessor-assets.v1.json`
- `docs/releases/v0.41.0/release-authority-v1.txt`
- `docs/releases/v0.41.0/release-body.md`
- `docs/releases/v0.41.0/release-plan.md`
- `docs/releases/v0.41.0/SHA256SUMS`

The hardening allowlist is closed at the release workflow plus these evidence paths. No release-hardening plan file is part of the contract: AgentsCommander issue #2183 removed `plans/` from the repository and dropped the derived plan route with it. Any implementation record is governance content kept outside the repository, and this bundle never hashes or incorporates it.

## 2. Frozen read-only facts

- Git remote main and GitHub API agree at `4dcc873eefff46ef23421fc203a7981eb731bed3`.
- Ordered planning-base parents: [a725a72e0bbf86a6b201db09391e7f72702781ef, 4da2598a9d495a5e32da99025aee5137e5d99908].
- Base `.github/workflows/release.yml` blob: `ed64253db473dd0de792d6c214b2eb164652c93b`; content SHA-256: `5fc231d824614084cfaa37ec5567dc5d32eadc797c5e38a99643134a2e508aad`.
- Predecessor annotated object: `62fc5c69742e768656d025f3db822c3add5e4c3b`; peeled commit: `b7380318cdf6dcc19d02d11be55c684d034f0752`.
- Predecessor immutable GitHub Release id: `397431753`.
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

- `package.json` `/version`: `0.40.0` (blob `cf4c364541bdb14aa5641f722fe26006dcfb3b6a`)
- `package-lock.json` `/version`: `0.40.0` (blob `41e10e5acc08cb490c6da9bd90c0253b0b301d99`)
- `package-lock.json` `/packages//version`: `0.40.0` (blob `41e10e5acc08cb490c6da9bd90c0253b0b301d99`)
- `npm/package.json` `/version`: `0.40.0` (blob `1f5842a7e646c88500decece30d8d42f9805e881`)
- `npm/install.js` `const VERSION`: `0.40.0` (blob `5e077f46e182ee5978bb0b248e444c9e2ff89be3`)
- `src-tauri/Cargo.toml` `[package].version`: `0.40.0` (blob `0423f4fc7e1f91b148f7e307d973c8464a1c4724`)
- `Cargo.lock` `agentscommander.version`: `0.40.0` (blob `97fd959b8798329b64b579092fe3b56c280e5981`)
- `src-tauri/tauri.conf.json` `/version`: `0.40.0` (blob `e2a52f647b687bb8241cba95c332aeb005f5ad6f`)

Approved scope:

- maintenance: test(sidebar): add Coding Agent picker testids (#2528) ([#2528](https://github.com/mblua/AgentsCommander/issues/2528), [PR #2706](https://github.com/mblua/AgentsCommander/pull/2706))
- feature: feat(picker): instrument selection timing (refs #2555) ([#2555](https://github.com/mblua/AgentsCommander/issues/2555), [PR #2722](https://github.com/mblua/AgentsCommander/pull/2722))
- feature: feat(sidebar): orphan-adoption notice on replica rows (#2568) ([#2568](https://github.com/mblua/AgentsCommander/issues/2568), [PR #2677](https://github.com/mblua/AgentsCommander/pull/2677))
- fix: fix(inject): strip wake wrap suffix, observe submit seam (#2586 phase 1) ([#2586](https://github.com/mblua/AgentsCommander/issues/2586), [PR #2692](https://github.com/mblua/AgentsCommander/pull/2692))
- maintenance: sidebar: keyboard access for 5 collapsible headers (S1082, #2655 phase A) ([#2657](https://github.com/mblua/AgentsCommander/issues/2657), [PR #2661](https://github.com/mblua/AgentsCommander/pull/2661))
- feature: feat(sidebar): keyboard access for five more ProjectPanel rows (refs #2658) ([#2658](https://github.com/mblua/AgentsCommander/issues/2658), [PR #2734](https://github.com/mblua/AgentsCommander/pull/2734))
- feature: feat(sidebar): keyboard access for nested rows (refs #2659) ([#2659](https://github.com/mblua/AgentsCommander/issues/2659), [PR #2743](https://github.com/mblua/AgentsCommander/pull/2743))
- fix: fix(scripts): resolve git by absolute path in scripts (#2670) ([#2670](https://github.com/mblua/AgentsCommander/issues/2670), [PR #2671](https://github.com/mblua/AgentsCommander/pull/2671))
- maintenance: test(loops): pin state write before audit append (#2679) ([#2679](https://github.com/mblua/AgentsCommander/issues/2679), [PR #2699](https://github.com/mblua/AgentsCommander/pull/2699))
- feature: feat(sidebar): show weekly remaining quota beside agents (refs #2681) ([#2681](https://github.com/mblua/AgentsCommander/issues/2681), [PR #2689](https://github.com/mblua/AgentsCommander/pull/2689))
- fix: fix(loops): serialize Loop config+state writes across processes (#2682) ([#2682](https://github.com/mblua/AgentsCommander/issues/2682), [PR #2700](https://github.com/mblua/AgentsCommander/pull/2700))
- fix: fix(pty): launch agents through the effective search path on Linux (#2684) ([#2684](https://github.com/mblua/AgentsCommander/issues/2684), [PR #2708](https://github.com/mblua/AgentsCommander/pull/2708))
- fix: fix(pty): apply launch search path at every spawn site (#2685) ([#2685](https://github.com/mblua/AgentsCommander/issues/2685), [PR #2721](https://github.com/mblua/AgentsCommander/pull/2721))
- feature: feat(quota): support remaining-percentage screen source (refs #2686) ([#2686](https://github.com/mblua/AgentsCommander/issues/2686), [PR #2705](https://github.com/mblua/AgentsCommander/pull/2705))
- feature: feat: add kind-aware Codex weekly quota contract (refs #2687) ([#2687](https://github.com/mblua/AgentsCommander/issues/2687), [PR #2710](https://github.com/mblua/AgentsCommander/pull/2710))
- feature: feat(#2688): default Codex weekly quota on both creation paths ([#2688](https://github.com/mblua/AgentsCommander/issues/2688), [PR #2723](https://github.com/mblua/AgentsCommander/pull/2723))
- feature: feat(testability): add ui-pointer and ui-key CLI verbs (refs #2690) ([#2690](https://github.com/mblua/AgentsCommander/issues/2690), [PR #2701](https://github.com/mblua/AgentsCommander/pull/2701))
- feature: feat(ui): dispatch pointer and key automation events (refs #2691) ([#2691](https://github.com/mblua/AgentsCommander/issues/2691), [PR #2702](https://github.com/mblua/AgentsCommander/pull/2702))
- fix: fix(loops): atomic config write, replace retry and CAS state write (#2694, phase A of #2678) ([#2694](https://github.com/mblua/AgentsCommander/issues/2694), [PR #2696](https://github.com/mblua/AgentsCommander/pull/2696))
- fix: fix(loops): split scheduler lock so Loop save no longer blocks on delivery (#2695) ([#2695](https://github.com/mblua/AgentsCommander/issues/2695), [PR #2697](https://github.com/mblua/AgentsCommander/pull/2697))
- fix: fix(loops): do not re-deliver a run whose post-delivery state write failed (#2698) ([#2698](https://github.com/mblua/AgentsCommander/issues/2698), [PR #2707](https://github.com/mblua/AgentsCommander/pull/2707))
- feature: feat(settings): General tab categories + cross-category search (#2704) ([#2704](https://github.com/mblua/AgentsCommander/issues/2704), [PR #2732](https://github.com/mblua/AgentsCommander/pull/2732))
- fix: fix(frontend): remove agent-help/ipc dependency cycle (#2711) ([#2711](https://github.com/mblua/AgentsCommander/issues/2711), [PR #2725](https://github.com/mblua/AgentsCommander/pull/2725))
- maintenance: test: support occupied Windows default root (refs #2712) ([#2712](https://github.com/mblua/AgentsCommander/issues/2712), [PR #2730](https://github.com/mblua/AgentsCommander/pull/2730))
- maintenance: refactor(config): file naming phase B1a, migration engine (refs #2703) ([#2713](https://github.com/mblua/AgentsCommander/issues/2713), [PR #2724](https://github.com/mblua/AgentsCommander/pull/2724))
- maintenance: refactor(naming): B1b migrate the instance families to layered names (#2714) ([#2714](https://github.com/mblua/AgentsCommander/issues/2714), [PR #2729](https://github.com/mblua/AgentsCommander/pull/2729))
- maintenance: refactor(#2715): B2 migrate coding-agents catalog family names ([#2715](https://github.com/mblua/AgentsCommander/issues/2715), [PR #2737](https://github.com/mblua/AgentsCommander/pull/2737))
- maintenance: refactor(settings): move agents and codingAgentProfiles to the instance agents file (refs #2716) ([#2716](https://github.com/mblua/AgentsCommander/issues/2716), [PR #2759](https://github.com/mblua/AgentsCommander/pull/2759))
- maintenance: refactor(config): migrate .ac/project-settings.json to layer 50 (refs #2717) ([#2717](https://github.com/mblua/AgentsCommander/issues/2717), [PR #2762](https://github.com/mblua/AgentsCommander/pull/2762))
- maintenance: refactor: file naming phase B4b, migrate loop state files (#2718) ([#2718](https://github.com/mblua/AgentsCommander/issues/2718), [PR #2763](https://github.com/mblua/AgentsCommander/pull/2763))
- maintenance: refactor(ui): B5 name the renamed settings, agents and project files (#2719) ([#2719](https://github.com/mblua/AgentsCommander/issues/2719), [PR #2765](https://github.com/mblua/AgentsCommander/pull/2765))
- docs: docs: name the phase-B config files and record no-downgrade (#2720) ([#2720](https://github.com/mblua/AgentsCommander/issues/2720), [PR #2773](https://github.com/mblua/AgentsCommander/pull/2773))
- fix: fix: Codex weekly quota pattern matches the real lowercase 'weekly' row (#2726) ([#2726](https://github.com/mblua/AgentsCommander/issues/2726), [PR #2727](https://github.com/mblua/AgentsCommander/pull/2727))
- fix: fix(loops): allow disabling loops whose room is missing and stop repeated error toasts (refs #2733) ([#2733](https://github.com/mblua/AgentsCommander/issues/2733), [PR #2735](https://github.com/mblua/AgentsCommander/pull/2735))
- feature: feat(catalog): add optional installCommands field (refs #2736, #2738) ([#2738](https://github.com/mblua/AgentsCommander/issues/2738), [PR #2750](https://github.com/mblua/AgentsCommander/pull/2750))
- feature: #2736 P2 welcome status: tested table and detection IPC (#2739) ([#2739](https://github.com/mblua/AgentsCommander/issues/2739), [PR #2753](https://github.com/mblua/AgentsCommander/pull/2753))
- feature: feat(agent-install): silent install runner (#2740) ([#2740](https://github.com/mblua/AgentsCommander/issues/2740), [PR #2755](https://github.com/mblua/AgentsCommander/pull/2755))
- feature: #2736 P4 Welcome status chips and order (#2741) ([#2741](https://github.com/mblua/AgentsCommander/issues/2741), [PR #2758](https://github.com/mblua/AgentsCommander/pull/2758))
- feature: #2736 P5: Welcome Copy and Install actions (#2742) ([#2742](https://github.com/mblua/AgentsCommander/issues/2742), [PR #2761](https://github.com/mblua/AgentsCommander/pull/2761))
- feature: feat(welcome): disclose captured install failure output (#2745) ([#2745](https://github.com/mblua/AgentsCommander/issues/2745), [PR #2772](https://github.com/mblua/AgentsCommander/pull/2772))
- maintenance: test(theme): light-mode contrast harness and allowlist (#2746) ([#2746](https://github.com/mblua/AgentsCommander/issues/2746), [PR #2752](https://github.com/mblua/AgentsCommander/pull/2752))
- fix: fix(theme): light-mode info toast and Save default contrast (#2747) ([#2747](https://github.com/mblua/AgentsCommander/issues/2747), [PR #2754](https://github.com/mblua/AgentsCommander/pull/2754))
- fix: fix(theme): define light-theme tokens and fix 33 light contrast rules (#2748) ([#2748](https://github.com/mblua/AgentsCommander/issues/2748), [PR #2757](https://github.com/mblua/AgentsCommander/pull/2757))
- fix: fix(theme): light-theme literal colours (Epic #2744 phase D) ([#2749](https://github.com/mblua/AgentsCommander/issues/2749), [PR #2760](https://github.com/mblua/AgentsCommander/pull/2760))
- fix: fix(sidebar): theme-aware selected-row rail default in light mode (#2751) ([#2751](https://github.com/mblua/AgentsCommander/issues/2751), [PR #2769](https://github.com/mblua/AgentsCommander/pull/2769))
- fix: fix(capture): preserve newer co-managed candidates, refs #2756 ([#2756](https://github.com/mblua/AgentsCommander/issues/2756), [PR #2771](https://github.com/mblua/AgentsCommander/pull/2771))
- maintenance: chore(#2764): update website references to agentscommander.org ([#2764](https://github.com/mblua/AgentsCommander/issues/2764), [PR #2766](https://github.com/mblua/AgentsCommander/pull/2766))

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

Before implementation, independently query remote main through raw Git and the GitHub ref API. Both must equal `4dcc873eefff46ef23421fc203a7981eb731bed3`; the GitHub commit API must return the ordered parent list [a725a72e0bbf86a6b201db09391e7f72702781ef, 4da2598a9d495a5e32da99025aee5137e5d99908]; the contents API must return workflow blob `ed64253db473dd0de792d6c214b2eb164652c93b`. Any mismatch is `FROZEN_INPUT_CHANGED`: discard the bundle and run the generator again from a new config/output path.

This gate is not reused after required merges. Later gates bind `FINAL_CANDIDATE_MAIN` instead.

## 5. Hardening PR contract

Branch from the exact planning base. The exact changed-path allowlist is:

```text
.github/workflows/release.yml
docs/releases/v0.41.0/CHANGELOG.release.md
docs/releases/v0.41.0/candidate-assets.v1.json
docs/releases/v0.41.0/input-manifest.v1.json
docs/releases/v0.41.0/predecessor-assets.v1.json
docs/releases/v0.41.0/release-authority-v1.txt
docs/releases/v0.41.0/release-body.md
docs/releases/v0.41.0/release-plan.md
docs/releases/v0.41.0/SHA256SUMS
```

The allowlist is closed: no wildcard and no other changed path is permitted. Its final entry is the canonical plan path derived solely from the release issue number and candidate version; it is not a bundle artifact. The bundle files must be copied byte-for-byte from the reviewed output and verified against `SHA256SUMS` on the PR head. The workflow implementation must be generic for every normal `vX.Y.Z`; candidate-specific values live only in `docs/releases/$TAG/`.

For the selected merge method, record `HARDENING_HEAD` only after exact-head review. After protected merge, require a two-parent merge commit and assert the ordered parent vector is exactly:

```text
[4dcc873eefff46ef23421fc203a7981eb731bed3, HARDENING_HEAD]
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

Replace the exact current `Unreleased` body once: remove it from `Unreleased` and insert it under `## 0.41.0`. The expected complete result is `CHANGELOG.release.md`; use `release-body.md` verbatim for the GitHub Release. Duplicate or residual copies fail.

Every version-side scope/changelog declaration below is mandatory; a missing scope issue, summary substring, changelog substring, or whole-word alternative fails closed:

- No release-specific scope/changelog semantic assertions are declared.

### Package and asset gates

Run the repository version tool for `0.41.0`; do not hand-edit version surfaces. Require all eight parsed surfaces to equal the candidate on the version PR head. Run `npm pack --dry-run`, create the tarball, inspect it, and smoke-install from that exact tarball before tagging.

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

After explicit authorization, create one annotated tag at `FINAL_CANDIDATE_MAIN` using the exact `docs/releases/v0.41.0/release-authority-v1.txt` bytes. Before push, require the local tag object type to be `tag`, its payload to match byte-for-byte, and its peeled commit to equal `FINAL_CANDIDATE_MAIN`.

After push, resolve the two remote records and execute exact assertions against the local annotated object and peeled commit. Do not merely display them. A missing record, extra record, changed object, or changed peeled commit stops all later claims.

The tag workflow then applies the every-job bootstrap, reuses only the exact draft for the exact tag on rerun, uploads/verifies the exact ledger, makes the Release immutable and public before npm, publishes npm once with provenance, and performs a clean install from the registry.

## 9. Recovery and completion

- Before GitHub publication, a failed run may resume only with the exact same annotated-object/peeled pair, payload, final candidate, draft id, and asset digests.
- After an immutable public Release exists, never delete, replace, or recreate it. If npm is absent, resume only the npm job from the exact verified Release and package tarball.
- If npm already contains the version, require exact repository/tag/provenance/tarball identity; otherwise stop for a new version.
- Never use `--clobber` to hide a digest mismatch. Idempotent overwrite is allowed only when the existing asset already belongs to the same guarded draft and the replacement digest is the exact expected digest.

Completion requires all 17 candidate assets and checksums, a public immutable GitHub Release, verified attestations, npm dist-tag/version/provenance/signature, clean npm install, and the destination executable reporting `0.41.0`. WG23 independently repeats GitHub/npm/executable verification before declaring the release complete.

## 10. Required negative suite

The workflow and preparation generator must reject: event SHA equated to tag object; missing candidate-tree evidence; a missing, changed, wildcarded, duplicated, or extra canonical-plan allowlist route; canonical-plan entry into `review-set-v1`, `SHA256SUMS`, or the bundle archive; a caller-supplied canonical-plan config key; reordered/duplicated execution; stale planning-base final gate; ancestry-only topology; absent/moved/lightweight/duplicate tag records; post-tag absence semantics; insufficient uploader permission or excess OIDC; duplicated/weakened changelog; mutable/mismatched GitHub CLI; missing npm registry/version/token gates; unsatisfied reviewer authority; malformed manifests/payloads; changed main; candidate collisions; ambiguous GitHub/npm errors; duplicate/missing/extra assets or facts; path traversal; output escape; caller-controlled CLI fixture injection; credential material in any input/evidence/error/artifact; and wrong `review-set-v1` order, byte, separator, terminator, or length-prefix semantics.
