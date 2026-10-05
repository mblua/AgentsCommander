# Release preparation plan: v0.43.0

Status: `REVIEW_REQUIRED`. This generated bundle is not approval to implement, tag, publish, release, or deploy.

Repository: `mblua/AgentsCommander`  
Release issue: https://github.com/mblua/AgentsCommander/issues/2874  
Candidate: `0.43.0` / `v0.43.0`  
Planning base: `ef50fa2eebdeda599952ae3093acb00a56e4d391`  
Predecessor: `v0.42.0`

## 1. Exact review identity

The reviewed object is the complete generated bundle. Certify the exact `SHA256SUMS` bytes and separately record its SHA-256. No artifact may be regenerated, reformatted, or copied through a newline-changing tool after certification.

The annotated-tag message is the exact bytes of `release-authority-v1.txt`. Its `review-set-sha256` binds the plan, evidence manifest, changelog input, Release body, and both asset ledgers without a circular self-hash. The candidate tree must contain every bundle file at `docs/releases/v0.43.0/` before tagging.

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

- `docs/releases/v0.43.0/CHANGELOG.release.md`
- `docs/releases/v0.43.0/candidate-assets.v1.json`
- `docs/releases/v0.43.0/input-manifest.v1.json`
- `docs/releases/v0.43.0/predecessor-assets.v1.json`
- `docs/releases/v0.43.0/release-authority-v1.txt`
- `docs/releases/v0.43.0/release-body.md`
- `docs/releases/v0.43.0/release-plan.md`
- `docs/releases/v0.43.0/SHA256SUMS`

The hardening allowlist is closed at the release workflow plus these evidence paths. No release-hardening plan file is part of the contract: AgentsCommander issue #2183 removed `plans/` from the repository and dropped the derived plan route with it. Any implementation record is governance content kept outside the repository, and this bundle never hashes or incorporates it.

## 2. Frozen read-only facts

- Git remote main and GitHub API agree at `ef50fa2eebdeda599952ae3093acb00a56e4d391`.
- Ordered planning-base parents: [1ccc653aa53116f383bd20d878f31afc988c5a22, 955a773e31958f46b4e8adc547af570581c7e903].
- Base `.github/workflows/release.yml` blob: `ed64253db473dd0de792d6c214b2eb164652c93b`; content SHA-256: `5fc231d824614084cfaa37ec5567dc5d32eadc797c5e38a99643134a2e508aad`.
- Predecessor annotated object: `3effd7665b0e396df3e487d7d55c319ee31bdb02`; peeled commit: `d9c7db925d406ab77eafd5c326a800db367342d1`.
- Predecessor immutable GitHub Release id: `401434819`.
- npm latest: `0.42.0`; candidate tag, Release, and npm version are absent at both discovery snapshots.
- Ruleset/reviewer authority: rules require approval; `mblua` has the documented admin exception only for unavailable self-review after every check passes.
- Tag immutability/binding policy: the repository lacks a selected tag-protection guarantee, so every job/rerun and final verifier fail on any object/peeled movement.
- Release asset upload design: direct uploader jobs receive job-local contents: write and no id-token.
- Frozen GitHub CLI: `gh 2.102.0`, archive `gh_2.102.0_linux_amd64.tar.gz`, SHA-256 `bb766f710eef8ede859c18578c72c327597cd4c8a85b06001b1f3843c6019386`.

Resolved action pins:

- `actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09` (discovered from `fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09`)
- `actions/setup-node@a0853c24544627f65ddf259abe73b1d18a591444` (discovered from `a0853c24544627f65ddf259abe73b1d18a591444`)
- `actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a` (discovered from `043fb46d1a93c77aae656e7c1c64a875d1fc6a0a`)
- `dtolnay/rust-toolchain@4360b52568e2003a75bf9bc1d59f33a8e3fc893c` (discovered from `4360b52568e2003a75bf9bc1d59f33a8e3fc893c`)
- `swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6` (discovered from `6323deb102c322ba6fcbdcafc7e3dddab59af2b6`)
- `tauri-apps/tauri-action@84b9d35b5fc46c1e45415bdb6144030364f7ebc5` (discovered from `84b9d35b5fc46c1e45415bdb6144030364f7ebc5`)

Current version surfaces to advance with the repository's version tool:

- `package.json` `/version`: `0.42.0` (blob `553383bb536093432d4543d8befcf1fd0049ffeb`)
- `package-lock.json` `/version`: `0.42.0` (blob `fc8f1b0b8cff1a9ac6e4919287211415d8d7c123`)
- `package-lock.json` `/packages//version`: `0.42.0` (blob `fc8f1b0b8cff1a9ac6e4919287211415d8d7c123`)
- `npm/package.json` `/version`: `0.42.0` (blob `740ad82aa5a794e9001bb4b8d272eb6f8842b815`)
- `npm/install.js` `const VERSION`: `0.42.0` (blob `8c4f82884e78ec10f9333263f6ed6e49dfe72bab`)
- `src-tauri/Cargo.toml` `[package].version`: `0.42.0` (blob `92e754b46762b2e9638d2e1172aa756685810eca`)
- `Cargo.lock` `agentscommander.version`: `0.42.0` (blob `22ce9ee354d2c8d9dbff44d5dc47b71fade4a939`)
- `src-tauri/tauri.conf.json` `/version`: `0.42.0` (blob `a66503f9a062318ab17c429334c51a451d7a5a15`)

Approved scope:

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

Before implementation, independently query remote main through raw Git and the GitHub ref API. Both must equal `ef50fa2eebdeda599952ae3093acb00a56e4d391`; the GitHub commit API must return the ordered parent list [1ccc653aa53116f383bd20d878f31afc988c5a22, 955a773e31958f46b4e8adc547af570581c7e903]; the contents API must return workflow blob `ed64253db473dd0de792d6c214b2eb164652c93b`. Any mismatch is `FROZEN_INPUT_CHANGED`: discard the bundle and run the generator again from a new config/output path.

This gate is not reused after required merges. Later gates bind `FINAL_CANDIDATE_MAIN` instead.

## 5. Hardening PR contract

Branch from the exact planning base. The exact changed-path allowlist is:

```text
.github/workflows/release.yml
docs/releases/v0.43.0/CHANGELOG.release.md
docs/releases/v0.43.0/candidate-assets.v1.json
docs/releases/v0.43.0/input-manifest.v1.json
docs/releases/v0.43.0/predecessor-assets.v1.json
docs/releases/v0.43.0/release-authority-v1.txt
docs/releases/v0.43.0/release-body.md
docs/releases/v0.43.0/release-plan.md
docs/releases/v0.43.0/SHA256SUMS
```

The allowlist is closed: no wildcard and no other changed path is permitted. Its final entry is the canonical plan path derived solely from the release issue number and candidate version; it is not a bundle artifact. The bundle files must be copied byte-for-byte from the reviewed output and verified against `SHA256SUMS` on the PR head. The workflow implementation must be generic for every normal `vX.Y.Z`; candidate-specific values live only in `docs/releases/$TAG/`.

For the selected merge method, record `HARDENING_HEAD` only after exact-head review. After protected merge, require a two-parent merge commit and assert the ordered parent vector is exactly:

```text
[ef50fa2eebdeda599952ae3093acb00a56e4d391, HARDENING_HEAD]
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

Jobs that run GitHub attestation verification receive `attestations: read` and explicit `GH_TOKEN: ${{ github.token }}`. Before either attestation path, download only `gh_2.102.0_linux_amd64.tar.gz`, verify SHA-256 `bb766f710eef8ede859c18578c72c327597cd4c8a85b06001b1f3843c6019386`, install from that verified archive, and require `gh version 2.102.0` before use.

Each macOS architecture is built once. The raw binary, app archive, and installer for that architecture must derive from that one build and one recorded digest manifest; no second macOS build may generate a competing asset.

Derive the immediate predecessor dynamically by strict normal-SemVer comparison, then require its exact annotated object, peeled commit, immutable public Release, and asset ledger. Do not use mutable `latest` alone as predecessor authority.

### Changelog and Release body

Replace the exact current `Unreleased` body once: remove it from `Unreleased` and insert it under `## 0.43.0`. The expected complete result is `CHANGELOG.release.md`; use `release-body.md` verbatim for the GitHub Release. Duplicate or residual copies fail.

Every version-side scope/changelog declaration below is mandatory; a missing scope issue, summary substring, changelog substring, or whole-word alternative fails closed:

- No release-specific scope/changelog semantic assertions are declared.

### Package and asset gates

Run the repository version tool for `0.43.0`; do not hand-edit version surfaces. Require all eight parsed surfaces to equal the candidate on the version PR head. Run `npm pack --dry-run`, create the tarball, inspect it, and smoke-install from that exact tarball before tagging.

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

After explicit authorization, create one annotated tag at `FINAL_CANDIDATE_MAIN` using the exact `docs/releases/v0.43.0/release-authority-v1.txt` bytes. Before push, require the local tag object type to be `tag`, its payload to match byte-for-byte, and its peeled commit to equal `FINAL_CANDIDATE_MAIN`.

After push, resolve the two remote records and execute exact assertions against the local annotated object and peeled commit. Do not merely display them. A missing record, extra record, changed object, or changed peeled commit stops all later claims.

The tag workflow then applies the every-job bootstrap, reuses only the exact draft for the exact tag on rerun, uploads/verifies the exact ledger, makes the Release immutable and public before npm, publishes npm once with provenance, and performs a clean install from the registry.

## 9. Recovery and completion

- Before GitHub publication, a failed run may resume only with the exact same annotated-object/peeled pair, payload, final candidate, draft id, and asset digests.
- After an immutable public Release exists, never delete, replace, or recreate it. If npm is absent, resume only the npm job from the exact verified Release and package tarball.
- If npm already contains the version, require exact repository/tag/provenance/tarball identity; otherwise stop for a new version.
- Never use `--clobber` to hide a digest mismatch. Idempotent overwrite is allowed only when the existing asset already belongs to the same guarded draft and the replacement digest is the exact expected digest.

Completion requires all 17 candidate assets and checksums, a public immutable GitHub Release, verified attestations, npm dist-tag/version/provenance/signature, clean npm install, and the destination executable reporting `0.43.0`. WG23 independently repeats GitHub/npm/executable verification before declaring the release complete.

## 10. Required negative suite

The workflow and preparation generator must reject: event SHA equated to tag object; missing candidate-tree evidence; a missing, changed, wildcarded, duplicated, or extra canonical-plan allowlist route; canonical-plan entry into `review-set-v1`, `SHA256SUMS`, or the bundle archive; a caller-supplied canonical-plan config key; reordered/duplicated execution; stale planning-base final gate; ancestry-only topology; absent/moved/lightweight/duplicate tag records; post-tag absence semantics; insufficient uploader permission or excess OIDC; duplicated/weakened changelog; mutable/mismatched GitHub CLI; missing npm registry/version/token gates; unsatisfied reviewer authority; malformed manifests/payloads; changed main; candidate collisions; ambiguous GitHub/npm errors; duplicate/missing/extra assets or facts; path traversal; output escape; caller-controlled CLI fixture injection; credential material in any input/evidence/error/artifact; and wrong `review-set-v1` order, byte, separator, terminator, or length-prefix semantics.
